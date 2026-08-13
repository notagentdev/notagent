# Faktenbericht: Rust-Referenz für Minifizierung (read_minified / patch_minified)

Quelle: `/Users/dev/projects/notagent-main-rust` (bestehendes Rust-Projekt, Stand 2026-08-13).

**Vorgabe (Nutzer): tree-sitter wird NATIV und byte-genau genutzt — kein web-tree-sitter/WASM.**

## Referenz-Implementierung
- `/Users/dev/projects/notagent-main-rust/crates/notagent_services/src/tool_services/minify.rs` — 897 LOC. Span-basierte Minifizierung: Kommentare entfernt, Leerzeilen weg, Einrückung auf 1 Space pro Tiefe kollabiert, Whitespace-Runs zwischen Tokens auf 1 Space; Zeilenstruktur bleibt erhalten (keine Statement-Merges) → Ausgabe syntaktisch gültig, auch für Python. Mehrzeilige Literal-Tokens (Raw-Strings, Heredocs, Template-Literals, Docstrings) byte-genau erhalten.
- `MinifyResult` enthält: `text` (minifiziert), `src_map: Vec<usize>` (**Quell-Byte-Offset für jedes Byte des minifizierten Texts** — kopierte Bytes mappen exakt, synthetische Bytes auf den Start ihrer Quellregion), `indent_widths` (sortierte distinct Einrückungsbreiten; minifizierte Tiefe d ↔ indent_widths[d]), Kommentar-Byte-Ranges.
- `/Users/dev/projects/notagent-main-rust/crates/notagent_services/src/tool_services/minify_edit.rs` — 913 LOC (Edit-Rückabbildung minifizierter Raum → Original).
- `minify_multi_edit_tests.rs` — 244 LOC Tests. Zusätzlich Testbench: `/Users/dev/projects/notagent-main-rust/minified-edit-testbench/`.
- Tool-Beschreibungen: `crates/notagent_domain/src/tools/descriptions/fs_{read,patch,multi_patch}_minified.md`.

## Crate-Versionen (aus Cargo.toml, produktiv erprobt)
tree-sitter = "0.26.9"; tree-sitter-rust = "0.24.2"; tree-sitter-python = "0.25.0"; tree-sitter-javascript = "0.25.0"; tree-sitter-typescript = "0.23.2" (LANGUAGE_TYPESCRIPT + LANGUAGE_TSX); tree-sitter-go = "0.25.0"; tree-sitter-java = "0.23.5"; tree-sitter-c = "0.24.2"; tree-sitter-cpp = "0.23.4"; tree-sitter-ruby = "0.23.1"; tree-sitter-bash = "0.25.1"; tree-sitter-css = "0.25.0"; tree-sitter-html = "0.23.2"; tree-sitter-json = "0.24.8".

Extension-Mapping (language_for_extension): rs; py/pyi; js/mjs/cjs/jsx; ts/mts/cts; tsx; go; java; c/h; cpp/cc/cxx/hpp/hh/hxx; rb; sh/bash/zsh; css; html/htm; json/jsonc — identische Sprachliste wie die TS-Version (packages/coding-agent/src/core/mini-read/, tree-sitter-wasms).

## Konsequenz für den Port
Der Port von `core/mini-read/` + `read_minified` + `patch_minified`/`multi_patch_minified` ADAPTIERT diese Referenz (lesen, verstehen, übernehmen/anpassen) statt die WASM-basierte TS-Implementierung nachzubauen. Verhaltens-Oracle bleiben die TS-Tests der Tools; die Referenz-Testbench ergänzt.
