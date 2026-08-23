# notagent-tui

Terminal UI library with differential rendering: components render to lines,
and only changed lines are written to the terminal.

- `components/` — building blocks: text, markdown, input, editor, select
  lists, loaders with shimmer, scroll views, stacks, images
- `layout.rs` / `layout_node.rs` — layout tree and measurement
- `keybindings.rs` / `keys.rs` — key parsing and configurable bindings
- `fuzzy.rs` — fuzzy filtering for selectors
- `latex.rs` / `latex_tables.rs` — LaTeX-to-unicode rendering in markdown

The crate knows nothing about agents or LLMs; it is a general-purpose TUI
toolkit used by the `notagent` binary.
