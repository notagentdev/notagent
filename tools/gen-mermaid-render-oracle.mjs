// Oracle for crates/notagent/tests/mermaid_render.rs — prints what grok-mermaid
// draws for a corpus of sources: the plain lines, the styled spans, the width
// and the warnings of `render`, plus `sourceBox` for the fallback path.
//
// Node refuses to strip types under node_modules, so copy the library source
// out first and point the import at the copy:
//   cp -r node_modules/grok-mermaid/src /tmp/gm
//   node tools/gen-mermaid-render-oracle.mjs     # import path: /tmp/gm/index.ts
import { render, sourceBox } from "grok-mermaid/src/index.ts";

const SOURCES = [
	// flowchart — the shapes, links, heads and label paths
	"flowchart LR\n  A[Start] --> B[Done]",
	"graph TD\n  A --> B\n  B --> C",
	"flowchart TD\n  A{Choose} -->|yes| B((Round))\n  A -->|no| C[/Skew/]",
	"flowchart LR\n  A -- label --> B\n  B -.-> C\n  C ==> D",
	"flowchart LR\n  A & B --> C & D",
	"flowchart RL\n  A <-- back -- B",
	"flowchart BT\n  A --> B\n  B --> C",
	"flowchart LR\n  A o--o B\n  X x--x Y",
	"flowchart TD\n  A --> A",
	"flowchart LR\n  A --> A\n  A --> B",
	"flowchart TD\n  A --> B\n  B --> C\n  C --> A",
	"flowchart TD\n  A --> B\n  A --> C\n  B --> D\n  C --> D\n  A --> D",
	"flowchart LR\n  subgraph one[Group One]\n    A --> B\n  end\n  B --> C",
	"flowchart TD\n  subgraph outer\n    subgraph inner\n      A --> B\n    end\n    C\n  end\n  B --> C",
	"flowchart LR\n  A[A very long label that has to wrap somewhere sensible] --> B",
	"flowchart LR\n  A[<b>Bold</b> &amp; more] --> B[`**md**`]",
	"flowchart LR\n  A[Foo]:::highlight --> B[Bar]",
	"flowchart LR\n  A[Foo]:::highlight --> B[Bar]\n  C[Baz]:::other --> D[Qux]",
	"flowchart LR\n  A[Unclosed --> B",
	// state
	"stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy: work\n  Busy --> [*]",
	"stateDiagram-v2\n  direction LR\n  state \"Waiting room\" as W\n  [*] --> W\n  W --> Done",
	"stateDiagram-v2\n  state fork <<choice>>\n  [*] --> fork\n  fork --> A\n  fork --> B",
	"stateDiagram-v2\n  Idle: the idle state\n  [*] --> Idle --> Done",
	"stateDiagram-v2\n  note right of Idle: a note\n  [*] --> Idle",
	"stateDiagram-v2\n  [*] --> Idle\n  bogus statement here",
	// class
	"classDiagram\n  Animal <|-- Duck\n  Animal <|-- Fish",
	"classDiagram\n  class Animal {\n    +int age\n    +String name\n    +isMammal()\n  }",
	"classDiagram\n  <<interface>> Shape\n  Shape <|.. Circle",
	"classDiagram\n  Customer \"1\" --> \"0..*\" Ticket: books",
	"classDiagram\n  class List~T~ {\n    +add(item T)\n  }",
	"classDiagram\n  A o-- B\n  C *-- D\n  E .. F",
	"classDiagram\n  A --|> B\n  garbage ~~~ line",
	// ER
	"erDiagram\n  CUSTOMER ||--o{ ORDER : places",
	"erDiagram\n  CUSTOMER {\n    string name\n    string email \"the address\"\n  }\n  CUSTOMER ||--|{ ORDER : places",
	"erDiagram\n  A[Person] }o..o| B[Company] : works",
	"erDiagram\n  A ||--|| B\n  nonsense",
	// sequence
	"sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi",
	"sequenceDiagram\n  participant A as Alice\n  participant B as Bob\n  A->>B: ask\n  B--xA: refuse",
	"sequenceDiagram\n  autonumber\n  A->>B: one\n  B->>A: two",
	"sequenceDiagram\n  A->>A: think",
	"sequenceDiagram\n  Note over A,B: a shared note\n  A->>B: go",
	"sequenceDiagram\n  Note left of A: on the left\n  Note right of B: on the right\n  A->>B: go",
	"sequenceDiagram\n  loop every minute\n    A->>B: poll\n  end",
	"sequenceDiagram\n  alt is sunny\n    A->>B: walk\n  else is raining\n    A->>B: stay\n  end",
	"sequenceDiagram\n  A->>B: with activation\n  activate B\n  deactivate B",
	"sequenceDiagram\n  A-)B: async\n  B--)A: async back",
	// nothing to draw
	"pie\n  title Pets\n  \"Dogs\" : 4",
	"   ",
	"flowchart LR",
];

const out = SOURCES.map((src) => ({
	src,
	art: render(src),
	sourceBox: sourceBox(src, 40),
}));
console.log(JSON.stringify(out, null, "\t"));
