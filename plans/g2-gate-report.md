# Gate G2 — Headless-Parität (Abnahmebericht)

**Datum**: 2026-08-15 · **Abnahme durch**: Workstream C (Gate-Verwalter) · **Tag**: `gate-g2`
**Grundlage**: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt Gates.

## Kriterien und Belege

| Kriterium (Master-Plan) | Beleg | Ergebnis |
|---|---|---|
| Print-Modus end-to-end gegen faux-Provider (Prompt → Tool-Calls → Antwort) | `crates/notagent/tests/headless_end_to_end.rs::print_mode_runs_a_prompt_through_a_tool_call_to_the_answer` — echte Modell-Laufzeit mit nativ registriertem faux-Provider, echte Services aus `create_agent_session_services`, echte Session, echtes `write`-Tool; die geschriebene Datei auf der Platte ist der Zeuge, danach die Textantwort und Exit-Code 0 | erfüllt (5 Tests grün) |
| Komplette Tool-Suite grün | `bash_tool` 27, `task_tools` 24, `minified_tools` 17, `todo_and_skill_tools` 16, `task_tool` und die übrigen Tool-Suiten im Gesamtlauf | erfüllt |
| Subagenten mit nachweislich paralleler Ausführung (Zeitmessung) | `crates/notagent/tests/tasks_parallel.rs` — 4 Tests, Wanduhrmessung über acht detachte Kommandos und acht Kinder aus einem `task`-Aufruf | erfüllt (0,53 s für Arbeit, die seriell ein Vielfaches bräuchte) |
| Permissions-Policy-Chain-Tests grün | `permissions` 80 Tests, `permission_end_to_end` 31 Tests | erfüllt |
| `scripts/check.sh` grün auf dem Gesamtworkspace | Lauf vom 2026-08-15 auf `ws/c-app` (Stand des Merges): fmt, clippy `-D warnings`, `cargo test --workspace` | erfüllt — 231 Testziele, 3 423 Tests, 0 Fehler |

## Zusätzlich in diesem Gate erbracht

- **RPC-Modus**: `rpc_prompt_response_semantics` (3 Tests, Port der TS-Suite) und die
  RPC-Hälfte von `headless_end_to_end` (Kommandos, Ereignisstrom, Fehlerantworten).
- **CLI end-to-end gegen die gebaute Binary**: `session_id_readonly` (8 Tests) prüft die
  Startreihenfolge von `main.ts` — welche Flags eine Session auf der Platte reservieren,
  die Warnungen, ungültige Session-Ids ohne Stacktrace und die Fehlermeldung bei einer
  Nicht-Session-Datei.
- **Manueller Binary-Smoke** (Vorgriff auf G4): `notagent --version`, `notagent --help`,
  `notagent --list-models` laufen als Binary; `-p` gegen faux ist im Test abgedeckt, gegen
  einen echten Provider steht er für G4 aus.

## Zwei Befunde, die das Gate aufgehalten haben

1. **Verpasste Weckrufe** (`plans/interface-requests.md` C-15): `Notify::notified()`
   registriert den Warter erst beim ersten Poll. An vier Stellen stand die Prüfung vor dem
   `await`, ohne `enable()` — ein `notify_waiters()` dazwischen ging verloren.
   Korrigiert in `notagent-ai/src/utils/event_stream.rs` (2×),
   `notagent-agent/src/agent.rs` (`wait_for_idle`), `notagent/src/core/agent_session.rs`
   und `notagent/src/core/output_guard.rs`. Zwei davon liegen in Bs Crates; mechanisch,
   ohne Verhaltenswechsel, mit Bitte um Gegenlesen im Request dokumentiert.
2. **Race in der portierten Queue-Suite**: `start_run` pollte `is_streaming()` und drehte
   endlos, wenn der Lauf schneller fertig war als die erste Prüfung (reproduzierbar in
   ~10 % der Läufe, ohne Bezug zu Task 12). Die Suite hält den Lauf jetzt wie die
   TS-Vorlage mit einem blockierenden `wait`-Tool offen. 40 Läufe in Folge grün.

## Offene Punkte, die G2 nicht berühren

- Der Interactive-Zweig, `--resume` (Session-Picker), First-Time-Setup, Trust- und
  Session-cwd-Rückfrage brauchen eine TUI-Renderschleife; dafür fehlt der Pump-Seam
  (Interface-Request C-14 an A). Sie landen mit C-Task 13 (Gate G3).
- `--export` (HTML-Export) und die Package-/Config-Kommandos warten auf Bs Tasks 15/16.
- Die Komponenten-Zuteilung an A für die Interactive-Verdrahtung steht als C-16 im
  Interface-Request-Kanal.
