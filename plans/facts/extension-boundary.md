# Faktenbericht: Extension-System-Grenze — was im Rust-Port wegfällt und was nativ nachgebaut werden muss

Quelle: `/Users/dev/projects/notagent-main/packages/coding-agent`. Alle Aussagen aus dem Quellcode (Stand 2026-08-13).

---

# 1. Wo das Extension-System lebt

## Kern (das eigentliche System) — `src/core/extensions/`

| Datei | LOC | Rolle |
|---|---|---|
| `src/core/extensions/types.ts` | 1728 | Komplette API-Oberfläche: `ExtensionAPI`, `ExtensionContext`, `ExtensionUIContext`, alle Event-Typen, `ToolDefinition`, `ProviderConfig`, `ExtensionRuntime` |
| `src/core/extensions/runner.ts` | 1236 | `ExtensionRunner` — Dispatch, Kontext-Fabrik, Binding, Command-/Shortcut-Auflösung |
| `src/core/extensions/loader.ts` | 734 | jiti-Loader, Discovery, `createExtensionAPI()`, Modul-Cache, `VIRTUAL_MODULES` |
| `src/core/extensions/index.ts` | 186 | Re-Export-Barrel |
| `src/core/extensions/wrapper.ts` | 45 | `wrapRegisteredTool(s)` → `AgentTool` |
| **Summe Kern** | **3929** | |

## Built-in-Extension-Implementierungen

| Datei | LOC |
|---|---|
| `src/core/permissions/extension.ts` | 107 |
| `src/core/hooks/extension.ts` | 183 |
| `src/extensions/index.ts` | 4 |
| `src/extensions/llama/index.ts` | 228 |
| `src/extensions/llama/ui.ts` | 542 |
| `src/extensions/llama/client.ts` | 332 |
| `src/extensions/llama/huggingface.ts` | 158 |
| `src/extensions/llama/provider.ts` | 150 |
| **Summe** | **1704** |

## TUI-Komponenten nur für Extensions

`src/modes/interactive/components/extension-editor.ts` (132), `extension-input.ts` (87), `extension-selector.ts` (112) — 331 LOC. **Achtung**: `ExtensionSelectorComponent` wird in `interactive-mode.ts` auch für Kern-Dialoge wiederverwendet (Zeilen 5272, 5579, 5862, 6110) — die Komponente fällt *nicht* weg, nur ihr Name ist irreführend.

Ergänzend: `src/core/event-bus.ts` (33 LOC) existiert ausschließlich als `notagent.events`-Backing.

Doku/Beispiele/Tests: `docs/extensions.md` (2992 Zeilen), `examples/extensions/**/*.ts` (14 422 LOC), Extension-Tests ≈ 5223 LOC.

## Wie Extensions geladen werden

**jiti** (loader.ts:441-461), drei Runtime-Modi (Bun-Binary mit eingebetteten VIRTUAL_MODULES, TS-Source, gebautes Node mit Alias-Map). Extension-Modul = Default-Export-Factory `(notagent: ExtensionAPI) => void|Promise<void>`.

**Discovery** (loader.ts:686-734), Reihenfolge: 1. `<cwd>/.notagent/extensions/`, 2. `<agentDir>/extensions/`, 3. explizit konfigurierte Pfade (Settings + `--extension/-e`). Pro Verzeichnis: direkte *.ts/*.js; Unterordner mit index.ts/js; Unterordner mit package.json + notagent.extensions-Manifest. Modul-Cache invalidiert bei cwd-Wechsel.

## Extension-API-Oberfläche (types.ts:1198-1437)

**31 Event-Namen via `notagent.on(...)`:** project_trust, resources_discover, session_start, session_info_changed, session_before_switch, session_before_fork, session_before_compact, session_compact, session_shutdown, session_before_tree, session_tree, context, before_provider_request, before_provider_headers, after_provider_response, before_agent_start, agent_start, agent_end, agent_settled, turn_start, turn_end, message_start, message_update, message_end, tool_execution_start, tool_execution_update, tool_execution_end, model_select, thinking_level_select, tool_call, tool_result, user_bash, input.

Events mit Rückgabewirkung: project_trust → Trust-Entscheidung; resources_discover → zusätzliche Skill/Prompt/Theme-Pfade; session_before_* → cancel/eigene Compaction/Summary; context → ersetzt Message-Array; before_provider_request/headers → Payload/Header-Rewrite; before_agent_start → Message-Injektion + System-Prompt-Ersatz; message_end → Message-Ersatz; tool_call → {block, reason, terminate}; tool_result → Ersatz; user_bash → operations/result; input → continue|transform|handled.

**Registrierbar:** registerTool (ToolDefinition mit execute, renderCall, renderResult, promptSnippet, promptGuidelines, constrainedSampling, executionMode, prepareArguments, renderShell), registerCommand, registerShortcut, registerFlag/getFlag, registerMessageRenderer, registerEntryRenderer, registerMarkdownTransformer, registerProvider/unregisterProvider (inkl. OAuth login/refreshToken/getApiKey, streamSimple, refreshModels).

**Aktionen:** sendMessage, sendUserMessage, appendEntry, set/getSessionName, setLabel, exec, get/set ActiveTools, getCommands, setModel, get/setThinkingLevel, events (EventBus).

**ExtensionContext:** ui, mode (tui|rpc|json|print), hasUI, cwd, sessionManager, modelRegistry, model, scopedModels, thinkingLevel, isIdle(), isProjectTrusted(), signal, abort(), hasPendingMessages(), shutdown(), getContextUsage(), compact(), getSystemPrompt(). **ExtensionCommandContext zusätzlich:** getSystemPromptOptions(), waitForIdle(), newSession(), fork(), navigateTree(), switchSession(), reload().

**ExtensionUIContext (27 Methoden):** select, confirm, input, notify, onTerminalInput, setStatus, setWorkingMessage, setWorkingVisible, setWorkingIndicator, setHiddenThinkingLabel, setWidget, setFooter, setHeader, setTitle, custom, pasteToEditor, setEditorText, getEditorText, editor, addAutocompleteProvider, setEditorComponent, getEditorComponent, theme, getAllThemes, getTheme, setTheme, getToolsExpanded, setToolsExpanded.

---

# 2. KRITISCH: Built-in-Features, die intern als Extensions implementiert sind

Registriert in `src/main.ts:638-643` als `InlineExtension[]`: permissions (hidden), hooks (hidden), llama.cpp (hidden), plus SDK-injizierte. Für Nutzer sind das Kernfunktionen.

### 2.1 Permission-System — `src/core/permissions/extension.ts` (107 LOC)

Registriert drei Handler: `tool_call` → `createPermissionHandler(...)` (Policy-Chain aus chain.ts/policies.ts, `ApprovalCoordinator`, Session-Approval-History), gibt bei Ablehnung `{block: true, reason, terminate: true}` zurück; `session_start` → coordinator.reset(); `session_shutdown` → coordinator.abort().

Kommentar in main.ts:588-591: Reihenfolge sicherheitsrelevant, `emitToolCall` bricht beim ersten blockierenden Handler ab (runner.ts:932-953). **Das gesamte Approval-/YOLO-/Auto-Modus-Verhalten hängt am tool_call-Hook.** Rust-Port: nativer Pre-Tool-Gate im Agent-Loop.

Abhängige Kernmodule: `core/permissions/{chain,coordinator,hook,policies,policy,request,user-rules}.ts`.

### 2.2 Hooks-System (Claude-Code-kompatible User-Hooks) — `src/core/hooks/extension.ts` (183 LOC)

Mapping Extension-Event → Hook-Name: session_start→SessionStart, session_shutdown→SessionEnd, before_agent_start→UserPromptSubmit (Ergebnis als custom_type "hook_context"-Message injiziert), agent_end+agent_settled→Stop/StopFailure/Interrupt+Notification, tool_result→PostToolUse/PostToolUseFailure, session_before_compact→PreCompact, session_compact→PostCompact. `PreToolUse` läuft über den Permission-Pfad (decide-Callback main.ts:634-636). `createApprovalObserver()` emittiert PermissionRequest/PermissionResult/Notification.

Kernmodule: `core/hooks/{events,hooks,payload,runner,runtime}.ts`.

### 2.3 llama.cpp-Provider + /llama-Command — `src/extensions/llama/` (1410 LOC)

`registerProvider` (nativer @notagent/ai-Provider für lokale llama.cpp-Server, LLAMA_PROVIDER_ID = "llama.cpp", OpenAI-Completions-API, dynamischer Modellkatalog via refreshModels) + `registerCommand("llama", ...)` (TUI zum Laden/Entladen/Downloaden von Modellen inkl. HuggingFace-Suche). **Einziger Ort, an dem registerProvider/registerCommand produktiv im Kern verwendet wird.**

### 2.4 Negativ-Befunde (nativ, NICHT über Extension-API)

- **Slash-Commands** des Kerns: `core/slash-commands.ts`, Prompt-Templates, Skills — Extension-Commands nur eine von drei `SlashCommandSource`-Varianten (extension | prompt | skill)
- **Themes**: `modes/interactive/theme/`, geladen von `core/resource-loader.ts`
- **Tools**: bash, read, edit, write, grep, find, ls, skill, task, task-tools, todo-write nativ in `core/tools/`
- **Subagents/Task-Delegation**: `core/tasks/`, `core/delegation/` nativ
- **Provider allgemein**: `core/model-registry.ts`, `core/model-runtime.ts`, `core/provider-composer.ts` nativ

**Strukturelle Kopplung, die der Rust-Port ersetzen muss:** Alle Built-in-Tools sind mit dem Typ `ToolDefinition` aus `core/extensions/types.ts` definiert (`core/tools/index.ts:129`) und laufen durch `wrapToolDefinition(definition, ctxFactory)` (`core/tools/tool-definition-wrapper.ts`), das ihnen einen `ExtensionContext` reicht. In `agent-session.ts:2905-2915` werden auch Built-ins durch `wrapRegisteredTools(..., runner)` geschickt. Runtime-Nutzung des Kontexts aktuell nur in `core/tools/bash.ts:240-257` (Env-Vars NOTAGENT_SESSION_ID, Session-File, NOTAGENT_REASONING_LEVEL). Rust-Port: äquivalenter Tool-Kontext ohne Extension-Infrastruktur.

---

# 3. Integrationspunkte (Kern → Extension-System)

### Laden/Lifecycle
- `core/resource-loader.ts:547,563,579,609,957,571-621` (loadCurrentExtensionSet, Inline-Factories, zweiphasiges Laden um Project-Trust)
- `main.ts:616-643,664,677,932-935` (Factory-Liste, package/config-Command, Extension-Flags in --help)
- `core/agent-session.ts:3018-3029,2629-2661,3066-3087` (Runner-Erzeugung, bindExtensions → session_start + extendResourcesFromExtensions, reload())
- `core/agent-session-services.ts:84-183` (Flag-Werte, pendingProviderRegistrations)
- `core/project-trust.ts:55` (emitProjectTrustEvent)

### Agent-Loop (`core/agent-session.ts`)
Zeile 553 emitToolCall (als agent.beforeToolCall), 570 emitToolResult (agent.afterToolCall), 665 agent_settled, 799/801 agent_start/end, 808/821 turn_start/end, 828/835/841 message_start/update/end (ersetzt Message in-place), 862/871/880 tool_execution_*, 1511 emitInput, 1621 emitBeforeAgentStart, 1672-1714/1804 Extension-Command-Dispatch, 1961/2088/3342/3536 session_before_switch/info_changed/model_select+thinking/session_tree, 2207/2278 + 2476/2561 session_before_compact/compact (2 Pfade), 2650 session_start, 2659 emitResourcesDiscover, 2863-2915 Tool-Registry-Refresh mit wrapRegisteredTools, 3417 session_before_tree, 2731-2855 _bindExtensionCore (14 Actions + 12 Context-Actions + 3 Provider-Actions).

### Provider-/HTTP-Pfad (`core/sdk.ts`)
330 emitBeforeProviderHeaders (transformHeaders), 340 emitBeforeProviderRequest (Agent.onPayload), 347 after_provider_response (Agent.onResponse), 357 emitContext (Agent.transformContext). Verbunden über `extensionRunnerRef` (sdk.ts:295).

### Runtime-Host (`core/agent-session-runtime.ts`)
142/159 session_before_switch/fork; 171/399 emitSessionShutdownEvent (reason new|resume|fork|quit).

### TUI (`modes/interactive/interactive-mode.ts`, 6688 LOC)
1833 bindExtensions, 2360-2413 createExtensionUIContext (27 Methoden), 1998-2053 Shortcuts, 1992 MarkdownTransformers, 3689/3728 Entry-/MessageRenderer, 662-672 Command-Konflikt-Diagnostik, 730-756 Autocomplete, 1657-1748 Startup-Anzeige, 1794-1812 Diagnostics, 4510-4590 Queue-Sonderbehandlung, 6474-6484 Hilfe, 6575 emitUserBash, 2123-2340 Widgets/Footer/Header/Status, 2588-2660 setEditorComponent.

### RPC/Print
- `modes/rpc/rpc-mode.ts:136-280` implementiert ExtensionUIContext über Wire-Protokoll; rpc-types.ts:238-282 (RpcExtensionUIRequest/Response); Event extension_error (349); get_commands mit source "extension" (681-685); emitUserBash (560).
- `modes/print-mode.ts:76`: bindExtensions ohne uiContext → noOpUIContext (runner.ts:235-266).

### Andere Pakete — KEINE Abhängigkeit
server/client/protocol: keine Fundstelle "extension", importieren coding-agent nicht. ai: nur Legacy-Kompatibilitätstypen (`ai/src/compat/extension-oauth-types.ts`).

---

# 4. examples/extensions/ (118 Dateien, 14 422 LOC)

Kategorien: Lifecycle/Safety (permission-gate, project-trust, protected-paths, sandbox/, gondolin/), Custom Tools (subagent/ 1033 LOC, ssh.ts, …), Commands/UI (plan-mode/, doom-overlay/, modal-editor, …), Git (git-checkpoint, …), System-Prompt/Compaction, Custom Provider (custom-provider-anthropic/, custom-provider-gitlab-duo/), Resources, Messages, Session-Metadaten. Fällt komplett weg.

---

# 5. SDK-/RPC-Abhängigkeit vom Extension-System

- **SDK-Modus: ja, stark** — `core/sdk.ts` verdrahtet 4 Hooks in die Agent-Instanz; `MainOptions.extensionFactories` ist der offizielle SDK-Injektionsweg.
- **RPC-Modus: ja** — eigene Wire-Typen für ExtensionUIContext (fallen im Port weg oder bleiben als reservierte Nachrichtentypen undokumentiert).
- **Print/JSON: minimal** — noOp-UIContext.
- **Andere Pakete: nein.**

---

# 6. Features NUR über Extensions (Verlustliste)

**Muss im Rust-Port NATIV nachgebaut werden (sind Kern-Features der App):**
1. **Permission-/Approval-System** komplett — inkl. --auto/--yolo-Semantik, Policy-Chain, Session-Approval-History.
2. **Hooks-System** — 16 Claude-Code-kompatible Hook-Namen; PreToolUse über Permission-Pfad.
3. **llama.cpp-Provider + /llama-Command** — einziger eingebauter lokaler Modell-Provider mit Modell-Management-UI und HuggingFace-Download.

**Entfällt ersatzlos (nur über Nutzer-Extensions verfügbar):** Custom Provider via registerProvider (inkl. OAuth), Sandboxing (sandbox/, gondolin/), Plan-Mode, SSH-Remote-Tools, Custom Editor/Vim-Modus, Custom Footer/Header/Widgets/Overlays, Custom Autocomplete-Provider, Custom Compaction, Custom Branch-Summary, Provider-Payload/Header-Manipulation, Input-Transformation, Message/Entry/Markdown-Renderer-Overrides, Extension-CLI-Flags und -Shortcuts, dynamische Ressourcen via resources_discover, Project-Trust-Übernahme, Inter-Extension-EventBus.

**Mit-abhängige Infrastruktur:** `core/package-manager.ts` (2677 LOC) verwaltet ResourceType = extensions | skills | prompts | themes — die CLI-Kommandos `notagent install/remove/list/update` behalten skills/prompts/themes, verlieren extensions. `--extension/-e` und `--no-extensions/-ne` in `cli/args.ts:166-169` entfallen, ebenso EXTENSION_LOAD_FAILURE_HINT (main.ts:85).
