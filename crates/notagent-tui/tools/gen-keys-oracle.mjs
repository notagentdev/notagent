/**
 * Generates `tests/fixtures/keys-oracle.json`: the TS behaviour of
 * `packages/tui/src/keys.ts` for a broad corpus of terminal input sequences and
 * key identifiers, for both Kitty protocol states.
 *
 * Usage (from crates/notagent-tui/):
 *   node tools/gen-keys-oracle.mjs > tests/fixtures/keys-oracle.json
 */
const TS_REPO = process.env.NOTAGENT_TS_REPO ?? "/Users/dev/projects/notagent-main";
const keys = await import(`${TS_REPO}/packages/tui/src/keys.ts`);
const {
	matchesKey,
	parseKey,
	isKeyRelease,
	isKeyRepeat,
	decodeKittyPrintable,
	decodePrintableKey,
	setKittyProtocolActive,
} = keys;

// The Windows Terminal heuristic reads the environment; pin it for the fixture.
delete process.env.WT_SESSION;

// Representative bases per class — the cross product with 13 modifier
// prefixes and every input stays inside the runtime budget of check.sh.
const LETTERS = [..."acdhkpvz"];
const DIGITS = [..."019"];
const SYMBOLS = [..."-[]\\;/_+"];
const SPECIALS = [
	"escape", "esc", "enter", "return", "tab", "space", "backspace", "delete", "insert",
	"clear", "home", "end", "pageUp", "pageDown", "up", "down", "left", "right",
	"f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
];
const MODIFIER_PREFIXES = [
	"", "ctrl+", "shift+", "alt+", "super+",
	"ctrl+shift+", "shift+ctrl+", "ctrl+alt+", "alt+shift+", "ctrl+super+",
	"shift+super+", "ctrl+shift+alt+", "ctrl+shift+super+",
];

const keyIds = [];
for (const prefix of MODIFIER_PREFIXES) {
	for (const base of [...LETTERS, ...DIGITS, ...SYMBOLS, ...SPECIALS]) {
		keyIds.push(prefix + base);
	}
}

const inputs = new Set();
const add = (s) => inputs.add(s);

// Raw control characters and printable ASCII
for (let code = 0; code <= 127; code++) add(String.fromCharCode(code));
// ESC-prefixed legacy forms
for (let code = 0; code <= 127; code++) add(`\x1b${String.fromCharCode(code)}`);
// Legacy sequences from the tables
for (const seq of [
	"\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D", "\x1bOA", "\x1bOB", "\x1bOC", "\x1bOD",
	"\x1b[H", "\x1bOH", "\x1b[1~", "\x1b[7~", "\x1b[F", "\x1bOF", "\x1b[4~", "\x1b[8~",
	"\x1b[2~", "\x1b[3~", "\x1b[5~", "\x1b[[5~", "\x1b[6~", "\x1b[[6~", "\x1b[E", "\x1bOE",
	"\x1bOP", "\x1bOQ", "\x1bOR", "\x1bOS", "\x1b[11~", "\x1b[12~", "\x1b[13~", "\x1b[14~",
	"\x1b[[A", "\x1b[[B", "\x1b[[C", "\x1b[[D", "\x1b[[E", "\x1b[15~", "\x1b[17~", "\x1b[18~",
	"\x1b[19~", "\x1b[20~", "\x1b[21~", "\x1b[23~", "\x1b[24~",
	"\x1b[a", "\x1b[b", "\x1b[c", "\x1b[d", "\x1b[e", "\x1bOa", "\x1bOb", "\x1bOc", "\x1bOd", "\x1bOe",
	"\x1b[2$", "\x1b[3$", "\x1b[5$", "\x1b[6$", "\x1b[7$", "\x1b[8$",
	"\x1b[2^", "\x1b[3^", "\x1b[5^", "\x1b[6^", "\x1b[7^", "\x1b[8^",
	"\x1b[Z", "\x1bOM", "\x1b\r", "\x1b\n", "\x1b\x7f", "\x1b\b", "\x1b ",
	"\x1bb", "\x1bf", "\x1bp", "\x1bn", "\x1bB", "\x1bF",
	"\x1b[200~pasted:3u\x1b[201~", "\x1b[200~90:62:3F:A5\x1b[201~",
]) add(seq);

// Kitty CSI-u across code points, modifiers and event types
const CSI_U_CODEPOINTS = [
	9, 13, 27, 32, 47, 49, 69, 99, 104, 107, 122, 127, 1089, 57399, 57410, 57412, 57414,
	57417, 57421, 57426,
];
for (const cp of CSI_U_CODEPOINTS) {
	for (const mod of [undefined, 1, 2, 3, 5, 6, 7, 9, 13, 14, 17, 65, 129]) {
		for (const event of [undefined, 1, 2, 3]) {
			const modPart = mod === undefined ? "" : `;${mod}`;
			const eventPart = event === undefined ? "" : `:${event}`;
			if (eventPart && !modPart) continue;
			add(`\x1b[${cp}${modPart}${eventPart}u`);
		}
	}
	// alternate-key forms
	add(`\x1b[${cp}::99;5u`);
	add(`\x1b[${cp}:67:99;2u`);
	add(`\x1b[${cp}::${cp};5u`);
}
// Arrows / home / end / functional with modifiers and events
for (const mod of [1, 2, 5, 9]) {
	for (const event of [undefined, 2, 3]) {
		const eventPart = event === undefined ? "" : `:${event}`;
		for (const final of ["A", "B", "C", "D", "H", "F"]) add(`\x1b[1;${mod}${eventPart}${final}`);
		for (const num of [2, 3, 5, 6, 7, 8, 15, 24]) add(`\x1b[${num};${mod}${eventPart}~`);
	}
}
// modifyOtherKeys
for (const cp of [9, 13, 27, 32, 47, 49, 69, 99, 104, 127, 196]) {
	for (const mod of [1, 2, 3, 5, 6, 7, 9]) add(`\x1b[27;${mod};${cp}~`);
}

const inputList = [...inputs];
const snapshot = () =>
	inputList.map((input) => ({
		input,
		parseKey: parseKey(input) ?? null,
		isKeyRelease: isKeyRelease(input),
		isKeyRepeat: isKeyRepeat(input),
		decodeKittyPrintable: decodeKittyPrintable(input) ?? null,
		decodePrintableKey: decodePrintableKey(input) ?? null,
		matches: keyIds.filter((keyId) => matchesKey(input, keyId)),
	}));

setKittyProtocolActive(false);
const legacy = snapshot();
setKittyProtocolActive(true);
const kitty = snapshot();
setKittyProtocolActive(false);

process.stdout.write(JSON.stringify({ keyIds, legacy, kitty }, null, 0));
process.stderr.write(`inputs: ${inputList.length}, keyIds: ${keyIds.length}\n`);
