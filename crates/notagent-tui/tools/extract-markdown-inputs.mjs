#!/usr/bin/env node
// Extracts every Markdown source used by `packages/tui/test/markdown.test.ts`
// into a JSON array, so the oracle generator can feed them to marked.

import { readFileSync, writeFileSync } from "node:fs";

const TEST = "/Users/dev/projects/notagent-main/packages/tui/test/markdown.test.ts";
const OUTPUT = process.argv[2];

const source = readFileSync(TEST, "utf8");
const inputs = [];

/** Read the JS string literal that starts at `start` and evaluate it. */
function readLiteral(start, raw = false) {
	const quote = source[start];
	if (quote !== '"' && quote !== "'" && quote !== "`") return null;
	let index = start + 1;
	while (index < source.length) {
		const character = source[index];
		if (character === "\\") {
			index += 2;
			continue;
		}
		if (character === quote) {
			const text = source.slice(start, index + 1);
			try {
				// eslint-disable-next-line no-eval
				return raw ? eval(`String.raw${text}`) : eval(text);
			} catch {
				return null;
			}
		}
		if (quote === "`" && character === "$" && source[index + 1] === "{") return null;
		index++;
	}
	return null;
}

for (const pattern of [
	/new Markdown\(\s*/g,
	/new Markdown\(\s*String\.raw/g,
	/\.setText\(\s*/g,
	/(?:const|let)\s+(?:source|text|input|shellVariables|longText|markdownSource)\s*=\s*(?:String\.raw)?/g,
]) {
	let match = pattern.exec(source);
	while (match) {
		let start = match.index + match[0].length;
		// `String.raw` tags the following template literal.
		if (source.startsWith("String.raw", start)) start += "String.raw".length;
		const value = readLiteral(start, source.slice(match.index, start).includes("String.raw"));
		if (typeof value === "string") inputs.push(value);
		match = pattern.exec(source);
	}
}

const unique = [...new Set(inputs)];
const json = JSON.stringify(unique, null, "\t");
if (OUTPUT === "-" || OUTPUT === undefined) {
	process.stdout.write(json);
} else {
	writeFileSync(OUTPUT, json);
	console.error(`extracted ${unique.length} sources`);
}
