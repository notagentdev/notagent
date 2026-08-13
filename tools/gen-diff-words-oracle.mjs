// Generates the jsdiff `diffWords` oracle for crates/notagent/tests/diff_words_oracle.rs.
//
// Run from the TypeScript repo so that the real `diff` package (8.0.4) resolves:
//   node tools/gen-diff-words-oracle.mjs > crates/notagent/tests/fixtures/diff-words-oracle.json
import * as Diff from "diff";

const OLD_LINES = [
	"const x = 1;",
	"let value = compute(x)",
	"  indented old",
	"foo(a, b)",
	"return foo.bar(1);",
	"a b c",
	"",
	"old",
	"  leading",
	"tab\there",
	"a  b",
	"CamelCase word",
	"x=1",
	"if (a && b) { return c; }",
	"import { readFileSync } from \"node:fs\";",
	"export function render(width: number): string[] {",
	"\tconst items = list.filter((item) => item.enabled);",
	"// TODO: fix this later",
	"await Promise.all(tasks.map((t) => t.run()));",
	"const re = /^[a-z]+$/gu;",
	"café naïve",
	"emoji 🎉 here",
	"日本語 テキスト",
	"a b",
	"   ",
	"one",
	"same text",
	"trailing space ",
	"multi   spaces   here",
	"x",
];
const NEW_LINES = [
	"const y = 2;",
	"let result = compute(y)",
	"  indented new",
	"foo(a, c)",
	"return foo.baz(2);",
	"a b c d",
	"new",
	"",
	"trailing  ",
	"tab\tthere",
	"a b",
	"camelcase word",
	"x=2",
	"if (a || b) { return d; }",
	"import { writeFileSync } from \"node:fs\";",
	"export function render(width: number, height: number): string[] {",
	"\tconst items = list.filter((item) => item.visible);",
	"// FIXME: fix this now",
	"await Promise.allSettled(tasks.map((t) => t.start()));",
	"const re = /^[A-Z]+$/gu;",
	"cafe naive",
	"emoji 🎈 here",
	"日本語 テキストです",
	"a b",
	"  ",
	"one two three",
	"same text",
	" trailing space",
	"multi spaces here",
	"",
];

const cases = [];
for (const oldLine of OLD_LINES) {
	for (const newLine of NEW_LINES) {
		cases.push({
			old: oldLine,
			new: newLine,
			parts: Diff.diffWords(oldLine, newLine).map((part) => ({
				value: part.value,
				added: !!part.added,
				removed: !!part.removed,
			})),
		});
	}
}
process.stdout.write(JSON.stringify(cases, null, "\t"));
