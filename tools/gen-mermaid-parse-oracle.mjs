// Oracle for crates/notagent/tests/mermaid_parse.rs — prints what grok-mermaid's
// flowchart grammar makes of a corpus of sources.
//
// Node refuses to strip types under node_modules, so copy the library source
// out first and point the import at the copy:
//   cp -r node_modules/grok-mermaid/src /tmp/gm
//   node tools/gen-mermaid-parse-oracle.mjs      # import path: /tmp/gm/parse.ts
import { diagramKind, parseGraph, statementsOf } from "grok-mermaid/src/parse.ts";

const SOURCES = [
	"flowchart LR\n  A[Start] --> B[Done]",
	"graph TD\n  A --> B\n  B --> C",
	"pie\n  title Pets\n  \"Dogs\" : 4",
	"stateDiagram-v2\n  [*] --> Idle",
	"flowchart LR\n  A[Foo]:::highlight --> B[Bar]",
	"flowchart LR\n  A[Foo]:::highlight --> B[Bar]\n  C[Baz]:::other --> D[Qux]",
	"flowchart TD\n  A{Choose} -->|yes| B((Round))\n  A -->|no| C[/Skew/]",
	"flowchart LR\n  A -- label --> B\n  B -.-> C\n  C ==> D",
	"flowchart LR\n  A & B --> C & D",
	"flowchart RL\n  A <-- back -- B",
	"flowchart LR\n  A o--o B\n  X x--x Y",
	"flowchart LR\n  subgraph one[Group One]\n    A --> B\n  end\n  B --> C",
	'flowchart LR\n  A["a] b"] --> B[5" pipe]',
	"flowchart LR\n  A[<b>Bold</b> &amp; more] --> B[`**md**`]",
	"flowchart LR\n  A[Unclosed --> B",
	"flowchart LR\n  --> B",
	"flowchart LR; A-->B; %% comment\n  B-->C",
	"flowchart LR\n  A[A very long label that has to wrap somewhere sensible] --> B",
];

const out = SOURCES.map((src) => {
	const graph = parseGraph(src);
	return {
		src,
		kind: diagramKind(src),
		statements: statementsOf(src),
		graph:
			graph === null
				? null
				: {
						dir: graph.dir,
						nodes: graph.nodes,
						edges: graph.edges,
						groups: graph.groups,
						nodeGroup: graph.nodeGroup,
						warnings: graph.warnings,
					},
	};
});
console.log(JSON.stringify(out, null, "\t"));
