/**
 * Erzeugt `tests/fixtures/utils-oracle.json`: Erwartungswerte der TS-Vorlage
 * für `visibleWidth`, `wrapTextWithAnsi` und `truncateToWidth` über ein breites
 * Korpus. Der Rust-Port wird in `tests/utils_oracle.rs` dagegen geprüft.
 *
 * Master-Plan, Risiko 1 (Breiten-Details): "differierende Einzelfälle gegen
 * Node-REPL-Ausgabe der TS-Funktionen fixieren (Fixture-Generierung aus dem
 * TS-Repo)".
 *
 * Aufruf (aus crates/notagent-tui/):
 *   node tools/gen-utils-oracle.mjs > tests/fixtures/utils-oracle.json
 */
import fs from "node:fs";

const TS_REPO = process.env.NOTAGENT_TS_REPO ?? "/Users/dev/projects/notagent-main";
const utils = await import(`${TS_REPO}/packages/tui/src/utils.ts`);
const { visibleWidth, wrapTextWithAnsi, truncateToWidth, sliceWithWidth, extractSegments } = utils;

const corpus = [];
const add = (s) => {
	if (s.length > 0 && !corpus.includes(s)) corpus.push(s);
};

// 1. Handverlesene Klassen
for (const s of [
	"hello world",
	"a\tb\tc",
	"\t",
	"   ",
	"\x1b[31mred\x1b[0m",
	"\x1b[1;4;38;5;240mstyled\x1b[0m",
	"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
	"\x1b]8;;https://example.com\x07link\x1b]8;;\x07",
	"\x1b_pi:c\x07cursor",
	"\x1b]133;A\x07prompt\x1b]133;B\x07",
	"abc\x1bnot-ansi",
	"中文汉字测试段落内容",
	"ネットワークが",
	"한국어 텍스트",
	"नेटवर्क",
	"सर्वाधिकार सुरक्षित। ऑर्डर पर क्लिक करें",
	"ကာကေက်ကျကြကဳကဴကဵကးကိုက္",
	"กำ ก ำ ກຳ",
	"é čřžůú שָׁ بّ རྐ ᜠ᜴ 가〮 가〯",
	"🙂界🙂界🙂界",
	"👨‍👩‍👧‍👦 family",
	"🏳️‍🌈🏳️‍⚧️",
	"❤️ ❤ ‼️ ↔️ ©️ ®️ ™️ 1️⃣ #️⃣",
	"🇨 🇨🇳 🇯🇵🇺🇸",
	"👍🏻👍🏼👍🏽👍🏾👍🏿",
	"mixed 中文 with ASCII and 🙂 emoji",
	"a".repeat(200),
	"word ".repeat(40),
	"\x1b[44m" + "bg ".repeat(30) + "\x1b[0m",
	"ﬀﬁﬂ ＡＢＣ ｱｲｳ",
	"​‌‍﻿",
	"line1\nline2\r\nline3\rline4",
]) {
	add(s);
}

// 2. Alle RGI-ZWJ-Sequenzen und Basis-Emoji aus der lokalen Node-Runtime
const RGI = /^\p{RGI_Emoji}$/v;
let emojiCount = 0;
for (let cp = 0; cp <= 0x10ffff && emojiCount < 400; cp++) {
	if (cp >= 0xd800 && cp <= 0xdfff) continue;
	const s = String.fromCodePoint(cp);
	if (RGI.test(s)) {
		add(s);
		add(`x${s}y`);
		emojiCount++;
	}
}
const zwjData = process.env.NOTAGENT_ZWJ_DATA;
if (zwjData && fs.existsSync(zwjData)) {
	for (const line of fs.readFileSync(zwjData, "utf8").split("\n")) {
		const body = line.split("#")[0].trim();
		if (!body) continue;
		const fields = body.split(";");
		if (fields.length < 2) continue;
		const seq = fields[0]
			.trim()
			.split(/\s+/)
			.map((h) => String.fromCodePoint(parseInt(h, 16)))
			.join("");
		add(seq);
	}
}

// 3. Deterministisches Pseudozufalls-Korpus über gemischte Alphabete
const ALPHABET = [
	..."abcdefgh ",
	"\t",
	"中",
	"界",
	"あ",
	"한",
	"🙂",
	"👍🏻",
	"❤️",
	"🇨",
	"े",
	"ำ",
	"́",
	"\x1b[31m",
	"\x1b[0m",
	"\x1b[4m",
	"\x1b]8;;https://e.co\x07",
	"\x1b]8;;\x07",
];
let seed = 0x12345678;
const rnd = () => {
	seed ^= seed << 13;
	seed ^= seed >>> 17;
	seed ^= seed << 5;
	seed >>>= 0;
	return seed / 0x1_0000_0000;
};
for (let i = 0; i < 400; i++) {
	const len = 1 + Math.floor(rnd() * 24);
	let s = "";
	for (let j = 0; j < len; j++) s += ALPHABET[Math.floor(rnd() * ALPHABET.length)];
	add(s);
}

const WIDTHS = [1, 2, 5, 10, 40];
// (startCol, length, strict) für sliceWithWidth und (beforeEnd, afterStart, afterLen, strictAfter)
// für extractSegments — die Kombinationen, die die Overlay-Komposition benutzt.
const SLICES = [
	[0, 10, false],
	[0, 10, true],
	[3, 5, true],
	[5, 4, true],
	[2, 20, false],
];
const SEGMENTS = [
	[5, 9, 11, true],
	[4, 8, 4, true],
	[0, 4, 6, false],
	[10, 13, 10, true],
];
const cases = corpus.map((input) => ({
	input,
	visibleWidth: visibleWidth(input),
	wrapped: WIDTHS.map((w) => wrapTextWithAnsi(input, w)),
	truncated: WIDTHS.map((w) => truncateToWidth(input, w)),
	truncatedEllipsisPad: WIDTHS.map((w) => truncateToWidth(input, w, "…", true)),
	slices: SLICES.map(([start, len, strict]) => sliceWithWidth(input, start, len, strict)),
	segments: SEGMENTS.map(([beforeEnd, afterStart, afterLen, strictAfter]) =>
		extractSegments(input, beforeEnd, afterStart, afterLen, strictAfter),
	),
}));

process.stdout.write(JSON.stringify({ widths: WIDTHS, slices: SLICES, segments: SEGMENTS, cases }, null, 0));
process.stderr.write(`oracle cases: ${cases.length}\n`);
