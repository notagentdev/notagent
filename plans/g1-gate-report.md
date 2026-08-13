# Gate G1 — Abnahme (Fundamente)

Datum: 2026-08-13 · Prüfer: Agent C (Gate-Verwalter) · Commit: siehe Tag `gate-g1`

## Kriterien aus dem Master-Plan

> **G1 — Fundamente**: A: utils/keys/Terminal/TuiBase/Main-Screen-Renderer mit
> Virtual-Terminal-Tests grün. B: ai-Kerntypen, EventStream, Anthropic- +
> OpenAI-Completions-Streaming, faux-Provider, Agent-Loop mit portierten Loop-Tests grün.
> C: protocol/client/server/session-sqlite fertig (Konformanztests portiert),
> session-manager + Basistools (read/write/edit/ls) grün.

## Ergebnis: erfüllt

### Gesamtworkspace
`scripts/check.sh` auf main grün (fmt, clippy `-D warnings`, test):
**1 690 Tests bestanden, 0 fehlgeschlagen, 2 ignoriert.**

Die zwei ignorierten Tests sind `crates/notagent/tests/auth_storage.rs` — sie hängen an
Interface-Request C-4 (notagent-ai serialisiert OAuth-Credentials als `o_auth` statt `oauth`)
und sind mit dem TS-Wire-Format geschrieben. Sie blockieren kein G1-Kriterium.

### A — notagent-tui (711 Tests)
Tasks 1-14 abgehakt. Belegt im Ledger: `test/virtual-terminal.ts` als Testhelfer
(`src/test_terminal.rs`), `src/utils.ts`, `src/keys.ts`, `src/stdin-buffer.ts`,
`src/terminal.ts`, `src/tui.ts`, `src/tui-main-screen.ts` (tui-render-Suite 24/24 Fälle
inkl. Kitty-Vollredraw), `src/tui-alt-screen.ts`, Layout, Komponenten, Editor, Markdown/LaTeX,
Terminal-Bilder, Autocomplete/Fuzzy/Keybindings, öffentliche API.
Offen im Ledger: nur Task 15 (App-TUI-Zuarbeit), kein G1-Kriterium.

### B — notagent-ai (430), notagent-agent (36), notagent-telemetry (7)
Tasks 1-9, 11 (faux) und 12 abgehakt: Kontrakttypen (`ai/types.ts`, `agent/types.ts`),
EventStream, Provider-/Models-Registry, Modellkatalog, Auth-Kern und OAuth-Flows,
SSE + Anthropic-API, OpenAI-Familie, faux-Provider (`providers/faux.rs`, verifiziert),
Agent-Loop mit portierten Loop-Tests. Offen: Task 10 (Google/Vertex/Bedrock) und Task 13
(Testabschluss) — beides keine G1-Kriterien.

### C — notagent-protocol (44), notagent-client (36), notagent-server (47),
### notagent-session-sqlite (71), notagent (308)
- Tasks 2-4 abgehakt: Protokoll (CBOR-Subset, Framing, Schemas), Client (Leases, Revisions),
  Server (Stage-Automat, Unix-Transport, LiveSessionManager) samt der portierten
  Konformanztests beider Seiten; sqlite-Backend mit Writer-Lease, FTS5 und Konformanzsuite.
- Task 6 abgehakt: `core/session-manager.ts` vollständig, 73 Tests inklusive Roundtrips
  gegen die TS-Fixtures `before-compaction.jsonl` und `large-session.jsonl`.
- Task 7 abgehakt: `read`, `write`, `edit`, `ls` (plus `find`, `grep`) grün, dazu
  truncate/path-utils/file-mutation-queue/edit-diff/output-accumulator/tools-manager,
  MIME-Sniffing und Bild-Pipeline sowie der native `ToolDefinition`-Kontrakt.

## Ledger-Selbstaudit

| Crate | Ledger-Zeilen | offen |
|---|---|---|
| notagent-protocol | 29 | 0 |
| notagent-client | 39 | 0 |
| notagent-server | 53 | 0 |
| notagent-session-sqlite | 47 | 0 |
| notagent | 66 | 5 |
| notagent-tui | 310 | 1 |
| notagent-ai | 178 | 0 |
| notagent-agent | 25 | 0 |
| notagent-telemetry | 21 | 0 |

Die fünf offenen Zeilen in `crates/notagent/PARITY.md` sind mit ihrer Zieltask geführt:
`tools/render-utils.ts` und `tools/index.ts` (Task 13 bzw. nach Tasks 8-10), sowie drei
session-bezogene CLI-E2E-Tests (Tasks 11/12). Keine davon ist G1-Kriterium.

## Offene Anforderungen an A/B (in plans/interface-requests.md)

- **C-3** (an A): `OutputSink::Collector` nur in Tests konstruiert — kosmetisch, kein Gate.
- **C-4** (an B): `Credential`/`AuthType` serialisieren OAuth als `o_auth` statt `oauth`.
  Verletzt die auth.json-Formatkompatibilität; muss vor G2 umgesetzt sein, weil der
  Print-Modus gegen echte Credentials laufen soll.
- **C-5** (an A): Komponenten-Zuteilung für A-Task 15 (Theme-Batch 0 blockiert Cs
  `tools/render-utils.ts` und die Renderer-Hälften aller Tools).
