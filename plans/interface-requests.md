# Interface-Requests (workstream-übergreifender Kanal)

Einziger Kommunikationskanal zwischen den drei Workstreams. **Append-only**: neue
Einträge unten in der eigenen Sektion anhängen, fremde Einträge nie löschen oder
umschreiben. Der Owner der betroffenen Crate setzt um und hakt ab.

Ownership: A = `crates/notagent-tui/`, B = `crates/notagent-{telemetry,ai,agent}/`,
C = alles übrige (Root-Dateien, Protokoll-Crates, `crates/notagent/`).

Eintragsformat:

```
### <ID> <kurzer Titel>
- **Von / An**: <Workstream> → <Workstream>
- **Datum**: YYYY-MM-DD
- **Betrifft**: <Crate/Datei/Typ>
- **Beleg**: <TS-Quelle mit Zeilen, die das Verhalten festlegt>
- **Wunsch**: <konkret, mit Signaturvorschlag>
- **Status**: offen | umgesetzt (<commit>) | abgelehnt (<Begründung>)
```

IDs: `A-1`, `B-1`, `C-1`, … fortlaufend je Absender.

## Sektion A (Workstream A — TUI)

### A-1 vt100 als Workspace-Dependency (Test-Terminal)
- **Von / An**: A → C
- **Datum**: 2026-08-13
- **Betrifft**: Root-`Cargo.toml`, `[workspace.dependencies]`
- **Beleg**: `packages/tui/test/virtual-terminal.ts:1-218` (implementiert `Terminal` auf
  `@xterm/headless`); Master-Plan Tech-Substitution „@xterm/headless (Test-Terminal) →
  Rust-VT-Emulator-Crate"; WS-A-Plan Task 1 (Entscheidung nach harten Kriterien).
- **Wunsch**: Bitte `vt100 = "0.16"` in `[workspace.dependencies]` aufnehmen. Nutzung
  ausschließlich als `[dev-dependencies]` in `crates/notagent-tui` (Test-Harness
  `VirtualTerminal`), nicht im Produktivpfad.
  Begründung der Auswahl (empirisch, 69 Prüfpunkte gegen xterm.js-Fixtures aus dem
  TS-Repo, 64 grün): vt100 bildet als einziger Kandidat die für die Testerwartungen
  entscheidende Zellsemantik von xterm.js `translateToString(true)` ab (geschriebenes
  Leerzeichen bleibt erhalten, nie beschriebene/gelöschte Zelle fällt weg —
  `Cell::has_contents()`); avt 0.18 kann das strukturell nicht (`Cell::blank` == ' '
  mit Default-Pen, `is_default()` nicht unterscheidbar), wezterm-term ist auf crates.io
  nur als Fork verfügbar.
- **Status**: umgesetzt (Root-`Cargo.toml`, `[workspace.dependencies] vt100 = "0.16"`)

### A-2 Hinweis: Terminal-Handler ohne `Send`, Test-Terminal hinter Feature
- **Von / An**: A → C (nur Information, keine Aktion nötig)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui/src/terminal.rs`, `crates/notagent-tui` Feature `test-terminal`
- **Beleg**: `packages/tui/src/tui.ts` (Komponentenmodell mit geteilten Objektreferenzen,
  Render-Kern einsträngig); `packages/tui/test/virtual-terminal.ts:1-218`
- **Wunsch**: Zwei Kontrakt-Details, gegen die C bauen kann:
  1. `InputHandler`/`ResizeHandler` sind `Box<dyn FnMut(...)>` **ohne** `Send` — der
     TUI-Kern läuft wie in TS einsträngig; der stdin-Leser reicht Daten per Kanal
     an den TUI-Strang.
  2. Das virtuelle Testterminal liegt in `notagent_tui::test_terminal` hinter dem
     Feature `test-terminal` (hält `vt100` aus Produktivbuilds). Für die
     G3-E2E-Szenarien:
     `notagent-tui = { workspace = true, features = ["test-terminal"] }` in
     `[dev-dependencies]`.
- **Status**: umgesetzt (A)

## Sektion B (Workstream B — AI + Agent)

### B-1 Kontrakt-Entscheidungen des Typ-Commits (Information für C)
- **Von / An**: B → C (und A, soweit betroffen)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/types.rs`, `crates/notagent-agent/src/types.rs`
- **Beleg**: `packages/ai/src/types.ts` (830), `packages/agent/src/types.ts` (443),
  `packages/agent/src/harness/messages.ts:55-62`, `packages/coding-agent/src/core/messages.ts:69-76`,
  Session-Fixtures `packages/coding-agent/test/fixtures/*.jsonl`
- **Wunsch**: keiner — dies dokumentiert die Form der Typen, gegen die C programmiert:
  1. `AgentMessage` ist eine feste Aufzählung mit sieben Rollen (`user`, `assistant`, `toolResult`,
     `bashExecution`, `custom`, `branchSummary`, `compactionSummary`). Declaration Merging gibt es in
     Rust nicht; die vier Custom-Rollen sind in agent-core und coding-agent identisch deklariert.
  2. `AgentTool` ist ein Trait (`name`/`description`/`parameters`/`label`/`execute`/`execution_mode`,
     `to_tool()` liefert die `Tool`-Sicht). Tools werden als `Arc<dyn AgentTool>` gehalten.
  3. Optionshierarchie per Komposition: `SimpleStreamOptions { base: StreamOptions { base:
     ProviderRequestOptions, .. }, .. }`; alle drei Ebenen haben `Default`.
  4. `Usage.total_tokens` ist `Option<u64>`: historische Session-Dateien der TS-App enthalten
     `totalTokens` nicht (18 von 32 Assistant-Nachrichten im Fixture). `estimate.ts` behandelt
     `undefined` und `0` gleich, das Verhalten bleibt identisch.
  5. Content-Blöcke (`TextContent`, `ThinkingContent`, `ToolCall`) haben ein `extra: Map<String, Value>`
     (`#[serde(flatten)]`). Es hält Scratch-Felder abgebrochener Streams (`partialJson`), die die
     TS-Implementierung in Session-Dateien schreibt — nötig für verlustfreie Session-Roundtrips.
  6. Zahlen werden JS-kompatibel serialisiert (`utils/js_number`): `0` statt `0.0`, `0.000003`
     statt `3e-6`.
- **Status**: umgesetzt (Kontrakt-Commit `ai: …`/`agent: …`, Task 1)

### B-2 serde_json-Feature `raw_value` für vollständige JS-Zahlparität
- **Von / An**: B → C (Owner der Root-`Cargo.toml`)
- **Datum**: 2026-08-13
- **Betrifft**: `Cargo.toml`, `[workspace.dependencies] serde_json`
- **Beleg**: `JSON.stringify(0.000003)` → `"0.000003"`, `serde_json` → `"3e-6"`;
  `JSON.stringify(1e20)` → `"100000000000000000000"`, `serde_json` → `"1e+20"`.
  Referenzwerte in `crates/notagent-ai/src/utils/js_number.rs` (Test gegen Node-Ausgabe).
- **Wunsch**: `serde_json = { version = "1", features = ["preserve_order", "raw_value"] }`.
  Damit kann `js_number::serialize` nicht-ganzzahlige Werte über `RawValue` exakt in
  JS-Schreibweise ausgeben. Ohne das Feature bleibt die aktuelle Lösung (ganzzahlige Werte als
  JSON-Integer), die den häufigsten Fall (`0`) abdeckt; Kosten-Nachkommawerte unterhalb 1e-6
  bzw. ab 1e21 würden abweichend formatiert.
- **Status**: umgesetzt (Root-`Cargo.toml`, `serde_json` mit `preserve_order` + `raw_value`)

## Sektion C (Workstream C — App)

_(noch leer)_
