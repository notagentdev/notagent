#!/usr/bin/env python3
"""Record the real notagent TUI against an isolated, scripted local provider."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


ROOT = Path(__file__).resolve().parents[2]
SOURCE = """// Prices are stored in cents to avoid floating-point rounding.
// Shipping is free when the cart reaches the advertised threshold.
pub fn shipping_cost(subtotal: u32) -> u32 {
    if subtotal > 5000 {
        0
    } else {
        499
    }
}

#[cfg(test)]
mod tests {
    use super::shipping_cost;

    #[test]
    fn below_threshold_pays_shipping() {
        assert_eq!(shipping_cost(4999), 499);
    }

    #[test]
    fn at_threshold_shipping_is_free() {
        assert_eq!(shipping_cost(5000), 0);
    }

    #[test]
    fn above_threshold_shipping_is_free() {
        assert_eq!(shipping_cost(5001), 0);
    }
}
"""


def tool(name, arguments):
    return {"name": name, "arguments": json.dumps(arguments)}


STEPS = [
    ("I'll inspect the shipping rule using a compact source read.",
     tool("read_minified", {"path": "src/pricing.rs"})),
    ("The boundary is off by one: `> 5000` excludes exactly $50.\n\n"
     "**Plan**\n"
     "1. Change `>` to `>=`; keep the surrounding source intact.\n"
     "2. Run the three boundary tests: below, at and above $50.\n\n"
     "Ready to implement.", None),
    ("Applying the plan with the same conversation context.",
     tool("patch_minified", {"path": "src/pricing.rs",
                            "old_string": "subtotal > 5000",
                            "new_string": "subtotal >= 5000"})),
    ("The one-line fix is in place. Running the boundary tests.",
     tool("bash", {"command": "cargo test --lib --quiet"})),
    ("**Fixed and verified.** All 3 boundary tests pass.\n\n"
     "- Compact source read, precise patch, visible diff.\n"
     "- Plan and execution share one conversation.\n"
     "- Changes and test results stay in your terminal.", None),
]


class DemoServer(ThreadingHTTPServer):
    step = 0
    failure = None


class Provider(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path != "/v1/chat/completions" or self.server.step >= len(STEPS):
            self.server.failure = "Unexpected provider request"
            self.send_error(400, self.server.failure)
            return

        index = self.server.step
        messages = request.get("messages", [])
        last = messages[-1] if messages else {}
        result = str(last.get("content", ""))
        if last.get("role") == "tool" and any(
            marker in result.lower() for marker in ("error:", "test result: failed", "not found")
        ):
            self.server.failure = f"Tool failed: {result}"
            self.send_error(400, "The demo tool failed; see the recorder output")
            return
        if index == 4 and "3 passed" not in result:
            self.server.failure = f"Expected three passing tests, got: {result}"
            self.send_error(400, "Boundary tests did not pass")
            return

        text, call = STEPS[index]
        if call and call["name"] not in {
            entry.get("function", {}).get("name") for entry in request.get("tools", [])
        }:
            self.server.failure = f"Tool unavailable in current mode: {call['name']}"
            self.send_error(400, self.server.failure)
            return

        self.server.step += 1
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def emit(delta, finish=None):
            chunk = {"id": f"demo-{index}", "object": "chat.completion.chunk",
                     "created": 0, "model": "scripted-demo",
                     "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.flush()

        try:
            time.sleep(0.4)
            emit({"role": "assistant"})
            for offset in range(0, len(text), 5):
                emit({"content": text[offset:offset + 5]})
                time.sleep(0.025)
            if call:
                time.sleep(0.6)
                emit({"tool_calls": [{"index": 0, "id": f"call-{index}",
                                      "type": "function", "function": call}]})
            emit({}, "tool_calls" if call else "stop")
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            self.server.failure = "Recording disconnected from the scripted provider"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="notagent", help="Binary to record")
    parser.add_argument("--tape", type=Path, default=ROOT / "scripts/demo/notagent.tape")
    args = parser.parse_args()
    binary = shutil.which(args.binary)
    if not binary:
        parser.error(f"Binary not found: {args.binary}")
    for dependency in ("vhs", "ttyd", "ffmpeg", "cargo", "rustc"):
        if not shutil.which(dependency):
            parser.error(f"Missing dependency: {dependency}")

    output = ROOT / "assets/demo"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="notagent-vhs-") as temporary:
        directory = Path(temporary)
        project = directory / "shipping-demo"
        config = directory / "config"
        (project / "src").mkdir(parents=True)
        config.mkdir()
        (project / "src/pricing.rs").write_text(SOURCE)
        (project / "Cargo.toml").write_text(
            '[package]\nname = "shipping-demo"\nversion = "0.1.0"\nedition = "2021"\n'
            '\n[lib]\npath = "src/pricing.rs"\n'
        )
        server = DemoServer(("127.0.0.1", 0), Provider)
        model = {"id": "scripted-demo", "name": "Scripted demo", "reasoning": False,
                 "input": ["text"], "contextWindow": 100000, "maxTokens": 8000,
                 "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}}
        (config / "models.json").write_text(json.dumps({"providers": {"demo": {
            "baseUrl": f"http://127.0.0.1:{server.server_port}/v1",
            "api": "openai-completions", "apiKey": "local-demo", "models": [model]
        }}}))
        (config / "settings.json").write_text(json.dumps({
            "quietStartup": True, "lastMode": "plan", "theme": "dark",
            "enableInstallTelemetry": False, "enableAnalytics": False,
            "bashFilter": False, "atomicLeases": True,
        }))
        # An allowlist keeps ambient provider credentials and user settings out of VHS.
        environment = {key: os.environ[key] for key in
                       ("PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "VHS_BROWSER_PATH")
                       if key in os.environ}
        environment.update({
            "NOTAGENT_CODING_AGENT_DIR": str(config), "NOTAGENT_OFFLINE": "1",
            "NOTAGENT_TELEMETRY": "0", "DO_NOT_TRACK": "1",
            "NOTAGENT_DEMO_PROJECT": str(project), "NOTAGENT_DEMO_BINARY": binary,
            "ZDOTDIR": str(directory), "PS1": "$ ", "PROMPT": "$ ",
        })
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            subprocess.run(["vhs", str(args.tape.resolve())], cwd=ROOT,
                           env=environment, check=True)
            if server.failure or server.step != len(STEPS):
                raise RuntimeError(server.failure or f"Only {server.step}/{len(STEPS)} demo steps ran")
            actual = (project / "src/pricing.rs").read_text()
            if actual != SOURCE.replace("subtotal > 5000", "subtotal >= 5000"):
                raise RuntimeError("The recorded edit changed more than the intended comparison")
            print(f"Verified real source edit and three passing tests. GIF: {output / 'notagent.gif'}")
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
