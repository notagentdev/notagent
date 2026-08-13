#!/usr/bin/env node
// Generates `tests/fixtures/markdown-render-oracle.json`: the exact lines the
// TypeScript `Markdown` component renders for every source of the ported test
// suite, across widths, paddings and options.
//
// This is a stricter oracle than the TS assertions (which check substrings):
// the Rust renderer must reproduce every line byte for byte.

import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { Chalk } from "/Users/dev/projects/notagent-main/node_modules/chalk/source/index.js";
import { Markdown } from "/Users/dev/projects/notagent-main/packages/tui/src/components/markdown.ts";
import {
	resetCapabilitiesCache,
	setCapabilities,
} from "/Users/dev/projects/notagent-main/packages/tui/src/terminal-image.ts";

const OUTPUT = new URL("../tests/fixtures/markdown-render-oracle.json", import.meta.url);
const chalk = new Chalk({ level: 3 });

/** `defaultMarkdownTheme` of `test/test-themes.ts`. */
const theme = {
	heading: (text) => chalk.bold.cyan(text),
	link: (text) => chalk.blue(text),
	linkUrl: (text) => chalk.dim(text),
	code: (text) => chalk.yellow(text),
	codeBlock: (text) => chalk.green(text),
	codeBlockBorder: (text) => chalk.dim(text),
	quote: (text) => chalk.italic(text),
	quoteBorder: (text) => chalk.dim(text),
	hr: (text) => chalk.dim(text),
	listBullet: (text) => chalk.cyan(text),
	bold: (text) => chalk.bold(text),
	italic: (text) => chalk.italic(text),
	strikethrough: (text) => chalk.strikethrough(text),
	underline: (text) => chalk.underline(text),
};

const sources = JSON.parse(
	execFileSync(process.execPath, [new URL("extract-markdown-inputs.mjs", import.meta.url).pathname, "-"], {
		encoding: "utf8",
	}),
);

const variants = [
	{ name: "default", paddingX: 0, paddingY: 0, options: undefined },
	{ name: "padded", paddingX: 2, paddingY: 1, options: undefined },
	{ name: "preserve-markers", paddingX: 0, paddingY: 0, options: { preserveOrderedListMarkers: true } },
	{ name: "preserve-escapes", paddingX: 0, paddingY: 0, options: { preserveBackslashEscapes: true } },
	{ name: "no-latex", paddingX: 0, paddingY: 0, options: { renderLatex: false } },
];
const widths = [80, 60, 40, 20];

const cases = [];
for (const hyperlinks of [false, true]) {
	setCapabilities({ images: null, trueColor: true, hyperlinks });
	for (const source of sources) {
		for (const variant of variants) {
			for (const width of widths) {
				const markdown = new Markdown(source, variant.paddingX, variant.paddingY, theme, undefined, variant.options);
				cases.push({
					source,
					variant: variant.name,
					width,
					hyperlinks,
					lines: markdown.render(width),
				});
			}
		}
	}
}
resetCapabilitiesCache();

writeFileSync(OUTPUT, JSON.stringify(cases));
console.log(`wrote ${cases.length} cases`);
