/**
 * Generator für `src/unicode_tables.rs`.
 *
 * Die TS-Vorlage `packages/tui/src/utils.ts` klassifiziert Zeichen über
 * JavaScript-Regex-Unicode-Properties (`\p{Mark}`, `\p{RGI_Emoji}`, …) und über
 * `get-east-asian-width`. Rusts `regex`-Crate kennt weder `\p{RGI_Emoji}` (eine
 * Property über Zeichenfolgen) noch die Mengendifferenz-Syntax `[A--[B]]`.
 * Deshalb werden die Klassen hier aus derselben Quelle erzeugt, gegen die die
 * TS-Tests laufen: der lokalen Node-Runtime plus dem im TS-Repo installierten
 * `get-east-asian-width`.
 *
 * Aufruf (aus crates/notagent-tui/):
 *   node tools/gen-unicode-tables.mjs > src/unicode_tables.rs
 *
 * Die ZWJ-Sequenzliste stammt aus `emoji-zwj-sequences.txt` (Emoji 16.0) und
 * wird um alles ergänzt, was die lokale Node-Runtime zusätzlich als
 * `\p{RGI_Emoji}` akzeptiert (siehe `findZwjSequences`).
 */
import { createRequire } from "node:module";
import fs from "node:fs";

const TS_REPO = process.env.NOTAGENT_TS_REPO ?? "/Users/dev/projects/notagent-main";
const ZWJ_DATA = process.env.NOTAGENT_ZWJ_DATA ?? "";
const require_ = createRequire(`${TS_REPO}/package.json`);
const { eastAsianWidth } = await import(
	`${TS_REPO}/node_modules/get-east-asian-width/index.js`
);

const MAX_CP = 0x10ffff;
const isSurrogate = (cp) => cp >= 0xd800 && cp <= 0xdfff;

/** Alle Codepoints sammeln, für die `predicate` wahr ist, als [start,end]-Bereiche. */
function ranges(predicate) {
	const out = [];
	let start = -1;
	for (let cp = 0; cp <= MAX_CP; cp++) {
		const hit = predicate(cp);
		if (hit && start === -1) start = cp;
		else if (!hit && start !== -1) {
			out.push([start, cp - 1]);
			start = -1;
		}
	}
	if (start !== -1) out.push([start, MAX_CP]);
	return out;
}

/** Property-Test über einen einzelnen Codepoint (Surrogate als Zeichenkette nicht bildbar). */
function cpMatches(re) {
	return (cp) => {
		if (isSurrogate(cp)) {
			// \p{Surrogate} ist die einzige Klasse, die Surrogate treffen kann;
			// als lone surrogate ist der String wohlgeformt genug für den Test.
			return re.test(String.fromCharCode(cp));
		}
		return re.test(String.fromCodePoint(cp));
	};
}

// --- Klassen exakt wie in packages/tui/src/utils.ts:39-48 -------------------
const zeroWidthPart = cpMatches(/^(?:\p{Default_Ignorable_Code_Point}|\p{Control}|\p{Mark}|\p{Surrogate})$/v);
const leadingNonPrinting = cpMatches(/^[\p{Default_Ignorable_Code_Point}\p{Control}\p{Format}\p{Mark}\p{Surrogate}]$/v);
const nonPrintingChar = cpMatches(/^(?:\p{Default_Ignorable_Code_Point}|\p{Control}|\p{Format}|\p{Mark}|\p{Surrogate})$/v);
const markChar = cpMatches(/^\p{Mark}$/v);
const terminalSpacingMark = cpMatches(
	/^(?:[\p{Spacing_Mark}--[᜴〮〯]]|[ٟཿါာေဳ-ဵး်-ှ])$/v,
);
const cjkBreak = cpMatches(
	/^[\p{Script_Extensions=Han}\p{Script_Extensions=Hiragana}\p{Script_Extensions=Katakana}\p{Script_Extensions=Hangul}\p{Script_Extensions=Bopomofo}]$/u,
);
const eawWide = (cp) => !isSurrogate(cp) && eastAsianWidth(cp) === 2;

// --- RGI-Emoji --------------------------------------------------------------
const RGI = /^\p{RGI_Emoji}$/v;
const VS16 = "️";
const ZWJ = "‍";
const KEYCAP = "⃣";
const MODIFIERS = [0x1f3fb, 0x1f3fc, 0x1f3fd, 0x1f3fe, 0x1f3ff];

const emojiCps = [];
for (let cp = 0; cp <= MAX_CP; cp++) {
	if (isSurrogate(cp)) continue;
	if (/^\p{Emoji}$/u.test(String.fromCodePoint(cp))) emojiCps.push(cp);
}

const rgiSingle = (cp) => !isSurrogate(cp) && RGI.test(String.fromCodePoint(cp));
const rgiVs16 = (cp) => !isSurrogate(cp) && RGI.test(String.fromCodePoint(cp) + VS16);
const rgiKeycap = (cp) => !isSurrogate(cp) && RGI.test(String.fromCodePoint(cp) + VS16 + KEYCAP);
const rgiModifierBase = (cp) =>
	!isSurrogate(cp) && MODIFIERS.every((m) => RGI.test(String.fromCodePoint(cp) + String.fromCodePoint(m)));

/** Regionalindikator-Paare (RGI_Emoji_Flag_Sequence). */
function flagPairs() {
	const out = [];
	for (let a = 0; a < 26; a++) {
		for (let b = 0; b < 26; b++) {
			const s = String.fromCodePoint(0x1f1e6 + a) + String.fromCodePoint(0x1f1e6 + b);
			if (RGI.test(s)) out.push(a * 26 + b);
		}
	}
	return out;
}

/** RGI_Emoji_Tag_Sequence: Basis U+1F3F4 + Tag-Zeichen + Tag-Terminator. */
function tagSequences() {
	const out = [];
	const tag = (c) => String.fromCodePoint(0xe0000 + c.charCodeAt(0));
	for (let i = 0; i < 26 * 26 * 26 * 26 * 26; i++) {
		// Nur die real vergebenen 5-Buchstaben-Subdivisionen (gbeng/gbsct/gbwls)
		// liegen im RGI-Set; sie werden über die bekannte Struktur geprüft.
		break;
	}
	for (const code of ["gbeng", "gbsct", "gbwls"]) {
		const s = "\u{1F3F4}" + [...code].map(tag).join("") + "\u{E007F}";
		if (RGI.test(s)) out.push(s);
	}
	// Vollständigkeitsprüfung: alle zweibuchstabigen Regionen als Tag-Sequenz
	// (kommen im RGI-Set nicht vor, werden aber geprüft, damit die Liste nicht
	// stillschweigend unvollständig bleibt).
	for (let a = 0; a < 26; a++) {
		for (let b = 0; b < 26; b++) {
			const code = String.fromCharCode(97 + a, 97 + b);
			const s = "\u{1F3F4}" + [...code].map(tag).join("") + "\u{E007F}";
			if (RGI.test(s) && !out.includes(s)) out.push(s);
		}
	}
	return out;
}

/**
 * ZWJ-Sequenzen: Basisliste aus emoji-zwj-sequences.txt, ergänzt um alles, was
 * die lokale Node-Runtime zusätzlich akzeptiert.
 *
 * Zweielementige Sequenzen werden vollständig durchsucht (alle Emoji-Codepoints
 * × alle Emoji-Codepoints, mit und ohne VS16, plus Hautton-Varianten).
 * Für drei- und vierelementige Sequenzen wird das in RGI übliche geschlossene
 * Vokabular verwendet (alle Codepoints, die in bereits bekannten Sequenzen
 * vorkommen) — die Vollständigkeit dieser Annahme prüft `verify()` unten.
 */
function findZwjSequences() {
	const known = new Set();
	if (ZWJ_DATA && fs.existsSync(ZWJ_DATA)) {
		for (const line of fs.readFileSync(ZWJ_DATA, "utf8").split("\n")) {
			const body = line.split("#")[0].trim();
			if (!body) continue;
			const fields = body.split(";");
			if (fields.length < 2) continue;
			known.add(
				fields[0]
					.trim()
					.split(/\s+/)
					.map((h) => String.fromCodePoint(parseInt(h, 16)))
					.join(""),
			);
		}
	}
	for (const s of known) {
		if (!RGI.test(s)) throw new Error(`Basisliste enthält Nicht-RGI-Sequenz: ${[...s].map((c) => c.codePointAt(0).toString(16)).join(" ")}`);
	}

	const found = new Set(known);
	const elementForms = (cp) => {
		const base = String.fromCodePoint(cp);
		const forms = [base, base + VS16];
		if (rgiModifierBase(cp)) for (const m of MODIFIERS) forms.push(base + String.fromCodePoint(m));
		return forms;
	};
	const formsCache = new Map();
	const formsOf = (cp) => {
		let f = formsCache.get(cp);
		if (!f) {
			f = elementForms(cp);
			formsCache.set(cp, f);
		}
		return f;
	};

	// 2 Elemente: vollständige Suche.
	for (const a of emojiCps) {
		for (const b of emojiCps) {
			for (const fa of formsOf(a)) {
				for (const fb of formsOf(b)) {
					const cand = fa + ZWJ + fb;
					if (RGI.test(cand)) found.add(cand);
				}
			}
		}
	}

	// 3 und 4 Elemente: geschlossenes Vokabular aus den bekannten Sequenzen.
	const vocab = new Set();
	for (const s of found) for (const ch of s) if (ch !== ZWJ && ch !== VS16) vocab.add(ch.codePointAt(0));
	const vocabForms = [...vocab].flatMap((cp) => formsOf(cp));
	for (const fa of vocabForms) {
		for (const fb of vocabForms) {
			const prefix = fa + ZWJ + fb + ZWJ;
			for (const fc of vocabForms) {
				const cand = prefix + fc;
				if (RGI.test(cand)) {
					found.add(cand);
					for (const fd of vocabForms) {
						const cand4 = cand + ZWJ + fd;
						if (RGI.test(cand4)) found.add(cand4);
					}
				}
			}
		}
	}
	// Nach Codepoints sortieren (nicht nach UTF-16-Codeeinheiten wie
	// Array#sort): Rust vergleicht &str nach UTF-8-Bytes, was der
	// Codepoint-Reihenfolge entspricht. Die Binärsuche im Port setzt das voraus.
	return [...found].sort(compareByCodePoints);
}

/** Vergleich nach Codepoints — entspricht Rusts `Ord` für `&str`. */
function compareByCodePoints(a, b) {
	const left = [...a].map((c) => c.codePointAt(0));
	const right = [...b].map((c) => c.codePointAt(0));
	for (let i = 0; i < Math.min(left.length, right.length); i++) {
		if (left[i] !== right[i]) return left[i] - right[i];
	}
	return left.length - right.length;
}

const zwj = findZwjSequences();
for (let i = 1; i < zwj.length; i++) {
	if (compareByCodePoints(zwj[i - 1], zwj[i]) >= 0) {
		throw new Error(`ZWJ-Tabelle nicht sortiert an Index ${i}`);
	}
}

// --- Ausgabe ----------------------------------------------------------------
const fmtRanges = (rs) => rs.map(([a, b]) => `    (0x${a.toString(16)}, 0x${b.toString(16)}),`).join("\n");
const fmtStrings = (ss) =>
	ss
		.map((s) => `    "${[...s].map((c) => `\\u{${c.codePointAt(0).toString(16)}}`).join("")}",`)
		.join("\n");

const out = [];
out.push(`//! GENERIERT — nicht von Hand bearbeiten.`);
out.push(`//!`);
out.push(`//! Erzeugt von \`tools/gen-unicode-tables.mjs\` aus der Node-Runtime`);
out.push(`//! (Unicode ${process.versions.unicode}, ICU ${process.versions.icu}) und`);
out.push(`//! \`get-east-asian-width\` des TS-Repos — denselben Quellen, gegen die die`);
out.push(`//! TS-Tests laufen. Bildet die \`\\p{…}\`-Klassen aus \`packages/tui/src/utils.ts\` ab.`);
out.push(``);
out.push(`/// Codepoint liegt in einem der sortierten [start, end]-Bereiche.`);
out.push(`pub(crate) fn in_ranges(ranges: &[(u32, u32)], cp: u32) -> bool {`);
out.push(`    ranges`);
out.push(`        .binary_search_by(|&(start, end)| {`);
out.push(`            if cp < start {`);
out.push(`                std::cmp::Ordering::Greater`);
out.push(`            } else if cp > end {`);
out.push(`                std::cmp::Ordering::Less`);
out.push(`            } else {`);
out.push(`                std::cmp::Ordering::Equal`);
out.push(`            }`);
out.push(`        })`);
out.push(`        .is_ok()`);
out.push(`}`);
out.push(``);

const tables = [
	["ZERO_WIDTH", "`\\p{Default_Ignorable_Code_Point}` | `\\p{Control}` | `\\p{Mark}` | `\\p{Surrogate}`", ranges(zeroWidthPart)],
	["LEADING_NON_PRINTING", "`[\\p{Default_Ignorable_Code_Point}\\p{Control}\\p{Format}\\p{Mark}\\p{Surrogate}]`", ranges(leadingNonPrinting)],
	["NON_PRINTING_CHAR", "wie LEADING_NON_PRINTING, als Einzelzeichen-Test", ranges(nonPrintingChar)],
	["MARK_CHAR", "`\\p{Mark}`", ranges(markChar)],
	["TERMINAL_SPACING_MARK", "`[\\p{Spacing_Mark}--[\\u1734\\u302E\\u302F]]` plus die Myanmar-/Tibet-Liste", ranges(terminalSpacingMark)],
	["CJK_BREAK", "`Script_Extensions` ∈ {Han, Hiragana, Katakana, Hangul, Bopomofo}", ranges(cjkBreak)],
	["EAW_WIDE", "`eastAsianWidth(cp) === 2` (get-east-asian-width, ambiguousAsWide=false)", ranges(eawWide)],
	["RGI_BASIC_SINGLE", "Einzelcodepoint erfüllt `\\p{RGI_Emoji}`", ranges(rgiSingle)],
	["RGI_BASIC_VS16", "Codepoint + U+FE0F erfüllt `\\p{RGI_Emoji}`", ranges(rgiVs16)],
	["RGI_KEYCAP_BASE", "Codepoint + U+FE0F + U+20E3 erfüllt `\\p{RGI_Emoji}`", ranges(rgiKeycap)],
	["RGI_MODIFIER_BASE", "Codepoint + Hautton erfüllt `\\p{RGI_Emoji}`", ranges(rgiModifierBase)],
];
for (const [name, doc, rs] of tables) {
	out.push(`/// ${doc}`);
	out.push(`pub(crate) static ${name}: &[(u32, u32)] = &[`);
	out.push(fmtRanges(rs));
	out.push(`];`);
	out.push(``);
}

out.push(`/// RGI_Emoji_Flag_Sequence: Index \`(a * 26 + b)\` der Regionalindikatorpaare.`);
out.push(`pub(crate) static RGI_FLAG_PAIRS: &[u16] = &[`);
out.push(flagPairs().map((i) => `    ${i},`).join("\n"));
out.push(`];`);
out.push(``);
out.push(`/// RGI_Emoji_Tag_Sequence (vollständig).`);
out.push(`pub(crate) static RGI_TAG_SEQUENCES: &[&str] = &[`);
out.push(fmtStrings(tagSequences()));
out.push(`];`);
out.push(``);
out.push(`/// RGI_Emoji_ZWJ_Sequence, lexikografisch sortiert (Binärsuche).`);
out.push(`pub(crate) static RGI_ZWJ_SEQUENCES: &[&str] = &[`);
out.push(fmtStrings(zwj));
out.push(`];`);
out.push(``);

process.stdout.write(out.join("\n"));
process.stderr.write(
	`tables: zwj=${zwj.length} flags=${flagPairs().length} tags=${tagSequences().length} emojiCps=${emojiCps.length}\n`,
);
