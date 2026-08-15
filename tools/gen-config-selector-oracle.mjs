// Oracle for crates/notagent/tests/config_selector.rs — drives the TypeScript
// ConfigSelectorComponent with the fixtures of that suite and prints the
// rendered lines plus the settings writes each key produces.
//
// Run from packages/coding-agent of the TypeScript repo:
//   node tools/gen-config-selector-oracle.mjs
import { ConfigSelectorComponent } from "./src/modes/interactive/components/config-selector.ts";
import { initTheme } from "./src/modes/interactive/theme/theme.ts";

initTheme("dark", true);

const strip = (s) => s.replace(/\x1b\[[0-9;]*m/g, "");

const AGENT_DIR = "/home/u/.notagent/agent";
const CWD = "/proj";

function makeSettings(global, project) {
	const writes = [];
	const record = (name, value) => writes.push([name, JSON.parse(JSON.stringify(value))]);
	return {
		writes,
		getGlobalSettings: () => global,
		getProjectSettings: () => project,
		setExtensionPaths: (v) => { global.extensions = v; record("setExtensionPaths", v); },
		setSkillPaths: (v) => { global.skills = v; record("setSkillPaths", v); },
		setPromptTemplatePaths: (v) => { global.prompts = v; record("setPromptTemplatePaths", v); },
		setThemePaths: (v) => { global.themes = v; record("setThemePaths", v); },
		setPackages: (v) => { global.packages = v; record("setPackages", v); },
		setProjectExtensionPaths: (v) => { project.extensions = v; record("setProjectExtensionPaths", v); },
		setProjectSkillPaths: (v) => { project.skills = v; record("setProjectSkillPaths", v); },
		setProjectPromptTemplatePaths: (v) => { project.prompts = v; record("setProjectPromptTemplatePaths", v); },
		setProjectThemePaths: (v) => { project.themes = v; record("setProjectThemePaths", v); },
		setProjectPackages: (v) => { project.packages = v; record("setProjectPackages", v); },
	};
}

const empty = () => ({ extensions: [], skills: [], prompts: [], themes: [] });

function res(path, enabled, source, scope, origin, baseDir) {
	return { path, enabled, metadata: { source, scope, origin, ...(baseDir ? { baseDir } : {}) } };
}

function show(title, selector, settings) {
	console.log(`--- ${title} ---`);
	for (const line of selector.render(80)) console.log(JSON.stringify(strip(line)));
	if (settings?.writes.length) {
		console.log("writes:");
		for (const [name, value] of settings.writes) console.log(`  ${name} ${JSON.stringify(value)}`);
		settings.writes.length = 0;
	}
}

function build(resolvedPaths, settings, writeScope = "global", projectModeAvailable = true) {
	return new ConfigSelectorComponent(
		resolvedPaths, settings, CWD, AGENT_DIR,
		() => console.log("[onClose]"), () => console.log("[onExit]"), () => {},
		24, writeScope, projectModeAvailable,
	);
}

// --- Case 1: grouping, labels, ordering, display names -----------------------
{
	const globalPaths = {
		extensions: [],
		skills: [
			res("/home/u/.notagent/agent/skills/zed/SKILL.md", true, "auto", "user", "top-level"),
			res("/home/u/.notagent/agent/skills/alpha.md", false, "auto", "user", "top-level"),
			res("/pkg/wolf/skills/w.md", true, "wolf-pack", "user", "package", "/pkg/wolf"),
		],
		prompts: [res("/home/u/.notagent/agent/prompts/p.md", true, "auto", "user", "top-level")],
		themes: [res("/pkg/wolf/themes/t.json", false, "wolf-pack", "user", "package", "/pkg/wolf")],
	};
	const projectPaths = {
		extensions: [],
		skills: [res("/proj/.notagent/skills/local.md", true, "auto", "project", "top-level")],
		prompts: [],
		themes: [],
	};
	const settings = makeSettings(empty(), empty());
	const selector = build({ global: globalPaths, project: projectPaths }, settings);
	show("case1 global render", selector, settings);

	const list = selector.getResourceList();
	list.handleInput("\x1b[B"); // down
	show("case1 after down", selector);
	list.handleInput(" ");      // toggle selected (top-level user skill)
	show("case1 after toggle", selector, settings);
}

// --- Case 2: top-level toggle + package toggle with an existing package entry -
{
	const globalPaths = {
		extensions: [],
		skills: [
			res("/home/u/.notagent/agent/skills/alpha.md", true, "auto", "user", "top-level"),
			res("/pkg/wolf/skills/w.md", true, "wolf-pack", "user", "package", "/pkg/wolf"),
		],
		prompts: [], themes: [],
	};
	const settings = makeSettings(
		{ ...empty(), skills: ["skills/alpha.md"], packages: ["wolf-pack"] },
		empty(),
	);
	const selector = build({ global: globalPaths, project: empty() }, settings);
	const list = selector.getResourceList();
	show("case2 initial", selector, settings);
	list.handleInput(" ");           // toggle top-level alpha.md off
	show("case2 toggled top-level off", selector, settings);
	list.handleInput(" ");           // toggle it back on
	show("case2 toggled top-level on", selector, settings);
	list.handleInput("\x1b[B");      // down to the package skill
	list.handleInput(" ");           // disable the package resource
	show("case2 package disabled", selector, settings);
	list.handleInput(" ");           // re-enable -> filter list becomes ["+w.md"]
	show("case2 package enabled", selector, settings);
}

// --- Case 3: project write scope, override cycling ---------------------------
{
	const globalPaths = {
		extensions: [],
		skills: [res("/home/u/.notagent/agent/skills/alpha.md", true, "auto", "user", "top-level")],
		prompts: [], themes: [],
	};
	const projectPaths = {
		extensions: [],
		skills: [
			res("/home/u/.notagent/agent/skills/alpha.md", true, "auto", "user", "top-level"),
			res("/proj/.notagent/skills/local.md", false, "auto", "project", "top-level"),
		],
		prompts: [], themes: [],
	};
	const settings = makeSettings(empty(), empty());
	const selector = build({ global: globalPaths, project: projectPaths }, settings, "project");
	const list = selector.getResourceList();
	show("case3 project initial", selector, settings);
	list.handleInput(" ");        // inherited + enabled -> unload
	show("case3 inherited -> unload", selector, settings);
	list.handleInput(" ");        // unload + inherited enabled -> load
	show("case3 unload -> load", selector, settings);
	list.handleInput(" ");        // load -> inherit
	show("case3 load -> inherit", selector, settings);
	list.handleInput("\x1b[B");
	list.handleInput(" ");        // project-local, not inherited-enabled -> load
	show("case3 project-local -> load", selector, settings);
}

// --- Case 4: tab switches the write scope ------------------------------------
{
	const globalPaths = {
		extensions: [],
		skills: [res("/home/u/.notagent/agent/skills/alpha.md", true, "auto", "user", "top-level")],
		prompts: [], themes: [],
	};
	const settings = makeSettings(empty(), empty());
	const selector = build({ global: globalPaths, project: { ...empty() } }, settings);
	selector.getResourceList().handleInput("\t");
	show("case4 after tab", selector, settings);
	const noSwitch = build({ global: globalPaths, project: { ...empty() } }, settings, "global", false);
	noSwitch.getResourceList().handleInput("\t");
	show("case4 tab without project mode", noSwitch, settings);
}

// --- Case 5: search filter and empty result ----------------------------------
{
	const globalPaths = {
		extensions: [],
		skills: [
			res("/home/u/.notagent/agent/skills/alpha.md", true, "auto", "user", "top-level"),
			res("/home/u/.notagent/agent/skills/beta.md", false, "auto", "user", "top-level"),
		],
		prompts: [res("/pkg/wolf/prompts/alphabet.md", true, "wolf-pack", "user", "package", "/pkg/wolf")],
		themes: [],
	};
	const settings = makeSettings(empty(), empty());
	const selector = build({ global: globalPaths, project: empty() }, settings);
	const list = selector.getResourceList();
	for (const ch of "alpha") list.handleInput(ch);
	show("case5 filtered alpha", selector);
	for (const ch of "zzz") list.handleInput(ch);
	show("case5 no match", selector);
}

// --- Case 6: scroll indicator ------------------------------------------------
{
	const skills = [];
	for (let i = 0; i < 30; i++) {
		skills.push(res(`/home/u/.notagent/agent/skills/s${String(i).padStart(2, "0")}.md`, i % 2 === 0, "auto", "user", "top-level"));
	}
	const settings = makeSettings(empty(), empty());
	const selector = build({ global: { ...empty(), skills }, project: empty() }, settings);
	const list = selector.getResourceList();
	for (let i = 0; i < 12; i++) list.handleInput("\x1b[B");
	show("case6 scrolled", selector);
	list.handleInput("\x1b[6~"); // page down
	show("case6 page down", selector);
}

// --- Case 7: package resource in the project scope ---------------------------
{
	const pkgRes = res("/pkg/wolf/skills/w.md", true, "wolf-pack", "user", "package", "/pkg/wolf");
	const globalPaths = { ...empty(), skills: [pkgRes] };
	const projectPaths = { ...empty(), skills: [pkgRes] };
	const settings = makeSettings({ ...empty(), packages: ["wolf-pack"] }, empty());
	const selector = build({ global: globalPaths, project: projectPaths }, settings, "project");
	const list = selector.getResourceList();
	show("case7 project package initial", selector, settings);
	list.handleInput(" ");   // inherit -> unload, creates the project package entry
	show("case7 -> unload", selector, settings);
	list.handleInput(" ");   // unload -> load
	show("case7 -> load", selector, settings);
	list.handleInput(" ");   // load -> inherit, entry loses its last filter
	show("case7 -> inherit", selector, settings);
}

// --- Case 8: local package source, override entry creation -------------------
{
	const pkgRes = res("/proj/vendor/skills/w.md", true, "./vendor", "user", "package", "/proj/vendor");
	const settings = makeSettings({ ...empty(), packages: ["./vendor"] }, empty());
	const selector = build(
		{ global: { ...empty(), skills: [pkgRes] }, project: { ...empty(), skills: [pkgRes] } },
		settings, "project",
	);
	selector.getResourceList().handleInput(" ");
	show("case8 local package override", selector, settings);
}
