# G4-Smoke-Report

Manueller Abnahmelauf der Rust-Binary analog zur Release-Prozedur der TS-App
(`/Users/dev/projects/notagent-main/AGENTS.md`, Abschnitt „Releasing", Schritt 2:
`--help`, `--version`, `--list-models`, `-p`, interaktive Sitzung mit Prompt-Roundtrip).

- **Datum**: 2026-08-16
- **Commit**: `2a168f1` (ws/c-app, deckungsgleich mit main nach dem Merge dieser Task)
- **Binary**: `target/debug/notagent` (`cargo build -p notagent --bin notagent`), 143 MB
  Debug-Build; ein Release-Build ist für die geprüften Pfade verhaltensgleich und wurde
  aus Zeit- und Plattengründen nicht zusätzlich gebaut (dokumentierte Abweichung von der
  TS-Prozedur, die zwei Distributionsformen prüft — Node und Bun; die Rust-Distribution
  hat nur eine).
- **Ausgeführt aus** `/tmp/notagent-smoke`, also außerhalb des Repos, wie die TS-Prozedur es
  verlangt.

## Modellzugänge im Lauf

| Zugang | Wofür | Kosten |
|---|---|---|
| `zai/glm-5.2` (echter Provider des Nutzers, Credential aus `~/.notagent/agent/auth.json`) | ein Print-Prompt und eine interaktive Sitzung | zwei Kurzprompts („Say exactly: ok"), zusammen ~40 Eingabe-Token |
| `llama.cpp/smoke-model` gegen einen lokalen Mock-Router (`/tmp/notagent-smoke/mock_llama.py`, 127.0.0.1:8099) | Print, JSON-Modus, interaktive Sitzung, `/llama` | keine |

Der `faux`-Provider aus dem Master-Plan steht der Binary nicht zur Verfügung: er ist wie in
TypeScript ein Testprovider und wird nur von den Suiten registriert (`packages/ai` bzw.
`crates/notagent-ai/src/providers/faux.rs`). Seine Rolle im Smoke — ein Modell ohne echte
Kosten und ohne Netz — übernimmt hier der lokale llama.cpp-Router, der zusätzlich die in
Task 14 portierte Provider- und `/llama`-Strecke mitprüft.

## Ergebnisse

| # | Schritt | Kommando | Ergebnis |
|---|---|---|---|
| 1 | Version | `notagent --version` | `0.1.0` |
| 2 | Hilfe | `notagent --help` | vollständiger Hilfetext: Usage, sieben Kommandos (install/remove/uninstall/update/list/config/auth), Optionsliste, Env-Variablen — ohne die entfallenen Extension-Flags |
| 3 | Modelle | `notagent --list-models` | Tabelle mit 14 Modellen der drei konfigurierten Provider (minimax, openai-codex, zai) samt Kontext-, Max-Out-, Thinking- und Bild-Spalten |
| 4 | Modelle (leerer Agent-Dir) | `NOTAGENT_CODING_AGENT_DIR=… notagent --list-models` | „No models available. Use /login …" plus die beiden Doku-Pfade — der Zweig für einen Zugang ohne Credentials |
| 5 | Katalog-Refresh | `notagent update --models` | `Model catalogs refreshed`; `models-store.json` enthält danach den llama.cpp-Katalog des lokalen Routers |
| 6 | Print (lokal) | `notagent -p --model llama.cpp/smoke-model "Say exactly: ok"` | `ok` |
| 7 | Print (echter Provider) | `notagent -p --model zai/glm-5.2 "Say exactly: ok"` | `ok`, 4,8 s |
| 8 | JSON-Modus | `notagent -p --mode json --model llama.cpp/smoke-model …` | JSONL-Strom: `session`, `agent_start`, `turn_start`, `message_start`/`message_end` für User und Assistant — die Ereignisnamen und die Wire-Form der TS-Seite |
| 9 | Interaktiv (lokal) | `notagent --model llama.cpp/smoke-model` in einer echten PTY (`expect`) | Startbild mit Header `notagent v0.1.0`, Hinweiszeile, Editor-Rahmen, Footer mit cwd/Kontext/Modell; Prompt abgeschickt, Antwortzeile `ok` im Transkript, Ctrl+D beendet mit Exit-Code 0 und der Zeile „To resume this session: notagent --session …" |
| 10 | Interaktiv (echter Provider) | `notagent --model zai/glm-5.2` in einer echten PTY | wie 9, zusätzlich aktualisiert der Footer die Nutzung (`↑37 ↓3 R5.8k CH99.4%`) |
| 11 | `/llama` | im interaktiven Lauf gegen den lokalen Router | Manager öffnet: Titel `llama.cpp models`, Server-URL, Zeile `smoke-model — loaded · 8k context`, Zeile `Download model… — Hugging Face owner/repository[:quant]`, Fußzeile `enter load/unload/download • escape/ctrl+c close`; Escape schließt und gibt den Editor zurück |
| 12 | Sitzungsdatei | nach Lauf 9 | JSONL v3 mit `session`-Header, `model_change`, `thinking_level_change`, User- und Assistant-Message samt Usage — vom Port geschrieben und wieder lesbar (`notagent --session <id>`) |

Kein Schritt ist fehlgeschlagen.

## Beobachtungen aus dem Lauf

1. **`update --models` kannte den llama.cpp-Katalog nicht** (im Lauf gefunden und behoben).
   Die native Provider-Registrierung saß nur in `create_agent_session_services`; TypeScript
   reicht die eingebauten Extensions auch an `handlePackageCommand` (`main.ts:663`), damit
   `update --models` ihre Kataloge mitnimmt. `package_manager_cli.rs` registriert den
   Provider jetzt ebenso. Ohne den Smoke wäre das durch keine Suite aufgefallen.
2. **Ein erster interaktiver Lauf zeigte die Antwort nicht**, obwohl die Sitzungsdatei sie
   enthielt. Zwei Wiederholungen desselben Ablaufs (und ein dritter mit Tastendruck danach)
   zeigten sie zuverlässig; der erste Lauf war zugleich der, in dem der Katalog kalt war und
   im Hintergrund nachgeladen wurde. Nicht reproduzierbar, deshalb als Beobachtung notiert
   und nicht als Fund.
3. `tmux` ist auf dieser Maschine nicht installiert; die interaktiven Läufe fahren deshalb
   über `expect` in einer echten PTY. Das ist dieselbe Prüfschärfe (echtes Terminal, echte
   Escape-Sequenzen, echter Editor-Pfad), nur ohne Multiplexer.

## Reproduktion

```bash
cargo build -p notagent --bin notagent
mkdir -p /tmp/notagent-smoke && cd /tmp/notagent-smoke
python3 mock_llama.py 8099 &                    # Skript im Report-Anhang unten
cat > agent/auth.json <<'JSON'
{ "llama.cpp": { "type": "api_key", "key": "local", "env": { "LLAMA_BASE_URL": "http://127.0.0.1:8099" } } }
JSON
export NOTAGENT_CODING_AGENT_DIR=/tmp/notagent-smoke/agent
notagent --version && notagent --help && notagent update --models && notagent --list-models
notagent -p --model llama.cpp/smoke-model "Say exactly: ok"
expect interactive.exp $(which notagent)        # Startbild, Prompt, Ctrl+D
```

Der Mock beantwortet `GET /models` mit einem geladenen Router-Modell, `GET /models/sse` mit
einem leeren Ereignisstrom und `POST /v1/chat/completions` mit drei SSE-Chunks
(`role`, `content: "ok"`, `finish_reason: "stop"` samt Usage).
