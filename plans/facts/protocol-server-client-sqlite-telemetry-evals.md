# Faktenbericht: protocol, client, server, session-backends, telemetry, evals

Quelle: `/Users/dev/projects/notagent-main/packages/*`. Alle Aussagen aus dem Quellcode (Stand 2026-08-13).

## 1. @notagent/protocol (1 236 LOC src)
Transport-neutrales **CBOR-Binärprotokoll** für Remote-Sessions. KEIN JSON-RPC. Version 1.
Dateien: index.ts 4, schemas.ts 450 (TypeBox-Schemas + PROTOCOL_VERSION=1), codec.ts 172 (encode/decode + ProtocolValidationError + isSupportedProtocolVersion), framing.ts 165 (encodeFrame, FrameDecoder, DEFAULT_MAX_FRAME_LENGTH 16 MiB), cbor/encoder.ts 216, cbor/decoder.ts 168, cbor/options.ts 52. Tests: 716 LOC. Dep: nur typebox. Kein node:*-Import (runtime-neutral).

**Wire-Format**: [uint32 BE Länge][ein definite-length CBOR-Item]. Header exakt 4 Byte. FrameDecoder inkrementell (64-KiB-Blöcke), Nulllängen-Frames erlaubt, end() erkennt Truncation.

**CBOR-Subset (strikt, RFC 8949 Teilmenge)**: Major 0,1,2,3,4,5,7; Simple nur 20/21/22/27 (false/true/null/float64 0xfb). **Abgelehnt**: Tags (Major 6), Indefinite-Length, Break, Float16/32, Zyklen, sparse Arrays, undefined in Arrays, nicht-finite Zahlen, unsichere Integer, doppelte Map-Keys, Nicht-String-Map-Keys, Trailing Data, ungültiges UTF-8. Integer kanonisch kürzeste Form. undefined-Properties beim Encoding weggelassen. Limits: maxByteLength 16 MiB, maxContainerLength 1e6, maxDepth 64 (Max 512).

**Nachrichten**: Client→Server: ClientHello {type:"hello", version} (MUSS erste sein), RequestEnvelope {type:"request", id, request}. Commands (9): list, create {cwd?,name?,model?,thinkingLevel?}, attach {sessionId}, detach, prompt {sessionId,text}, steer, abort, set_model, set_thinking. Server→Client: ServerHello {type:"hello", version:1, connectionId, snapshot}, ServerHelloError, ResponseEnvelope {ok:true,result}|{ok:false,error}, EventEnvelope. Results (9): list→{sessions}, detach→{sessionId}, Rest→{session: SessionSnapshot}. Events (4): server_snapshot, session_snapshot, session_progress {sessionId, progress}, session_removed. TranscriptProgress (4): item_started, assistant_delta {messageId, contentIndex, kind: text|thinking|toolCall, delta}, item_updated, item_finished (transient, nicht autoritativ).
Datentypen: TranscriptItem = user|assistant|tool (assistant-Status streaming|complete|error|aborted; tool running|complete|error); ThinkingLevel (7 inkl. off); SessionPhase (5): idle, turn, compaction, branch_summary, retry; ProtocolErrorCode (7): version, busy, session_locked, not_found, invalid_request, not_implemented, internal_error; ModelRef {provider,id}; SessionMetadata (durable); SessionSnapshot (runtime: id, name?, cwd, createdAt, updatedAt, phase, model, thinkingLevel, attached, locked, revision, transcript[], queuedSteer[], queuedSteerCount); ServerSnapshot {serverId, protocolVersion, revision, sessions[], models[]}. **Alle Schemas additionalProperties:false.**

**Transport**: Nicht im Paket. Im Repo NUR Unix-Domain-Socket (server/transports/unix, client/unix.ts). Kein stdio/WebSocket/TCP.

## 2. @notagent/server (2 299 LOC src, experimentell)
Session-Server-KERN (kein HTTP/WS, keine CLI); Anwendung muss PiServerService implementieren. **Kein Paket im Repo konsumiert ihn** (nur eigene Tests) — publiziertes, intern ungenutztes Library-Paket.
- PiServer (server.ts 396): PiServerService + listeners; maxFrameLength 16 MiB, handshakeTimeoutMs 5000, serverId randomUUID; pro Connection Stage-Automat awaitingHello→handshaking→ready→closing→closed; Handshake: erste Nachricht hello, Versionsprüfung, ServerHello mit Snapshot (+ server_snapshot-Event bei zwischenzeitlicher Revision); Fehler-Sanitizing: internal_error mit fixer Message (Ursache nur an onError); Protokollfehler → hello_error als letzter Frame.
- LiveSessionManager (sessions.ts 346): Map id→LiveSession {runtime, connections, operationCount, ready, terminal, disposing}; create: Server generiert ID (randomUUID), Service MUSS sie persistieren (verifiziert); Auto-Dispose wenn keine Connections + keine Ops + Phase idle; normalizedSnapshot überschreibt phase/attached/locked; listMetadata merged durable + live; runtime-error → terminate (alle Connections schließen).
- UnixListener (transports/unix/listener.ts 434) — subtilster Teil: bindet auf privaten Pfad `.p-<sha256(path)[0..8]>` im selben Verzeichnis, dann link() auf Zielpfad (atomare Publikation); mkdir mode 0o700, Socket chmod 0o600; Stale-Erkennung: lstat muss Socket sein, isSocketLive probiert Connect 1 s; Entfernung via rename in `.s-<uuid>` mit dev/ino-Verifikation; Cleanup analog `.c-<uuid>`; Pfadlimit 107 Bytes (Linux)/103; UnixByteConnection: serialisierte Writes, maxPendingBytes (default maxFrameLength·4), graceful close 5 000 ms. Auth: KEINE (Dateisystem-Permissions).
- protocol.ts 382: Bridge ai↔protocol mit Compile-Time-Field-Manifests (ExactKeys) — einziger Grund für die ai-Dependency.
- testing/ 466: TestServerService, ProtocolTestClient, createTestServer. Tests 1 456 LOC (conformance 378!).

## 3. @notagent/client (1 225 LOC src)
Transport-neutraler Protokoll-Client. Dateien: client.ts 432 (PiClient), connection.ts 236, state.ts 156 (Snapshot-Cache, Revisions-Guard: kleinere revision verworfen), unix.ts 156, session-handle.ts 111 (SessionLease/AsyncDisposable), errors.ts 56, types.ts 26, transport.ts 18, promise.ts 16. Tests 1 227 LOC. Dep: nur protocol.
Verhalten: Request-IDs request-<seq>; Response-Validierung result.command gegen Pending (sonst Fail); KEIN Auto-Reconnect; **Lease-System**: acquireSession(id, shared|exclusive) — exclusive scheitert bei jedem Lease, shared bei exclusive (PiSessionOwnershipError); createSession → exclusive; Zähler + Generationen; detach erst beim letzten Release; fehlgeschlagenes dispose → Cleanup-Rekonziliation bei nächster Acquisition.
**Nutzung durch coding-agent**: nur src/client/remote-session.ts 414 (RemoteSession: unbound|ready|busy|disposed; open/create/submit/abort/setModel/setThinking/reconnect) + transcript.ts 101 — **nur in Tests instanziiert, nicht im CLI-Laufzeitpfad**. Der RPC-Modus des CLI ist ein ANDERES Protokoll (JSONL/stdio).

## 4. @notagent/session-backend-sqlite-node (2 505 LOC src)
Einziges Backend unter session-backends/. SQLite-Adapter über node:sqlite (DatabaseSync). Implementiert SessionRepo + SessionStorage aus @notagent/agent-core (harness/session/types.ts:361). **Kein Paket im Repo konsumiert es** (coding-agent nutzt eigenen JSONL-SessionManager). Publiziert, intern ungenutzt.
Schema (001_initial.sql, 122 Zeilen): sessions, entries (mit rowid für FTS5), session_sequences, session_stats, branch_entries (Cache), lanes, records, lane_moves, facts, branch_tips, writer_leases — alle außer entries WITHOUT ROWID. Zur Laufzeit: migrations-Tabelle + session_search_fts (FTS5, content=entries, trigram-Tokenizer, lazy bei erster Suche, 3 Trigger). PRAGMAs: WAL, synchronous=FULL, busy_timeout 5000.
Writer-Lease: owner_id (uuidv7), monotoner fence, expires_at_ms; ttl 30 s, heartbeat 10 s; jeder Write in BEGIN IMMEDIATE-Transaktion mit Lease-Erneuerung; Verlust → SessionError. Alle Writes durch SerialOperationQueue. Suche: bm25-Ranking, DB pro search() frisch geöffnet.
API: create/open/list/delete/fork + repairBranchCache/close; Storage: getMetadata, isForSession, getLanes, createLane, moveLane, appendEntry, appendRecord, getEntry, findEntries(OnBranch), findRecords, findOpenOperations, getLog, get/setName, get/setLabel, getStats, release.

## 5. @notagent/telemetry (935 LOC src)
Vendor-neutrale Contracts, KEIN Exporter, kein globaler Current-Span. TelemetryContext {startSpan(options, callback)}, TelemetrySpan {addEvent, setAttributes, setStatus} extends TelemetryContext (Callback-basiert, explizite Parent-Weitergabe). AttributeValue: string|number|boolean|Arrays davon. Schema-Definitionen (TelemetrySchemaDefinition mit spans/events/attributes, sensitive/cardinality) — reine Typinferenz, KEINE Runtime-Validierung. InMemoryTelemetryContext (Recording, defensive Kopien, Auto-Error-Status), NOOP_TELEMETRY_CONTEXT (frozen), Konformanz-Testsuite (315 LOC).
Nutzer: ai (optionales telemetryContext in Request-Options), agent-core (harness/telemetry.ts 620: AI_TELEMETRY_SCHEMA + HARNESS_TELEMETRY_SCHEMA, Span-Namen notagent.ai.request, notagent.harness.*, notagent.session.write). **Opt-in; der coding-agent setzt nirgends einen TelemetryContext.** Nicht verwechseln mit coding-agent core/telemetry.ts (Install-Ping, opt-out).

## 6. @notagent/evals — **private:true, reines Dev-Tool, wird nicht publiziert → NICHT Teil des Ports.**

## 7. Abhängigkeitsgraph (dependencies)
telemetry → (nichts); tui → (nichts intern); protocol → (typebox); ai → telemetry; agent-core → ai, telemetry; client → protocol; server → protocol, ai; session-backend-sqlite → agent-core, ai; coding-agent → agent-core, ai, client, protocol, tui; evals (dev) → coding-agent, ai.
Build-Reihenfolge (Root package.json): tui → telemetry → ai → agent → sqlite → protocol → client → server → coding-agent.

## 8. Auslieferung
bin-Einträge: genau 2 — coding-agent: `notagent` → dist/cli.js; ai: `notagent-ai` → dist/cli.js. Publiziert werden alle non-private Pakete (9). Single-Binary via bun build --compile + Assets (Themes, PNG, export-html-Vendor, photon-WASM, 14 tree-sitter-WASMs). Alle Pakete v0.1.0, ESM, Node ≥ 22.19.

## Port-Hinweise
1. protocol exakt spezifiziert: CBOR-Subset-Ablehnungsregeln müssen exakt gespiegelt werden (Konformanztests portieren!).
2. Unix-Socket-Publikationsverfahren (.p-<hash> + link() + dev/ino-Stale-Handling) ist der subtilste Teil.
3. server + sqlite-Backend sind publizierte Blätter ohne interne Konsumenten — Port als Library-Crates, App braucht sie nicht.
4. telemetry: nur Datenstrukturen + Noop; TS-Typmaschinerie entfällt.
5. evals entfällt.
