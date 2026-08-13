// Generates the renderDiff oracle for crates/notagent/tests/render_diff_oracle.rs.
//
// Run from packages/coding-agent of the TypeScript repo:
//   npx tsx tools/gen-render-diff-oracle.mjs > .../tests/fixtures/render-diff-oracle.json
const { renderDiff } = await import("./src/modes/interactive/components/diff.ts");
const { initTheme } = await import("./src/modes/interactive/theme/theme.ts");
const { setCapabilities } = await import("@notagent/tui");

setCapabilities({ images: null, trueColor: true, hyperlinks: false });
initTheme("dark");

const DIFFS = [
	"-  1 const x = 1;\n+  1 const y = 2;",
	" 10 unchanged line\n-11 old line\n+11 new line\n 12 another",
	"-  1 first removed\n-  2 second removed\n+  1 only added",
	"+  1 added only",
	"-  1 removed only",
	"     ...",
	"not a diff line at all",
	" 1 context\n 2 context",
	"-  5 \ttabbed\tcontent\n+  5 \tother\tcontent",
	"-  1 foo(a, b)\n+  1 foo(a, c)",
	"-  1 \n+  1 something",
	"-  1 café naïve\n+  1 cafe naive",
	"-  1 emoji 🎉 here\n+  1 emoji 🎈 here",
	"- 99 a  b  c\n+ 99 a b c",
	"-123 x\n+124 y\n-125 z",
	"",
	"\n",
	"-  1 a\n+  1 b\n-  2 c\n+  2 d",
	"-  1 import { readFileSync } from \"node:fs\";\n+  1 import { writeFileSync } from \"node:fs\";",
	"  1  leading double space content\n-  2   deep indent old\n+  2   deep indent new",
];

process.stdout.write(
	JSON.stringify(
		DIFFS.map((diff) => ({ diff, rendered: renderDiff(diff) })),
		null,
		"\t",
	),
);
