// Oracle for crates/notagent/tests/settings_selector.rs — drives the TypeScript
// SettingsSelectorComponent and prints the rendered lines plus every callback call.
//
// Run from packages/coding-agent of the TypeScript repo:
//   node tools/gen-settings-selector-oracle.mjs
import { setKeybindings } from "@notagent/tui";
import { KeybindingsManager } from "./src/core/keybindings.ts";
import { SettingsSelectorComponent } from "./src/modes/interactive/components/settings-selector.ts";
import { initTheme } from "./src/modes/interactive/theme/theme.ts";

initTheme("dark");
setKeybindings(new KeybindingsManager());

const strip = (s) => s.replace(/\x1b\[[0-9;]*m/g, "").replace(/\x1b\][^\x07]*\x07/g, "");
const ENTER = "\r";

function harness(config) {
	const calls = [];
	const record = (name) => (value) => calls.push([name, value]);
	const callbacks = new Proxy(
		{},
		{
			get(target, prop) {
				if (prop === "onThemePreview") return record("onThemePreview");
				return record(String(prop));
			},
		},
	);
	const component = new SettingsSelectorComponent(
		{
			autoCompact: true,
			showImages: false,
			imageWidthCells: 80,
			autoResizeImages: true,
			blockImages: false,
			enableSkillCommands: true,
			steeringMode: "one-at-a-time",
			followUpMode: "all",
			transport: "auto",
			httpIdleTimeoutMs: 300000,
			thinkingLevel: "medium",
			availableThinkingLevels: ["off", "low", "medium", "high"],
			currentTheme: "dark",
			terminalTheme: "dark",
			availableThemes: ["dark", "light", "solarized"],
			hideThinkingBlock: false,
			mermaidRenderingMode: "streaming",
			showCacheMissNotices: true,
			enableInstallTelemetry: false,
			doubleEscapeAction: "tree",
			treeFilterMode: "default",
			showHardwareCursor: false,
			editorPaddingX: 1,
			outputPad: 0,
			autocompleteMaxVisible: 10,
			quietStartup: false,
			defaultProjectTrust: "ask",
			clearOnShrink: true,
			showTerminalProgress: true,
			tuiMode: "regular",
			fullscreenExitOutput: "transcript",
			fullscreenScrollbar: "auto",
			warnings: {},
			...config,
		},
		callbacks,
	);
	return { component, list: component.getSettingsList(), calls };
}

function show(title, h) {
	console.log(`--- ${title} ---`);
	for (const line of h.component.render(80)) console.log(JSON.stringify(strip(line)));
	if (h.calls.length) {
		console.log("calls:");
		for (const [name, value] of h.calls) console.log(`  ${name} ${JSON.stringify(value)}`);
		h.calls.length = 0;
	}
}

function search(h, label) {
	for (const character of label) h.list.handleInput(character);
}

// --- Case 1: the TS suite's fullscreen cycles --------------------------------
{
	const h = harness({});
	search(h, "Fullscreen exit output");
	h.list.handleInput(ENTER);
	h.list.handleInput(ENTER);
	show("case1 fullscreen exit output cycled twice", h);
}

// --- Case 2: value row rendering and the http idle timeout cycle -------------
{
	const h = harness({});
	search(h, "HTTP idle timeout");
	show("case2 http idle row", h);
	h.list.handleInput(ENTER);
	show("case2 after one cycle", h);
}

// --- Case 3: warnings submenu ------------------------------------------------
{
	const h = harness({});
	search(h, "Warnings");
	h.list.handleInput(ENTER);
	show("case3 warnings submenu", h);
	h.list.handleInput(ENTER);
	show("case3 warning toggled", h);
	h.list.handleInput("\x1b");
	show("case3 back in the main list", h);
}

// --- Case 4: thinking submenu ------------------------------------------------
{
	const h = harness({});
	search(h, "Thinking level");
	h.list.handleInput(ENTER);
	show("case4 thinking submenu", h);
	h.list.handleInput("\x1b[B");
	h.list.handleInput(ENTER);
	show("case4 level selected", h);
}

// --- Case 5: theme submenu, single mode selects a theme ---------------------
{
	const h = harness({});
	search(h, "Theme");
	h.list.handleInput(ENTER);
	show("case5 theme submenu (single)", h);
	h.list.handleInput("\x1b[B");
	show("case5 selection moved", h);
	h.list.handleInput(ENTER);
	show("case5 theme selected", h);
}

// --- Case 6: theme submenu cancel restores the original preview --------------
{
	const h = harness({});
	search(h, "Theme");
	h.list.handleInput(ENTER);
	h.list.handleInput("\x1b");
	show("case6 cancelled", h);
}

// --- Case 7: single -> automatic -> light select -> apply --------------------
{
	const h = harness({});
	search(h, "Theme");
	h.list.handleInput(ENTER);
	h.list.handleInput("\x1b[A");
	show("case7 automatic row selected", h);
	h.list.handleInput(ENTER);
	show("case7 automatic menu", h);
	h.list.handleInput(ENTER);
	show("case7 light theme select", h);
	h.list.handleInput("\x1b[B");
	h.list.handleInput("\x1b[B");
	h.list.handleInput(ENTER);
	show("case7 light theme chosen", h);
	h.list.handleInput("\x1b[B");
	h.list.handleInput("\x1b[B");
	h.list.handleInput(ENTER);
	show("case7 applied", h);
}

// --- Case 8: automatic menu, switch back to single mode ---------------------
{
	const h = harness({ currentTheme: "light/solarized" });
	search(h, "Theme");
	h.list.handleInput(ENTER);
	show("case8 automatic menu from an auto setting", h);
	h.list.handleInput("\x1b[B");
	h.list.handleInput("\x1b[B");
	h.list.handleInput("\x1b[B");
	h.list.handleInput(ENTER);
	show("case8 switched to single", h);
}
