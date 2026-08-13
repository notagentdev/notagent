/**
 * Erzeugt `tests/fixtures/virtual-terminal-oracle.json`: das von
 * `@xterm/headless` 5.5.0 (dem Test-Terminal der TS-Suite) gerenderte Bild für
 * genau die Sequenzen, die `TuiMainScreen` und `TuiAltScreen` emittieren.
 *
 * Der Rust-Harness `test_terminal::VirtualTerminal` wird in
 * `tests/virtual_terminal.rs` dagegen geprüft (WS-A-Plan Task 1:
 * Akzeptanzkriterien Viewport, Scrollback, Cursorposition, Resize,
 * CSI/OSC/APC inkl. Synchronized Output).
 *
 * Aufruf (aus crates/notagent-tui/):
 *   node tools/gen-virtual-terminal-oracle.mjs > tests/fixtures/virtual-terminal-oracle.json
 */
import { createRequire } from "node:module";
const TS_REPO = process.env.NOTAGENT_TS_REPO ?? "/Users/dev/projects/notagent-main";
const require_ = createRequire(`${TS_REPO}/package.json`);
const xterm = require_("@xterm/headless");
const { Terminal } = xterm;

// Scenarios mirror exactly what TuiMainScreen / TuiAltScreen emit.
const CASES = [
  { name: "plain-crlf", cols: 16, rows: 4, w: ["a\r\nbb\r\nccc"] },
  { name: "written-trailing-spaces", cols: 16, rows: 3, w: ["base 0          \r\nbase 1          "] },
  { name: "erase-line-2K", cols: 16, rows: 3, w: ["hello world\r\nsecond", "\x1b[1;1H\x1b[2K"] },
  { name: "cursor-cuu-cud-cha", cols: 16, rows: 4, w: ["l0\r\nl1\r\nl2", "\x1b[2A\x1b[3GX"] },
  { name: "scroll-into-scrollback", cols: 8, rows: 3, w: ["a\r\nb\r\nc\r\nd\r\ne"] },
  { name: "clear-screen-home-3J", cols: 8, rows: 3, w: ["a\r\nb\r\nc\r\nd", "\x1b[2J\x1b[H\x1b[3J", "x"] },
  { name: "osc-title", cols: 8, rows: 2, w: ["\x1b]0;my title\x07ab"] },
  { name: "osc8-hyperlink", cols: 20, rows: 2, w: ["\x1b]8;;https://e.com\x1b\\link\x1b]8;;\x1b\\ end"] },
  { name: "apc-cursor-marker", cols: 12, rows: 2, w: ["ab\x1b_pi:c\x07cd"] },
  { name: "apc-kitty-image", cols: 12, rows: 2, w: ["\x1b_Ga=T,f=100,i=1;iVBORw0KGgo=\x1b\\X"] },
  { name: "sync-output", cols: 12, rows: 2, w: ["\x1b[?2026hab\r\ncd\x1b[?2026l"] },
  { name: "sgr-colors", cols: 12, rows: 2, w: ["\x1b[31mred\x1b[0m plain"] },
  { name: "wide-cjk", cols: 10, rows: 2, w: ["中文ab"] },
  { name: "emoji", cols: 10, rows: 2, w: ["👍x"] },
  { name: "cursor-hide-show", cols: 8, rows: 2, w: ["\x1b[?25lab\x1b[?25h"] },
  { name: "erase-from-cursor-J", cols: 10, rows: 3, w: ["aaa\r\nbbb\r\nccc", "\x1b[1;2H\x1b[J"] },
  { name: "cup-absolute", cols: 10, rows: 3, w: ["\x1b[3;1Hz"] },
  { name: "overwrite-shorter", cols: 10, rows: 2, w: ["abcdef", "\r\x1b[2Kxy"] },
  { name: "autowrap-off", cols: 6, rows: 3, w: ["\x1b[?7labcdefghi"] },
  { name: "autowrap-on", cols: 6, rows: 3, w: ["\x1b[?7habcdefghi"] },
  { name: "alt-screen", cols: 8, rows: 3, w: ["main\r\n", "\x1b[?1049h", "alt", "\x1b[?1049l"] },
  { name: "tab-char", cols: 12, rows: 2, w: ["a\tb"] },
];

function viewport(t) {
  const b = t.buffer.active, out = [];
  for (let i = 0; i < t.rows; i++) {
    const l = b.getLine(b.viewportY + i);
    out.push(l ? l.translateToString(true) : "");
  }
  return out;
}
function scrollbuf(t) {
  const b = t.buffer.active, out = [];
  for (let i = 0; i < b.length; i++) {
    const l = b.getLine(i);
    out.push(l ? l.translateToString(true) : "");
  }
  return out;
}
const flush = (t) => new Promise((r) => t.write("", () => r()));

const results = {};
for (const c of CASES) {
  const t = new Terminal({ cols: c.cols, rows: c.rows, disableStdin: true, allowProposedApi: true });
  for (const chunk of c.w) t.write(chunk);
  await flush(t);
  const b = t.buffer.active;
  results[c.name] = {
    cols: c.cols, rows: c.rows, writes: c.w,
    viewport: viewport(t), scrollback: scrollbuf(t),
    cursor: { x: b.cursorX, y: b.cursorY },
  };
}

// resize case: write, then resize, then read
{
  const t = new Terminal({ cols: 10, rows: 3, disableStdin: true, allowProposedApi: true });
  t.write("hello\r\nworld");
  await flush(t);
  t.resize(6, 4);
  await flush(t);
  const b = t.buffer.active;
  results["resize"] = { cols: 6, rows: 4, viewport: viewport(t), scrollback: scrollbuf(t), cursor: { x: b.cursorX, y: b.cursorY } };
}

console.log(JSON.stringify(results, null, 1));
