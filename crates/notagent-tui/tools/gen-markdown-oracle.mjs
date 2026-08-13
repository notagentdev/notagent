#!/usr/bin/env node
// Generates `tests/fixtures/markdown-oracle.json`: the marked token stream for
// every Markdown source used by the ported test suite.
//
// The Rust lexer must reproduce this stream exactly; the fixture is the oracle
// for `tests/markdown_oracle.rs`.

import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { Marked, Tokenizer } from "/Users/dev/projects/notagent-main/node_modules/marked/lib/marked.esm.js";
const OUTPUT = new URL("../tests/fixtures/markdown-oracle.json", import.meta.url);

const STRICT_STRIKETHROUGH_REGEX = /^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)/;

class StrictStrikethroughTokenizer extends Tokenizer {
	del(src) {
		const match = STRICT_STRIKETHROUGH_REGEX.exec(src);
		if (!match) return undefined;
		const text = match[2];
		return { type: "del", raw: match[0], text, tokens: this.lexer.inlineTokens(text) };
	}
}

function isEscaped(source, index) {
	let backslashes = 0;
	for (let position = index - 1; position >= 0 && source[position] === "\\"; position--) backslashes++;
	return backslashes % 2 === 1;
}

function findClosingDelimiter(source, closing, start) {
	let index = source.indexOf(closing, start);
	while (index >= 0 && isEscaped(source, index)) index = source.indexOf(closing, index + closing.length);
	return index;
}

function looksLikePendingDollarMath(source) {
	return /\\[A-Za-z]+|[_^=+*/<>()[\]|±≤≥≠≈∈→⇒∞∫∑√-]/.test(source);
}

function tokenizeInlineLatex(source) {
	let opening = "";
	let closing = "";
	if (source.startsWith("$$")) {
		opening = "$$";
		closing = "$$";
	} else if (source.startsWith("\\(")) {
		opening = "\\(";
		closing = "\\)";
	} else if (source.startsWith("\\[")) {
		opening = "\\[";
		closing = "\\]";
	} else if (source.startsWith("$") && !/^\$\s/.test(source)) {
		opening = "$";
		closing = "$";
	} else {
		return undefined;
	}

	const closingIndex = findClosingDelimiter(source, closing, opening.length);
	if (
		closingIndex >= 0 &&
		opening === "$" &&
		(/\s$/.test(source.slice(opening.length, closingIndex)) ||
			/^\d/.test(source.slice(closingIndex + 1)) ||
			(/^[A-Z_][A-Z0-9_]*(?:[^A-Za-z0-9_\s])?$/.test(source.slice(opening.length, closingIndex)) &&
				/^[A-Za-z_][A-Za-z0-9_]*/.test(source.slice(closingIndex + 1))) ||
			source.slice(opening.length, closingIndex).includes("`"))
	) {
		return undefined;
	}

	if (closingIndex < 0) {
		const pendingSource = source.slice(opening.length);
		if (opening.startsWith("\\") || looksLikePendingDollarMath(pendingSource)) {
			return { type: "latex", raw: source, text: pendingSource, pending: true };
		}
		return undefined;
	}

	const text = source.slice(opening.length, closingIndex);
	if (!text || text.includes("\n")) return undefined;
	return { type: "latex", raw: source.slice(0, closingIndex + closing.length), text };
}

function tokenizeBlockLatex(source) {
	const dollarMatch = /^ {0,3}\$\$[ \t]*(?:\n)?([\s\S]*?)\$\$[ \t]*(?:\n|$)/.exec(source);
	if (dollarMatch?.[1]) return { type: "latexBlock", raw: dollarMatch[0], text: dollarMatch[1].trim() };

	const bracketMatch = /^ {0,3}\\\[[ \t]*(?:\n)?([\s\S]*?)\\\][ \t]*(?:\n|$)/.exec(source);
	if (bracketMatch?.[1]) return { type: "latexBlock", raw: bracketMatch[0], text: bracketMatch[1].trim() };

	const pendingBracket = /^ {0,3}\\\[[ \t]*(?:\n)?([\s\S]*)$/.exec(source);
	if (pendingBracket) return { type: "latexBlock", raw: pendingBracket[0], text: pendingBracket[1], pending: true };

	const pendingDollar = /^ {0,3}\$\$[ \t]*(?:\n)?([\s\S]*)$/.exec(source);
	if (pendingDollar?.[1] && looksLikePendingDollarMath(pendingDollar[1]))
		return { type: "latexBlock", raw: pendingDollar[0], text: pendingDollar[1], pending: true };
	return undefined;
}

const parser = new Marked();
parser.setOptions({ tokenizer: new StrictStrikethroughTokenizer() });
parser.use({
	extensions: [
		{
			name: "latexBlock",
			level: "block",
			start(source) {
				const match = /(?:^|\n) {0,3}(?:\$\$|\\\[)/.exec(source);
				return match ? match.index + (match[0].startsWith("\n") ? 1 : 0) : undefined;
			},
			tokenizer: tokenizeBlockLatex,
		},
		{
			name: "latex",
			level: "inline",
			start(source) {
				const indices = [source.indexOf("$"), source.indexOf("\\("), source.indexOf("\\[")].filter((i) => i >= 0);
				return indices.length > 0 ? Math.min(...indices) : undefined;
			},
			tokenizer: tokenizeInlineLatex,
		},
	],
});

/** Reduce a marked token to the fields the renderer reads. */
function simplify(token) {
	const result = { type: token.type, raw: token.raw };
	for (const key of ["text", "depth", "lang", "href", "ordered", "start", "loose", "task", "checked", "pending"]) {
		if (token[key] !== undefined) result[key] = token[key];
	}
	if (token.tokens) result.tokens = token.tokens.map(simplify);
	if (token.items) result.items = token.items.map(simplify);
	if (token.header) result.header = token.header.map((cell) => ({ text: cell.text, tokens: cell.tokens.map(simplify) }));
	if (token.rows)
		result.rows = token.rows.map((row) => row.map((cell) => ({ text: cell.text, tokens: cell.tokens.map(simplify) })));
	return result;
}

// The sources come straight out of the ported test suite.
const inputs = JSON.parse(
	execFileSync(process.execPath, [new URL("extract-markdown-inputs.mjs", import.meta.url).pathname, "-"], {
		encoding: "utf8",
	}),
);
const cases = inputs.map((source) => ({
	source,
	tokens: parser.lexer(source.replace(/\t/g, "   ")).map(simplify),
}));

writeFileSync(OUTPUT, JSON.stringify(cases, null, "\t"));
console.log(`wrote ${cases.length} cases`);
