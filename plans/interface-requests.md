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

### A-3 Stand der TUI-Crate für die Interactive-Verdrahtung
- **Von / An**: A → C (Information)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui`
- **Beleg**: `crates/notagent-tui/PARITY.md`, Abschnitt „Stand der Plan-Tasks"
- **Wunsch**: Kein Handlungsbedarf; damit C planen kann, was bereits nutzbar ist:
  **fertig** — utils, keys, stdin-buffer, ProcessTerminal, TUI-Kern mit Overlays und
  Fokusmaschine, Main-Screen-Renderer, Layout-Engine + ScrollView, alle
  Basis-Komponenten (Text, VStack, HStack, Box, Spacer, TruncatedText, Loader,
  CancellableLoader, SelectList, SettingsList, Input, Image), Keybindings, Fuzzy,
  Terminal-Bilder und -Farben, virtuelles Testterminal (Feature `test-terminal`).
  **offen** — Alt-Screen-Renderer, Editor, Markdown, LaTeX, Autocomplete.
  Die Renderschleife wird vom Aufrufer getrieben: `TuiMainScreen::request_render`,
  `render_deadline()`/`begin_frame()` bzw. `wait_for_render().await`;
  `ProcessTerminal::pump()` verarbeitet stdin, SIGWINCH und die Timeouts.
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

### B-3 Kontrakt-Nachzug: `max_tokens` und `ThinkingBudgets` sind `u64`
- **Von / An**: B → C, A
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/types.rs` (`StreamOptions.max_tokens`, `ThinkingBudgets`)
- **Beleg**: `packages/ai/src/api/simple-options.ts` rechnet `maxTokens` gegen
  `model.maxTokens` und `model.contextWindow` (beide `u64` im Port) — JS kennt nur einen
  Zahlentyp, die Breiten müssen also übereinstimmen.
- **Wunsch**: keiner. Reine Verbreiterung `u32` → `u64`; Integer-Literale bleiben
  quellkompatibel, betroffen wäre nur Code, der die Werte explizit als `u32` typisiert.
- **Status**: umgesetzt (Task 8)

### B-4 faux-Provider steht bereit (Information für C)
- **Von / An**: B → C
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/providers/faux.rs`
- **Beleg**: `packages/ai/src/providers/faux.ts` (708 LOC), vollständig portiert
- **Wunsch**: keiner. Gate G2 verlangt, dass eure Suite-Harness dagegen läuft — der
  Provider ist ab sofort nutzbar:
  ```rust
  let faux = faux_provider(FauxProviderOptions::default());
  faux.set_responses(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop).into()]);
  let models = create_models(None);
  models.set_provider(faux.provider.clone());
  ```
  Unterstützt: Skript-Schritte als Wert oder Factory, Delta-Streaming mit optionaler
  `tokens_per_second`-Bremse, Usage- und Prompt-Cache-Schätzung über `session_id`,
  Abbruch an jeder Chunk-Grenze und den kompletten Deferred-Fluss.
  Meldet euch über diesen Kanal, wenn die Harness etwas braucht, das fehlt.
- **Status**: umgesetzt (Task 11, faux-Teil)

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

### C-1 `get_supported_thinking_levels` in notagent-ai benötigt
- **Von / An**: C → B
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai` (Modellkatalog)
- **Beleg**: `packages/ai/src/models.ts:902-911` (`getSupportedThinkingLevels`), konsumiert von
  `packages/server/src/protocol.ts:207` (`toProtocolModelMetadata`).
- **Wunsch**: `pub fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel>`
  öffentlich exportieren (1:1-Port: `off` bei `reasoning == false`; sonst
  `EXTENDED_THINKING_LEVELS` gefiltert — `null` im `thinking_level_map` schließt aus,
  `xhigh`/`max` brauchen einen gesetzten Eintrag).
- **Status**: umgesetzt (B: `crates/notagent-ai/src/models.rs`); `to_protocol_model_metadata`
  in `crates/notagent-server/src/protocol.rs` nutzt sie und ist testbelegt.

### C-2 Hinweis: `Usage`-Token-Zähler als u64 schließen negative Korrekturen aus
- **Von / An**: C → B (Information, kein Änderungswunsch)
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/types.rs` (`Usage`)
- **Beleg**: `packages/agent/src/harness/session/testing/conformance.ts:600-615` — der Fall
  „keeps latest-value facts and computes ledger statistics across lanes" schreibt einen
  `usage`-Record mit `cause: "adjustment"` und `input: -2`, `totalTokens: -2`,
  `cost.total: -0.5` (Provider-Korrektur). In TS ist `Usage.input` eine `number` und darf
  negativ sein; im Port sind die Token-Zähler `u64`.
- **Wirkung**: Negative Token-Korrekturen sind im Port nicht darstellbar (die Kosten schon,
  die sind `f64`). Der Laufzeitpfad der App ist nicht betroffen — der `adjustment`-Cause
  gehört zum Harness, der laut Master-Plan ausgeschlossen ist. Der portierte
  Konformanzfall dokumentiert die Abweichung in seinen Erwartungswerten.
- **Status**: offen (nur zur Kenntnis; eine Änderung auf `i64` wäre eine Kontraktänderung
  und sollte, wenn überhaupt, von B entschieden werden)

### C-3 `OutputSink::Collector` nur in Tests konstruiert
- **Von / An**: C → A
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-tui/src/terminal.rs:190-192`
- **Beleg**: `cargo clippy -p notagent --all-targets -- -D warnings` schlägt fehl mit
  „variant `Collector` is never constructed". Beim Bauen als Abhängigkeit (ohne die
  Testziele der tui-Crate) ist die Variante tot; `scripts/check.sh` bleibt grün, weil
  `--workspace --all-targets` die tui-Tests mitbaut.
- **Wunsch**: die Variante entweder hinter `#[cfg(test)]` stellen oder produktiv nutzen
  (z. B. für den Byte-Log-Vergleich des RecordingTerminal-Äquivalents).
- **Status**: offen

### C-4 `Credential`/`AuthType` serialisieren OAuth als `o_auth` statt `oauth`
- **Von / An**: C → B
- **Datum**: 2026-08-13
- **Betrifft**: `crates/notagent-ai/src/auth/types.rs:50-56` (`Credential`) und
  `crates/notagent-ai/src/auth/types.rs:89-95` (`AuthType`)
- **Beleg**: `packages/ai/src/auth/types.ts:33` — `OAuthCredential.type` ist `"oauth"`;
  `packages/ai/src/auth/types.ts:114` — `AuthType = "api_key" | "oauth"`. Beide Rust-Typen
  tragen `#[serde(rename_all = "snake_case")]`, und serde macht daraus für die Variante
  `OAuth` den Wert `o_auth`.
- **Wirkung**: `auth.json` ist nicht mehr formatkompatibel — eine von der TS-App
  geschriebene Datei (`{"anthropic":{"type":"oauth",…}}`) lässt sich nicht lesen, eine vom
  Port geschriebene nicht von der TS-App. Das verletzt die Konfigurationskompatibilität aus
  dem Master-Plan. Betroffen sind auch alle Protokoll-/UI-Pfade, die `AuthType` serialisieren.
- **Wunsch**: `#[serde(rename = "oauth")]` an beiden `OAuth`-Varianten (Wert `api_key`
  bleibt durch `rename_all` korrekt).
- **Auswirkung bei C**: `crates/notagent/tests/auth_storage.rs` — die beiden Tests
  `returns_oauth_credentials_unchanged` und
  `translates_a_credential_store_refresh_failure_and_allows_a_later_retry` sind mit
  TS-Wire-Format geschrieben und bis zur Umsetzung `#[ignore]`.
- **Status**: offen
