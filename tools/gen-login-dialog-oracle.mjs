// Oracle for crates/notagent/tests/login_dialog.rs — drives the TypeScript
// LoginDialogComponent and prints the rendered lines plus the resolved values.
//
// `showAuth` launches the platform browser. Run it with a PATH that shadows the
// launcher, so the oracle stays a pure render:
//   mkdir -p /tmp/no-open && printf '#!/bin/sh\nexit 0\n' > /tmp/no-open/open \
//     && chmod +x /tmp/no-open/open && cp /tmp/no-open/open /tmp/no-open/xdg-open
//   PATH=/tmp/no-open:$PATH node tools/gen-login-dialog-oracle.mjs
import { setKeybindings } from "@notagent/tui";
import { KeybindingsManager } from "./src/core/keybindings.ts";
import { LoginDialogComponent } from "./src/modes/interactive/components/login-dialog.ts";
import { initTheme } from "./src/modes/interactive/theme/theme.ts";

initTheme("dark");
setKeybindings(new KeybindingsManager());

// Keep the OSC 8 wrappers visible but readable: \x1b]8;;URL\x07TEXT\x1b]8;;\x07
const strip = (s) =>
	s
		.replace(/\x1b\[[0-9;]*m/g, "")
		.replace(/\x1b\]8;;([^\x07]*)\x07/g, (_m, url) => (url ? `<link ${url}>` : "</link>"));

const events = [];
const tui = { requestRender: () => events.push(["requestRender"]) };

function dialog(providerId, nameOverride, titleOverride) {
	events.length = 0;
	return new LoginDialogComponent(
		tui,
		providerId,
		(success, message) => events.push(["onComplete", success, message]),
		nameOverride,
		titleOverride,
	);
}

function show(title, component) {
	console.log(`--- ${title} ---`);
	for (const line of component.render(60)) console.log(JSON.stringify(strip(line)));
	if (events.length) {
		console.log("events:");
		for (const event of events) console.log(`  ${JSON.stringify(event)}`);
		events.length = 0;
	}
}

// --- Case 1: title, provider name override -----------------------------------
{
	show("case1 default title", dialog("anthropic"));
	show("case1 name override", dialog("anthropic", "Anthropic Pro"));
	show("case1 title override", dialog("anthropic", "Anthropic Pro", "Sign in"));
}

// --- Case 2: showAuth with and without instructions --------------------------
{
	const component = dialog("anthropic");
	component.showAuth("https://example.com/auth?code=1");
	show("case2 auth", component);
	component.showAuth("https://example.com/auth?code=1", "Approve in the browser");
	show("case2 auth with instructions", component);
}

// --- Case 3: device code -----------------------------------------------------
{
	const component = dialog("copilot");
	component.showDeviceCode({ verificationUri: "https://github.com/login/device", userCode: "ABCD-1234" });
	show("case3 device code", component);
}

// --- Case 4: prompt, submit --------------------------------------------------
{
	const component = dialog("anthropic");
	const promise = component.showPrompt("Paste the code", "abc123");
	promise.then((value) => events.push(["resolved", value]));
	show("case4 prompt", component);
	for (const character of "xyz") component.handleInput(character);
	component.handleInput("\r");
	await new Promise((resolve) => setImmediate(resolve));
	show("case4 submitted", component);
}

// --- Case 5: manual input, cancel --------------------------------------------
{
	const component = dialog("anthropic");
	const promise = component.showManualInput("Paste the redirect URL");
	promise.catch((error) => events.push(["rejected", error.message]));
	show("case5 manual input", component);
	component.handleInput("\x1b");
	await new Promise((resolve) => setImmediate(resolve));
	console.log("aborted:", component.signal.aborted);
	show("case5 cancelled", component);
}

// --- Case 6: info, waiting, progress, details --------------------------------
{
	const component = dialog("anthropic");
	component.showInfo("Use the web console", [{ url: "https://example.com", label: "Console" }, { url: "https://example.org" }], true);
	show("case6 info", component);

	const waiting = dialog("anthropic");
	waiting.showWaiting("Waiting for approval…");
	waiting.showProgress("Polling…");
	show("case6 waiting and progress", waiting);

	const details = dialog("anthropic");
	details.showDetails(["First line", "Second line"]);
	show("case6 details", details);
}
