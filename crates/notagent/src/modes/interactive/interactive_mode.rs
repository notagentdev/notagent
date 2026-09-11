use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notagent_agent::types::{AgentEvent, AgentMessage, AgentToolResult};
use notagent_ai::auth::types::{
    AuthCheck, AuthEvent, AuthOperationOptions, AuthPrompt, AuthPromptKind, AuthType,
};
use notagent_ai::compat::extension_oauth_types::OAuthDeviceCodeInfo;
use notagent_ai::providers::mtplx;
use notagent_ai::types::{
    AssistantMessage, ImageContent, Model, StopReason, TextContent, TextOrImageContent,
    ToolResultMessage, UserContent, UserMessage,
};
use notagent_tui::autocomplete::{
    ArgumentCompletions, AutocompleteItem, CombinedAutocompleteProvider, CommandEntry, SlashCommand,
};
use notagent_tui::components::markdown::Markdown;
use notagent_tui::components::scroll_view::{Overscroll, ScrollView, ScrollViewOptions};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::stack::{StackEntryOptions, StackOptions};
use notagent_tui::components::text::Text;
use notagent_tui::components::truncated_text::TruncatedText;
use notagent_tui::components::v_stack::VStack;
use notagent_tui::editor_component::EditorComponent;
use notagent_tui::fuzzy::fuzzy_filter;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::layout_node::StackBasis;
use notagent_tui::terminal::{ProcessTerminal, Terminal, TerminalPump};
use notagent_tui::tui::{
    Component, ComponentRef, Container, Line, RenderLoop, TuiCore, TuiStopOptions, component_ref,
};
use notagent_tui::tui_alt_screen::TuiAltScreen;
use notagent_tui::tui_main_screen::TuiMainScreen;

use crate::config::{
    APP_NAME, APP_TITLE, CONFIG_DIR_NAME, InstallEnv, InstallMethod, PROVIDER_DOCUMENTATION_URL,
    VERSION, detect_install_method, get_agent_dir, get_auth_path,
};
use crate::core::agent_session::{
    AgentSession, AgentSessionEvent, CompactionReason, ExecuteBashOptions, NavigateTreeOptions,
    PromptOptions, QueueBehavior, parse_skill_block,
};
use crate::core::agent_session_runtime::{AgentSessionRuntime, ForkPosition};
use crate::core::bash_executor::BashResult;
use crate::core::cache_stats::{CACHE_TTL_MS, CacheMiss, detect_cache_miss};
use crate::core::diagnostics::{DiagnosticLevel, ResourceDiagnostic};
use crate::core::hooks::runtime::{HookReportLevel, HookReporter};
use crate::core::http_dispatcher::format_http_idle_timeout_ms;
use crate::core::keybindings::KeybindingsManager;
use crate::core::messages::{CustomMessage, create_compaction_summary_message};
use tokio_util::sync::CancellationToken;

use crate::core::agent_session_runtime::SessionOpenError;
use crate::core::export_html::tool_renderer::ToolDefinitionHtmlRenderer;
use crate::core::model_registry::ModelRegistry;
use crate::core::model_resolver::{
    ModelScopeDiagnosticCode, default_model_for_provider, find_exact_model_reference_match,
    resolve_model_scope_from_models,
};
use crate::core::modes::indicator::format_mode_switch_notice;
use crate::core::package_manager::{DefaultPackageManager, PackageManagerOptions};
use crate::core::permissions::coordinator::ApprovalPresenter;
use crate::core::permissions::request::{ApprovalAnswer, ApprovalRequest};
use crate::core::session_cwd::{MissingSessionCwdError, format_missing_session_cwd_prompt};
use crate::core::session_manager::{
    SessionEntry, SessionInfo, SessionManager, session_entry_to_context_messages,
};
use crate::core::settings_manager::{DoubleEscapeAction, FullscreenExitOutput, TuiMode};
use crate::core::slash_commands::BUILTIN_SLASH_COMMANDS;
use crate::core::source_info::{SourceInfo, SourceScope};
use crate::core::tasks::lifecycle::{TASK_LIFECYCLE_ENTRY_TYPE, TaskLifecycleRecord};
use crate::core::tasks::manager::TaskManager;
use crate::core::tasks::types::{TaskInfo, TaskStatus};
use crate::core::todos::Todo;
use crate::core::tools::truncate::TruncationResult;
use crate::core::trust_manager::ProjectTrustStore;
use crate::core::trust_manager::has_trust_requiring_project_resources;
use crate::modes::interactive::components::approval_selector::ApprovalSelectorComponent;
use crate::modes::interactive::components::armin::ArminComponent;
use crate::modes::interactive::components::assistant_message::AssistantMessageComponent;
use crate::modes::interactive::components::bash_execution::BashExecutionComponent;
use crate::modes::interactive::components::bordered_loader::BorderedLoader;
use crate::modes::interactive::components::branch_summary_message::BranchSummaryMessageComponent;
use crate::modes::interactive::components::compaction_summary_message::CompactionSummaryMessageComponent;
use crate::modes::interactive::components::custom_editor::CustomEditor;
use crate::modes::interactive::components::custom_message::CustomMessageComponent;
use crate::modes::interactive::components::daxnuts::DaxnutsComponent;
use crate::modes::interactive::components::dynamic_border::DynamicBorder;
use crate::modes::interactive::components::earendil_announcement::EarendilAnnouncementComponent;
use crate::modes::interactive::components::explore_block::{
    ExploreBlockComponent, is_explore_tool,
};
use crate::modes::interactive::components::footer::{FooterComponent, format_tokens};
use crate::modes::interactive::components::keybinding_hints::key_display_text;
use crate::modes::interactive::components::list_selector::ListSelectorComponent;
use crate::modes::interactive::components::login_dialog::{LoginCancelled, LoginDialogComponent};
use crate::modes::interactive::components::model_selector::{
    ModelRefreshOutcome, ModelSelectorComponent,
};
use crate::modes::interactive::components::oauth_selector::{
    AuthSelectorMethod, AuthSelectorMode, AuthSelectorProvider, OAuthSelectorComponent,
};
use crate::modes::interactive::components::scoped_models_selector::{
    ModelsCallbacks, ModelsConfig, RefreshStatusKind, ScopedModelsSelectorComponent,
};
use crate::modes::interactive::components::session_selector::{
    LoadRequest, SessionScope, SessionSelectorComponent, SessionSelectorOptions,
};
use crate::modes::interactive::components::settings_selector::{
    SettingsCallbacks, SettingsConfig, SettingsSelectorComponent,
};
use crate::modes::interactive::components::side_question_panel::{
    SideQuestionPanel, SideQuestionPanelOptions,
};
use crate::modes::interactive::components::skill_invocation_message::SkillInvocationMessageComponent;
use crate::modes::interactive::components::startup_header::{StartupHeader, StartupHeaderData};
use crate::modes::interactive::components::status_indicator::{
    CompactionStatusReason, IdleStatus, StatusIndicator, StatusIndicatorKind,
};
use crate::modes::interactive::components::subagent_panel::SubagentPanel;
use crate::modes::interactive::components::task_lifecycle::{
    is_background_bash_call, task_lifecycle_line,
};
use crate::modes::interactive::components::tasks_browser::{
    TasksBrowserComponent, TasksBrowserProps, TasksFilter,
};
use crate::modes::interactive::components::text_input_dialog::{
    TextInputDialogComponent, TextInputDialogOptions,
};
use crate::modes::interactive::components::thinking_selector::ThinkingSelectorComponent;
use crate::modes::interactive::components::to_locale_string;
use crate::modes::interactive::components::todo_list::{
    TodoListComponent, TodoListMode, TodoVisibility,
};
use crate::modes::interactive::components::tool_execution::{
    ToolExecutionComponent, ToolExecutionOptions, ToolExecutionResult,
};
use crate::modes::interactive::components::tree_selector::{
    TreeSelectorComponent, TreeSelectorOptions,
};
use crate::modes::interactive::components::trust_selector::{
    TrustSelection, TrustSelectorComponent, TrustSelectorOptions,
};
use crate::modes::interactive::components::user_message::UserMessageComponent;
use crate::modes::interactive::components::user_message_selector::{
    UserMessageItem, UserMessageSelectorComponent,
};
use crate::modes::interactive::external_editor::{
    ExternalEditorOptions, ExternalEditorResult, edit_in_external_editor,
};
use crate::modes::interactive::llama_command::{
    LLAMA_COMMAND_DESCRIPTION, LlamaCommand, Notify, NotifyLevel, create_llama_ui,
    llama_client_for_command, run_llama_command,
};
use crate::modes::interactive::model_search::{ModelSearchItem, get_model_search_text};
use crate::modes::interactive::theme::theme::{
    BlockStyle, ThemeBg, ThemeColor, block_style, get_available_themes, get_editor_theme,
    get_markdown_theme, on_theme_change, set_registered_themes, theme,
};
use crate::modes::interactive::theme::theme_controller::InteractiveThemeController;
use crate::utils::abort::timeout_signal;
use crate::utils::tools_manager::{ManagedTool, ensure_tool};
use crate::utils::version_check::{LatestPiRelease, check_for_new_pi_version};

// ============================================================================
// ============================================================================

/// A terminal handed to the interactive mode from outside, together with the
/// pump that feeds it.
/// the handle the TUI writes to and the pump the render loop drives
pub struct InteractiveTerminal {
    pub terminal: Box<dyn Terminal>,
    pub pump: Box<dyn TerminalPump>,
}

/// Options for the interactive mode — `InteractiveModeOptions`
#[derive(Default)]
pub struct InteractiveModeOptions {
    /// Providers that were migrated to `auth.json` (shows a warning).
    pub migrated_providers: Vec<String>,
    /// Warning shown when the session model could not be restored.
    pub model_fallback_message: Option<String>,
    /// Cwd to trust after a reload if it gained a `.notagent` directory during
    /// this implicitly trusted session.
    pub auto_trust_on_reload_cwd: Option<String>,
    /// owns the process signals and cancels this token on SIGTERM/SIGHUP; the
    /// mode then runs the `fromSignal` shutdown on its own thread, where the
    /// terminal lives.
    pub shutdown_signal: Option<CancellationToken>,
    /// Initial message sent on startup (may carry `@file` content).
    pub initial_message: Option<String>,
    /// Images attached to the initial message.
    pub initial_images: Vec<ImageContent>,
    /// Further messages sent after the initial one.
    pub initial_messages: Vec<String>,
    /// Force a verbose startup (overrides the `quietStartup` setting).
    pub verbose: bool,
    /// TUI layout mode.
    pub tui_mode: Option<TuiMode>,
    /// Only tests: the terminal and its pump instead of a `ProcessTerminal`.
    pub terminal: Option<InteractiveTerminal>,
}

/// Which queue a message waiting for the end of a compaction belongs to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionQueueMode {
    Steer,
    FollowUp,
}

struct CompactionQueuedMessage {
    text: String,
    mode: CompactionQueueMode,
}

/// The half of a settings change that only the mode can carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SettingsEffect {
    AutoCompact(bool),
    ShowImages(bool),
    ImageWidthCells(u64),
    RebuildAutocomplete,
    HttpIdleTimeout(u64),
    ThemeApplied,
    HideThinkingBlock(bool),
    InvalidateChat,
    RebuildChat,
    ShowHardwareCursor(bool),
    EditorPaddingX(u64),
    OutputPad(u64),
    AutocompleteMaxVisible(u64),
    ClearOnShrink(bool),
    TuiMode(TuiMode),
    WorkspaceInFooter(bool),
}

/// A permission request on its way to the dialog.
type ApprovalRequestMessage = (
    ApprovalRequest,
    tokio::sync::oneshot::Sender<ApprovalAnswer>,
);
type ApprovalSender = tokio::sync::mpsc::UnboundedSender<ApprovalRequestMessage>;

/// The `AuthInteraction` the login flow gets: it posts into the loop and waits
/// for the dialog's answer.
struct LoopAuthInteraction {
    tx: tokio::sync::mpsc::UnboundedSender<UiMessage>,
    signal: tokio_util::sync::CancellationToken,
}

impl notagent_ai::auth::types::AuthInteraction for LoopAuthInteraction {
    fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        Some(self.signal.clone())
    }

    fn prompt(
        &self,
        prompt: AuthPrompt,
    ) -> futures::future::BoxFuture<'_, Result<String, notagent_ai::auth::types::AuthError>> {
        let tx = self.tx.clone();
        Box::pin(async move {
            let (answer_tx, answer_rx) = tokio::sync::oneshot::channel();
            if tx
                .send(UiMessage::AuthPrompt {
                    prompt: Box::new(prompt),
                    answer: answer_tx,
                })
                .is_err()
            {
                return Err(notagent_ai::auth::types::AuthError(
                    "Login cancelled".to_owned(),
                ));
            }
            match answer_rx.await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(message)) => Err(notagent_ai::auth::types::AuthError(message)),
                Err(_) => Err(notagent_ai::auth::types::AuthError(
                    "Login cancelled".to_owned(),
                )),
            }
        })
    }

    fn notify(&self, event: AuthEvent) {
        let _ = self.tx.send(UiMessage::AuthEvent {
            event: Box::new(event),
        });
    }
}

/// What the task browser posted into the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TaskBrowserAction {
    Select(String),
    ToggleFilter,
    Refresh,
    Output { request: u64, output: String },
    Stop(String),
    StopRefused(String),
    ClearNotice,
}

#[derive(Clone)]
struct TasksBrowserState {
    filter: TasksFilter,
    selected_task_id: Option<String>,
    output: Option<String>,
    output_loading: bool,
    notice: Option<String>,
    notice_until: Option<Instant>,
    request: u64,
}

/// The selector that currently sits where the editor is.
struct ActiveSelector {
    id: u64,
    dispose: Option<Box<dyn FnOnce()>>,
}

/// A bash run the main loop drives, with what its tail needs.
struct PendingBash {
    command: String,
    exclude_from_context: bool,
    run: Pin<Box<dyn Future<Output = BashResult>>>,
}

/// What Escape does while a compaction or a retry is running
/// (`autoCompactionEscapeHandler`/`retryEscapeHandler`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeTarget {
    Default,
    Compaction,
    Retry,
    BranchSummary,
}

/// A session list the selector asked for, and the load itself.
type RunningSessionLoad = (LoadRequest, Pin<Box<dyn Future<Output = Vec<SessionInfo>>>>);

/// The prompt the main loop currently drives together with the text to restore
/// if preflight refuses it.
type PromptFuture = Pin<Box<dyn Future<Output = (String, Result<(), String>)>>>;

/// A manual compaction the main loop drives beside its UI and session events.
type CompactionFuture = Pin<Box<dyn Future<Output = ()>>>;

struct ActiveCompactionChatIndicator {
    spacer: ComponentRef,
    indicator: ComponentRef,
}

/// What [`create_interactive_mode`] hands back: the three pieces a caller needs
/// to run the mode on its own render loop.
pub struct InteractiveModeHandle {
    /// `permissionPresent` — shows the approval dialog and answers the gate.
    pub approval_presenter: ApprovalPresenter,
    /// `hookReport` — a hook diagnostic as an error or warning in the transcript.
    pub report: HookReporter,
    /// The renderer, to be driven with `notagent_tui::tui::run_until`. It stays
    /// valid across a fullscreen switch, which replaces the renderer behind it.
    pub renderer: Box<dyn RenderLoop>,
    /// The pump belonging to the terminal — the caller's own pump when one was
    /// passed in, otherwise the pump of the freshly created `ProcessTerminal`.
    pub pump: Box<dyn TerminalPump>,
    /// The mode itself; resolves with the process exit code.
    pub run: Pin<Box<dyn Future<Output = i32>>>,
}

/// Build the interactive mode over `runtime`.
/// together with the constructor of `InteractiveMode`. Nothing is drawn yet;
/// the first frame goes out when the returned future is driven.
pub fn create_interactive_mode(
    runtime: Arc<AgentSessionRuntime>,
    mut options: InteractiveModeOptions,
) -> InteractiveModeHandle {
    let (terminal, pump): (Box<dyn Terminal>, Box<dyn TerminalPump>) = match options.terminal.take()
    {
        Some(seam) => (seam.terminal, seam.pump),
        None => {
            let (terminal, pump) = ProcessTerminal::new().into_shared();
            (Box::new(terminal), Box::new(pump))
        }
    };
    let mode = InteractiveMode::new(runtime, options, terminal);
    let renderer = mode.renderer();
    let approval_presenter = mode.approval_presenter();
    let report = mode.reporter();
    InteractiveModeHandle {
        approval_presenter,
        report,
        renderer: Box::new(renderer),
        pump,
        run: Box::pin(async move {
            let mut mode = mode;
            mode.run().await
        }),
    }
}

// ============================================================================
// The renderer the caller drives
// ============================================================================

/// The renderer of the interactive mode.
/// `TuiMainScreen` today; the alternate screen joins it with the fullscreen
/// slice, which is why the renderer already lives behind a cell that can be
/// swapped underneath a caller holding [`InteractiveRenderer`].
enum ActiveRenderer {
    Main(TuiMainScreen),
    // Boxed: the alternate screen carries its own frame buffers and dwarfs the
    // main-screen variant.
    Alt(Box<TuiAltScreen>),
}

struct RendererState {
    renderer: ActiveRenderer,
    /// Bumped whenever the renderer is replaced, so a handle notices.
    generation: u64,
    terminal: SharedTerminal,
}

/// The terminal, shared between the renderer that has it now and the one a
/// fullscreen switch replaces it with.
/// same object to the next renderer; a `Box<dyn Terminal>` cannot be read out
/// of the core, so the mode keeps the terminal itself and gives every renderer
/// a handle onto it.
#[derive(Clone)]
struct SharedTerminal(Rc<RefCell<Box<dyn Terminal>>>);

#[async_trait::async_trait(?Send)]
impl Terminal for SharedTerminal {
    fn start(
        &mut self,
        on_input: notagent_tui::terminal::InputHandler,
        on_resize: notagent_tui::terminal::ResizeHandler,
    ) {
        self.0.borrow_mut().start(on_input, on_resize);
    }

    fn stop(&mut self) {
        self.0.borrow_mut().stop();
    }

    // The drain runs at shutdown, when nothing else touches the terminal; the
    // borrow is deliberately held across the await, because the inner terminal
    // owns the stdin lease it drains.
    #[allow(clippy::await_holding_refcell_ref)]
    async fn drain_input(&mut self, max_ms: Option<u64>, idle_ms: Option<u64>) {
        let mut terminal = self.0.borrow_mut();
        terminal.drain_input(max_ms, idle_ms).await;
    }

    fn write(&mut self, data: &str) {
        self.0.borrow_mut().write(data);
    }

    fn columns(&self) -> usize {
        self.0.borrow().columns()
    }

    fn rows(&self) -> usize {
        self.0.borrow().rows()
    }

    fn kitty_protocol_active(&self) -> bool {
        self.0.borrow().kitty_protocol_active()
    }

    fn move_by(&mut self, lines: isize) {
        self.0.borrow_mut().move_by(lines);
    }

    fn hide_cursor(&mut self) {
        self.0.borrow_mut().hide_cursor();
    }

    fn show_cursor(&mut self) {
        self.0.borrow_mut().show_cursor();
    }

    fn clear_line(&mut self) {
        self.0.borrow_mut().clear_line();
    }

    fn clear_from_cursor(&mut self) {
        self.0.borrow_mut().clear_from_cursor();
    }

    fn clear_screen(&mut self) {
        self.0.borrow_mut().clear_screen();
    }

    fn set_title(&mut self, title: &str) {
        self.0.borrow_mut().set_title(title);
    }

    fn set_progress(&mut self, active: bool) {
        self.0.borrow_mut().set_progress(active);
    }
}

fn renderer_mode(renderer: &ActiveRenderer) -> TuiMode {
    match renderer {
        ActiveRenderer::Main(_) => TuiMode::Regular,
        ActiveRenderer::Alt(_) => TuiMode::Fullscreen,
    }
}

/// Shared handle on the renderer; the mode holds one, the caller another.
#[derive(Clone)]
struct RendererCell(Rc<RefCell<RendererState>>);

impl RendererCell {
    fn new(renderer: ActiveRenderer, terminal: SharedTerminal) -> Self {
        Self(Rc::new(RefCell::new(RendererState {
            renderer,
            generation: 0,
            terminal,
        })))
    }

    fn generation(&self) -> u64 {
        self.0.borrow().generation
    }

    fn core(&self) -> TuiCore {
        let state = self.0.borrow();
        match &state.renderer {
            ActiveRenderer::Main(screen) => screen.core().clone(),
            ActiveRenderer::Alt(screen) => screen.core().clone(),
        }
    }

    fn start(&self) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.start(),
            ActiveRenderer::Alt(screen) => screen.start(),
        }
    }

    fn stop(&self, options: TuiStopOptions) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.stop(options),
            ActiveRenderer::Alt(screen) => screen.stop(options),
        }
    }

    /// `ui.requestRender(force)` — the forcing variant resets the differential
    /// render state, which `/reload` and the external editor need.
    fn request_render(&self, force: bool) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.request_render(force),
            ActiveRenderer::Alt(screen) => screen.request_render(force),
        }
    }

    fn render_pending_frame(&self) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.render_pending_frame(),
            ActiveRenderer::Alt(screen) => screen.render_pending_frame(),
        }
    }

    fn render_now(&self, force: bool) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.render_now(force),
            ActiveRenderer::Alt(screen) => screen.render_now(force),
        }
    }

    /// The fullscreen layout root, which only the alternate screen uses.
    fn set_layout_root(&self, root: Option<ComponentRef>) {
        let mut state = self.0.borrow_mut();
        if let ActiveRenderer::Alt(screen) = &mut state.renderer {
            screen.set_layout_root(root);
        }
    }

    /// `ui.flash(message)` — only the alternate screen has one.
    fn flash(&self, message: &str) -> bool {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(_) => false,
            ActiveRenderer::Alt(screen) => {
                screen.flash(message, None);
                true
            }
        }
    }

    /// Replace the renderer with one for `mode`, keeping the terminal, the
    /// children and the focus (`switchTuiMode`).
    fn switch(&self, mode: TuiMode, log_directory: std::path::PathBuf) -> bool {
        let mut state = self.0.borrow_mut();
        if mode == renderer_mode(&state.renderer) {
            return true;
        }
        let core = match &state.renderer {
            ActiveRenderer::Main(screen) => screen.core().clone(),
            ActiveRenderer::Alt(screen) => screen.core().clone(),
        };
        if core.has_overlay_entries() {
            return false;
        }

        let children = core.children();
        let focus = core.get_focused_component();
        let show_hardware_cursor = core.get_show_hardware_cursor();
        let clear_on_shrink = core.get_clear_on_shrink();
        let main_render_state = match &state.renderer {
            ActiveRenderer::Main(screen) => Some(screen.capture_render_state()),
            ActiveRenderer::Alt(_) => None,
        };

        // The outgoing renderer gives the terminal back without clearing the
        // screen, so the incoming one can take it over.
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.stop(TuiStopOptions {
                preserve_screen: true,
            }),
            ActiveRenderer::Alt(screen) => screen.stop(TuiStopOptions {
                preserve_screen: true,
            }),
        }
        core.set_focus(None);
        core.clear();
        if let ActiveRenderer::Alt(screen) = &mut state.renderer {
            screen.set_layout_root(None);
        }
        let terminal: Box<dyn Terminal> = Box::new(state.terminal.clone());

        state.renderer = match mode {
            TuiMode::Fullscreen => ActiveRenderer::Alt(Box::new(TuiAltScreen::new(
                terminal,
                notagent_tui::tui_alt_screen::TuiAltScreenOptions {
                    search_match_style: Rc::new(|text: &str| {
                        theme().bg(
                            ThemeBg::SearchMatchBg,
                            &theme().fg(ThemeColor::SearchMatchText, text),
                        )
                    }),
                    search_current_match_style: Rc::new(|text: &str| {
                        theme().bold(&theme().inverse(&theme().bg(
                            ThemeBg::SearchMatchBg,
                            &theme().fg(ThemeColor::SearchMatchText, text),
                        )))
                    }),
                    ..notagent_tui::tui_alt_screen::TuiAltScreenOptions::default()
                },
            ))),
            TuiMode::Regular => {
                let mut screen = TuiMainScreen::with_options(
                    terminal,
                    Some(show_hardware_cursor),
                    Some(log_directory),
                );
                if let Some(render_state) = main_render_state {
                    screen.restore_render_state(render_state);
                }
                ActiveRenderer::Main(screen)
            }
        };
        state.generation += 1;

        let next_core = match &state.renderer {
            ActiveRenderer::Main(screen) => screen.core().clone(),
            ActiveRenderer::Alt(screen) => screen.core().clone(),
        };
        next_core.set_clear_on_shrink(clear_on_shrink);
        next_core.set_show_hardware_cursor(show_hardware_cursor);
        for child in children {
            next_core.add_child(child);
        }
        next_core.invalidate();
        next_core.set_focus(focus);
        true
    }

    fn mode(&self) -> TuiMode {
        renderer_mode(&self.0.borrow().renderer)
    }
}

/// The renderer handed to the caller's render loop.
/// `RenderLoop::core` returns a reference, so the handle keeps its own clone of
/// the current core and re-reads it whenever the renderer behind the cell was
/// replaced — which can only happen between two frames.
pub struct InteractiveRenderer {
    cell: RendererCell,
    core: TuiCore,
    generation: u64,
}

impl RenderLoop for InteractiveRenderer {
    fn core(&self) -> &TuiCore {
        &self.core
    }

    fn render_pending_frame(&mut self) {
        let generation = self.cell.generation();
        if generation != self.generation {
            self.core = self.cell.core();
            self.generation = generation;
        }
        self.cell.render_pending_frame();
    }
}

// ============================================================================
// Messages the components post into the loop
// ============================================================================

/// One of the app actions the editor dispatches (`onAction` in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppAction {
    /// `app.clear` — Ctrl+C.
    Clear,
    /// `app.message.followUp` — Alt+Enter.
    FollowUp,
    /// `app.message.dequeue`.
    Dequeue,
    Copy,
    /// `app.clipboard.pasteImage`.
    PasteImage,
    CycleMode,
    CycleThinking,
    CycleModelForward,
    CycleModelBackward,
    SelectModel,
    ExpandTools,
    ToggleThinking,
    ExternalEditor,
    NewSession,
    Tree,
    Fork,
    Resume,
    DetachTasks,
    /// `app.suspend` — Ctrl+Z.
    Suspend,
    /// `app.exit` — Ctrl+D on an empty editor.
    Exit,
    /// `app.interrupt` — Escape.
    Escape,
    /// Up or down on an empty editor, which the side-question panel takes when
    /// one is open.
    ScrollSideQuestion {
        up: bool,
    },
}

/// What the component callbacks post into [`InteractiveMode::run`].
enum UiMessage {
    /// Terminal input was dispatched; the editor's queues may have grown.
    TerminalInput,
    Action(AppAction),
    /// A theme file changed on disk (`onThemeChange`).
    ThemeChanged,
    VersionChecked {
        release: Option<Box<LatestPiRelease>>,
    },
    PackageUpdatesChecked {
        packages: Vec<String>,
    },
    /// A hook diagnostic.
    Report {
        message: String,
        level: HookReportLevel,
    },
    SubagentModelSelected {
        id: u64,
        model: Box<Model>,
    },
    /// v0.1.7). `done` carries the final message.
    IndexBuildProgress {
        message: String,
        done: bool,
    },
    /// when it did not produce a file.
    InitFinished {
        error: Option<String>,
    },
    /// The side-question child produced an event. Boxed: `AgentEvent` carries a
    /// whole message and would otherwise set the size of every variant here.
    SideQuestionEvent {
        event: Box<AgentEvent>,
    },
    /// The side question settled; `error` is why, when it did not answer.
    SideQuestionFinished {
        error: Option<String>,
    },
    /// A selector reported a choice or a cancellation.
    ModelSelected {
        id: u64,
        // Boxed: `Model` is by far the largest payload here, and clippy asks
        // that a message enum not carry one variant's size for all of them.
        model: Box<Model>,
    },
    Settings {
        id: u64,
        effect: SettingsEffect,
    },
    EffortSelected {
        id: u64,
        level: notagent_agent::types::ThinkingLevel,
    },
    ResumeSession {
        id: u64,
        path: String,
    },
    TreeNavigate {
        id: u64,
        entry_id: String,
    },
    TreeLabel {
        entry_id: String,
        label: Option<String>,
    },
    /// Nothing to do; a side future that only forwarded an answer.
    Noop,
    AuthPrompt {
        prompt: Box<AuthPrompt>,
        answer: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
    AuthEvent {
        event: Box<AuthEvent>,
    },
    RestoreLoginDialog,
    LoginAuthType {
        id: u64,
        auth_type: AuthType,
        providers: Option<Vec<AuthSelectorProvider>>,
    },
    LoginProvider {
        id: u64,
        provider: Box<AuthSelectorProvider>,
    },
    /// The auth flow finished; posted by the login side-future so the flow
    /// never blocks this loop (it has to keep serving the AuthPrompt/AuthEvent
    /// bridge the login posts back while it runs).
    LoginFinished {
        id: u64,
        provider_id: String,
        provider_name: String,
        auth_type: AuthType,
        previous_model: Option<Box<Model>>,
        result: Result<(), String>,
    },
    LogoutProvider {
        id: u64,
        provider: Box<AuthSelectorProvider>,
    },
    LoginCatalogRefreshed {
        action_label: String,
        aborted: bool,
        failed: bool,
    },
    TaskBrowser {
        id: u64,
        action: TaskBrowserAction,
    },
    ScopedModelsChanged {
        enabled_ids: Option<Vec<String>>,
        persist: bool,
    },
    ScopedModelsRefreshed {
        aborted: bool,
        errors: Vec<String>,
    },
    /// The model selector's catalog refresh finished — the counterpart of the
    /// the future; the outcome goes back to the selector still open under `id`.
    ModelCatalogRefreshed {
        id: u64,
        outcome: ModelRefreshOutcome,
    },
    TreeCopy {
        text: Option<String>,
    },
    ForkAt {
        id: u64,
        entry_id: String,
        position: ForkPosition,
    },
    TrustDecision {
        id: u64,
        selection: TrustSelection,
    },
    SelectorCancelled {
        id: u64,
    },
    /// `ctx.ui.notify(message, type)` of the `/llama` command.
    Notify {
        message: String,
        level: NotifyLevel,
    },
    /// The `/llama` manager closed; the editor comes back.
    LlamaFinished {
        error: Option<String>,
        saved_text: String,
    },
}

// ============================================================================
// A text component with a collapsed and an expanded form
// ============================================================================

/// a `dyn Component` cannot be inspected that way, so the mode keeps the
trait Expandable {
    fn set_expanded(&mut self, expanded: bool);
}

macro_rules! expandable {
    ($($component:ty),* $(,)?) => {
        $(impl Expandable for $component {
            fn set_expanded(&mut self, expanded: bool) {
                <$component>::set_expanded(self, expanded);
            }
        })*
    };
}

expandable!(
    ToolExecutionComponent,
    SkillInvocationMessageComponent,
    CompactionSummaryMessageComponent,
    BranchSummaryMessageComponent,
    CustomMessageComponent,
    BashExecutionComponent,
    ExploreBlockComponent,
    // The expand toggle opens and closes the compact thinking block
    // (reference: Ctrl+O reaches every transcript component).
    AssistantMessageComponent,
);

/// Visible content ends the preceding exploration immediately: leaving its
/// timer running above a growing answer would repeatedly clear scrollback.
fn streamed_visible_chars(message: &notagent_ai::types::AssistantMessage) -> usize {
    message
        .content
        .iter()
        .map(|content| match content {
            notagent_ai::types::AssistantContent::Text(text) => text.text.trim().len(),
            notagent_ai::types::AssistantContent::Thinking(thinking) => {
                thinking.thinking.trim().len()
            }
            _ => 0,
        })
        .sum()
}

/// A completed message must not close the new exploration its own calls opened.
fn assistant_message_ends_search_run(message: &notagent_ai::types::AssistantMessage) -> bool {
    let announced_exploration = message.content.iter().any(|content| {
        matches!(
            content,
            notagent_ai::types::AssistantContent::ToolCall(call) if is_explore_tool(&call.name)
        )
    });
    streamed_visible_chars(message) > 0 && !announced_exploration
}

struct ExpandableText {
    text: Text,
    collapsed: String,
    expanded: String,
}

impl Expandable for ExpandableText {
    fn set_expanded(&mut self, expanded: bool) {
        self.text.set_text(if expanded {
            self.expanded.clone()
        } else {
            self.collapsed.clone()
        });
    }
}

impl ExpandableText {
    fn new(collapsed: String, expanded: String, is_expanded: bool, padding_x: usize) -> Self {
        let initial = if is_expanded { &expanded } else { &collapsed };
        Self {
            text: Text::new(initial.clone(), padding_x, 0),
            collapsed,
            expanded,
        }
    }

    /// Used by `setToolsExpanded`, which arrives with the expansion slice.
    #[allow(dead_code)]
    fn set_expanded(&mut self, expanded: bool) {
        self.text.set_text(if expanded {
            self.expanded.clone()
        } else {
            self.collapsed.clone()
        });
    }
}

impl Component for ExpandableText {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.text.render(width)
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
}

// ============================================================================
// The mode
// ============================================================================

pub struct InteractiveMode {
    runtime: Arc<AgentSessionRuntime>,
    options: InteractiveModeOptions,

    cell: RendererCell,
    ui: TuiCore,

    // Containers, in the order `init()` mounts them. The ones no slice fills
    // yet are mounted all the same, because their position in the dock is what
    // the layout depends on.
    header_container: Rc<RefCell<Container>>,
    /// Filled by `showLoadedResources`, which arrives with the resources slice.
    #[allow(dead_code)]
    loaded_resources_container: Rc<RefCell<Container>>,
    chat_container: Rc<RefCell<Container>>,
    document_container: Rc<RefCell<Container>>,
    pending_messages_container: Rc<RefCell<Container>>,
    status_container: Rc<RefCell<Container>>,
    todo_panel_container: Rc<RefCell<Container>>,
    todo_panel: Rc<RefCell<TodoListComponent>>,
    todo_visibility: TodoVisibility,
    subagent_panel: Rc<RefCell<SubagentPanel>>,
    subagent_panel_signature: String,
    widget_container_above: Rc<RefCell<Container>>,
    /// The side-question panel while one is open. Held apart from the container
    /// it is mounted in, because the loop has to reach it on every event the
    /// child produces.
    side_question_panel: Option<Rc<RefCell<SideQuestionPanel>>>,
    editor_container: Rc<RefCell<Container>>,
    widget_container_below: Rc<RefCell<Container>>,
    footer_container: Rc<RefCell<Container>>,
    subagent_panel_container: Rc<RefCell<Container>>,

    editor: Rc<RefCell<CustomEditor>>,
    footer: Rc<RefCell<FooterComponent>>,
    footer_data: Arc<crate::core::footer_data_provider::FooterDataProvider>,
    /// Handed to the selectors and the custom editors of the later slices.
    #[allow(dead_code)]
    keybindings: Rc<RefCell<KeybindingsManager>>,
    theme_controller: InteractiveThemeController,
    /// The layout the alternate screen renders (`fullscreenLayoutRoot`).
    fullscreen_layout_root: Option<ComponentRef>,

    is_initialized: bool,

    /// The active animated indicator. Compaction mounts it in the transcript;
    /// other activities use the status line.
    active_status_indicator: Option<Rc<RefCell<StatusIndicator>>>,
    compaction_chat_indicator: Option<ActiveCompactionChatIndicator>,
    working_message: Option<String>,
    working_visible: bool,
    hidden_thinking_label: String,

    last_sigint_time: Option<Instant>,
    anthropic_subscription_warning_shown: bool,

    /// `lastStatusSpacer`/`lastStatusText` — the mutation target of two status
    /// lines emitted back to back.
    last_status_spacer: Option<ComponentRef>,
    last_status_text: Option<Rc<RefCell<Text>>>,

    streaming_component: Option<Rc<RefCell<AssistantMessageComponent>>>,
    streaming_message: Option<AssistantMessage>,
    /// `pendingTools` — insertion-ordered like the JavaScript `Map`.
    pending_tools: Vec<(String, Rc<RefCell<ToolExecutionComponent>>)>,
    /// Every tool row in the transcript, for the settings that reach into them.
    chat_tool_rows: Vec<Rc<RefCell<ToolExecutionComponent>>>,
    /// The open search block collecting consecutive search calls, and every
    /// block of the session for completion routing (takeover of the
    /// reference's explore grouping, user decision 2026-08-17, v0.1.8).
    explore_block: Option<Rc<RefCell<ExploreBlockComponent>>>,
    chat_explore_blocks: Vec<Rc<RefCell<ExploreBlockComponent>>>,
    /// Visible thinking/text of the streaming message at the last update, so
    /// growth — the reference's reasoning/message moment — closes the block.
    streaming_visible_chars: usize,
    /// Every expandable child of the transcript (`app.tools.expand`).
    chat_expandables: Vec<Rc<RefCell<dyn Expandable>>>,
    last_escape_time: Option<Instant>,

    /// The bash row that is filling up, and the run behind it.
    bash_component: Option<Rc<RefCell<BashExecutionComponent>>>,
    pending_bash: Option<PendingBash>,
    /// Rows shown above the editor while a run is going; they move into the
    /// transcript with the next submission.
    pending_bash_components: Vec<Rc<RefCell<BashExecutionComponent>>>,
    bash_tx: tokio::sync::mpsc::UnboundedSender<String>,
    bash_rx: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    /// Messages submitted while a compaction runs.
    compaction_queued_messages: Vec<CompactionQueuedMessage>,
    /// The escape handler the compaction and the retry replace while they run.
    escape_target: EscapeTarget,
    /// When the panels are refreshed next (`tasksPanelTimer`).
    next_panel_refresh: Option<Instant>,
    /// Whether anything could be detached, as of the last panel refresh.
    has_foreground_tasks: Rc<std::cell::Cell<bool>>,
    /// Work the loop drives alongside everything else; each future ends in a
    side_futures: Vec<Pin<Box<dyn Future<Output = UiMessage>>>>,
    active_selector: Option<ActiveSelector>,
    selector_id: u64,
    /// Whether a `/index` build is running; a second `/index` is a no-op
    /// while it is, like the reference's disabled Reindex button.
    index_build_running: bool,
    /// Whether an `/init` child is running. A second one would explore the
    /// same project and race the first for the same file.
    init_running: bool,
    /// The session selector while it is open; the loop runs its list loads.
    session_selector: Option<Rc<RefCell<SessionSelectorComponent>>>,
    /// The model selector while it is open; its catalog refresh reports back.
    model_selector: Option<Rc<RefCell<ModelSelectorComponent>>>,
    /// The models selector while it is open; its catalog refresh reports back.
    scoped_models_selector: Option<Rc<RefCell<ScopedModelsSelectorComponent>>>,
    /// The task browser while it is open, with the state it is fed from.
    tasks_browser: Option<(Rc<RefCell<TasksBrowserComponent>>, TasksBrowserState)>,
    /// The login dialog while a flow is running.
    login_dialog: Option<Rc<RefCell<LoginDialogComponent>>>,

    tool_output_expanded: bool,
    hide_thinking_block: bool,
    output_pad: usize,
    is_bash_mode: bool,

    /// `skillCommands` — command name to skill file.
    skill_commands: Vec<(String, String)>,
    /// The `fd` binary the file completion uses, once `ensureTool` found it.
    fd_path: Option<String>,
    /// Prompts waiting for the main loop (`pendingUserInputs`/`getUserInput`).
    /// Images pasted into the editor, keyed by the number in their
    /// `[Image #n]` marker. The editor shows the marker; the bytes ride along
    /// on submit. Cleared once they have been sent — a marker the user cannot
    /// see any more has nothing left to resolve against.
    pasted_images: HashMap<u32, ImageContent>,
    next_image_paste_id: u32,
    pending_user_inputs: VecDeque<String>,
    pending_compactions: VecDeque<Option<String>>,

    is_shutting_down: bool,
    exit_code: Option<i32>,

    /// Permission requests waiting for the dialog, with the channel the gate
    /// is blocked on.
    approval_tx: ApprovalSender,
    approval_rx: Option<tokio::sync::mpsc::UnboundedReceiver<ApprovalRequestMessage>>,
    ui_tx: tokio::sync::mpsc::UnboundedSender<UiMessage>,
    ui_rx: Option<tokio::sync::mpsc::UnboundedReceiver<UiMessage>>,
    /// The event channel outlives every session: a session switch only swaps
    /// the listener, so the loop never sees its receiver close.
    agent_tx: tokio::sync::mpsc::UnboundedSender<AgentSessionEvent>,
    agent_rx: Option<tokio::sync::mpsc::UnboundedReceiver<AgentSessionEvent>>,
    /// Kept alive for as long as the mode runs; dropping it unsubscribes.
    agent_subscription: Option<crate::core::agent_session::ListenerHandle>,
}

impl InteractiveMode {
    fn new(
        runtime: Arc<AgentSessionRuntime>,
        mut options: InteractiveModeOptions,
        terminal: Box<dyn Terminal>,
    ) -> Self {
        let session = runtime.session();
        let settings_manager = session.settings_manager();
        let tui_mode = options
            .tui_mode
            .unwrap_or_else(|| settings_manager.get_tui_mode());
        options.tui_mode = Some(tui_mode);

        // `createInteractiveTui(options)` — the mode keeps the terminal so a
        // later fullscreen switch can hand it to the next renderer.
        let terminal = SharedTerminal(Rc::new(RefCell::new(terminal)));
        let renderer = match tui_mode {
            TuiMode::Regular => ActiveRenderer::Main(TuiMainScreen::with_options(
                Box::new(terminal.clone()),
                Some(settings_manager.get_show_hardware_cursor()),
                Some(get_agent_dir()),
            )),
            TuiMode::Fullscreen => ActiveRenderer::Alt(Box::new(TuiAltScreen::new(
                Box::new(terminal.clone()),
                notagent_tui::tui_alt_screen::TuiAltScreenOptions {
                    search_match_style: Rc::new(|text: &str| {
                        theme().bg(
                            ThemeBg::SearchMatchBg,
                            &theme().fg(ThemeColor::SearchMatchText, text),
                        )
                    }),
                    search_current_match_style: Rc::new(|text: &str| {
                        theme().bold(&theme().inverse(&theme().bg(
                            ThemeBg::SearchMatchBg,
                            &theme().fg(ThemeColor::SearchMatchText, text),
                        )))
                    }),
                    ..notagent_tui::tui_alt_screen::TuiAltScreenOptions::default()
                },
            ))),
        };
        let cell = RendererCell::new(renderer, terminal);
        let ui = cell.core();
        ui.set_clear_on_shrink(settings_manager.get_clear_on_shrink());

        let header_container = Rc::new(RefCell::new(Container::new()));
        let loaded_resources_container = Rc::new(RefCell::new(Container::new()));
        let chat_container = Rc::new(RefCell::new(Container::new()));
        let document_container = Rc::new(RefCell::new(Container::new()));
        {
            let mut document = document_container.borrow_mut();
            document.add_child(Rc::clone(&header_container) as ComponentRef);
            document.add_child(Rc::clone(&loaded_resources_container) as ComponentRef);
            document.add_child(Rc::clone(&chat_container) as ComponentRef);
        }

        let keybindings = Rc::new(RefCell::new(KeybindingsManager::create(None)));
        set_keybindings(keybindings.borrow().to_tui());

        let editor = Rc::new(RefCell::new(CustomEditor::new(
            ui.clone(),
            get_editor_theme(),
            Rc::clone(&keybindings),
            Some(notagent_tui::components::editor::EditorOptions {
                padding_x: Some(settings_manager.get_editor_padding_x() as usize),
                autocomplete_max_visible: Some(
                    settings_manager.get_autocomplete_max_visible() as usize
                ),
            }),
        )));
        let editor_container = Rc::new(RefCell::new(Container::new()));
        editor_container
            .borrow_mut()
            .add_child(Rc::clone(&editor) as ComponentRef);

        let footer_data = Arc::new(crate::core::footer_data_provider::FooterDataProvider::new(
            &session.with_session_manager(|manager| manager.get_cwd().to_owned()),
        ));
        let footer = Rc::new(RefCell::new(FooterComponent::new(
            Arc::clone(&session)
                as Arc<dyn crate::modes::interactive::components::footer::FooterSession>,
            Arc::clone(&footer_data)
                as Arc<dyn crate::modes::interactive::components::footer::FooterData>,
        )));
        footer
            .borrow_mut()
            .set_show_workspace(settings_manager.get_show_workspace_in_footer());
        let footer_container = Rc::new(RefCell::new(Container::new()));
        footer_container
            .borrow_mut()
            .add_child(Rc::clone(&footer) as ComponentRef);

        let hide_thinking_block = settings_manager.get_hide_thinking_block();
        let output_pad = settings_manager.get_output_pad() as usize;

        let _ = set_registered_themes(session.resource_loader().get_themes().0);
        let error_core = ui.clone();
        let error_chat = Rc::clone(&chat_container);
        let theme_controller = InteractiveThemeController::new(
            ui.clone(),
            Arc::clone(&settings_manager),
            Box::new(move |message| {
                // `showError` before the loop owns `self`: the same two children
                // the method appends.
                let mut chat = error_chat.borrow_mut();
                chat.add_child(component_ref(Spacer::new(1)));
                chat.add_child(component_ref(Text::new(
                    theme().fg(ThemeColor::Error, &format!("Error: {message}")),
                    1,
                    0,
                )));
                error_core.request_render();
            }),
            Box::new(|| {}),
        );

        // The panels of the dock; the loop refreshes them on its tick.
        let subagent_panel = Rc::new(RefCell::new(SubagentPanel::new()));
        let subagent_panel_container = Rc::new(RefCell::new(Container::new()));
        subagent_panel_container
            .borrow_mut()
            .add_child(Rc::clone(&subagent_panel) as ComponentRef);
        let todo_panel = Rc::new(RefCell::new(TodoListComponent::new(
            TodoListMode::Status,
            0,
        )));
        let todo_panel_container = Rc::new(RefCell::new(Container::new()));
        todo_panel_container
            .borrow_mut()
            .add_child(Rc::clone(&todo_panel) as ComponentRef);
        // The delay elapsed and the finished list goes down; the loop reads the
        // visibility state again on its next tick.
        let todo_visibility = TodoVisibility::new(Box::new(|| {}), 5000);

        let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (bash_tx, bash_rx) = tokio::sync::mpsc::unbounded_channel();
        let (approval_tx, approval_rx) = tokio::sync::mpsc::unbounded_channel();

        Self {
            runtime,
            options,
            cell,
            ui,
            header_container,
            loaded_resources_container,
            chat_container,
            document_container,
            pending_messages_container: Rc::new(RefCell::new(Container::new())),
            status_container: Rc::new(RefCell::new(Container::new())),
            todo_panel_container,
            todo_panel,
            todo_visibility,
            subagent_panel,
            subagent_panel_signature: String::new(),
            widget_container_above: Rc::new(RefCell::new(Container::new())),
            side_question_panel: None,
            editor_container,
            widget_container_below: Rc::new(RefCell::new(Container::new())),
            footer_container,
            subagent_panel_container,
            editor,
            footer,
            footer_data,
            keybindings,
            theme_controller,
            fullscreen_layout_root: None,
            is_initialized: false,
            active_status_indicator: None,
            compaction_chat_indicator: None,
            working_message: None,
            working_visible: true,
            hidden_thinking_label: DEFAULT_HIDDEN_THINKING_LABEL.to_owned(),
            last_sigint_time: None,
            anthropic_subscription_warning_shown: false,
            last_status_spacer: None,
            last_status_text: None,
            streaming_component: None,
            streaming_message: None,
            pending_tools: Vec::new(),
            chat_tool_rows: Vec::new(),
            explore_block: None,
            chat_explore_blocks: Vec::new(),
            streaming_visible_chars: 0,
            chat_expandables: Vec::new(),
            last_escape_time: None,
            bash_component: None,
            pending_bash: None,
            pending_bash_components: Vec::new(),
            bash_tx,
            bash_rx: Some(bash_rx),
            compaction_queued_messages: Vec::new(),
            escape_target: EscapeTarget::Default,
            next_panel_refresh: None,
            has_foreground_tasks: Rc::new(std::cell::Cell::new(false)),
            side_futures: Vec::new(),
            active_selector: None,
            selector_id: 0,
            index_build_running: false,
            init_running: false,
            session_selector: None,
            model_selector: None,
            scoped_models_selector: None,
            tasks_browser: None,
            login_dialog: None,
            tool_output_expanded: false,
            hide_thinking_block,
            output_pad,
            is_bash_mode: false,
            skill_commands: Vec::new(),
            fd_path: None,
            pasted_images: HashMap::new(),
            next_image_paste_id: 1,
            pending_user_inputs: VecDeque::new(),
            pending_compactions: VecDeque::new(),
            is_shutting_down: false,
            exit_code: None,
            approval_tx,
            approval_rx: Some(approval_rx),
            ui_tx,
            ui_rx: Some(ui_rx),
            agent_tx,
            agent_rx: Some(agent_rx),
            agent_subscription: None,
        }
    }

    /// `permissionPresent = (request) => interactiveMode.requestApproval(request)`
    /// Deviation (class 1): the gate runs wherever the tool call runs and needs
    /// a `Send` presenter, while the dialog is `!Send`. The presenter therefore
    /// hands the request to the loop and awaits the answer on a `oneshot`; a
    /// dropped channel is a denial, exactly as a dismissed dialog is.
    pub fn approval_presenter(&self) -> ApprovalPresenter {
        let tx = self.approval_tx.clone();
        Arc::new(move |request: ApprovalRequest| {
            let tx = tx.clone();
            Box::pin(async move {
                let (answer_tx, answer_rx) = tokio::sync::oneshot::channel();
                if tx.send((request, answer_tx)).is_err() {
                    return ApprovalAnswer::Deny;
                }
                answer_rx.await.unwrap_or(ApprovalAnswer::Deny)
            }) as futures::future::BoxFuture<'static, ApprovalAnswer>
        })
    }

    /// because it is called from wherever a hook ran.
    pub fn reporter(&self) -> HookReporter {
        let tx = self.ui_tx.clone();
        Arc::new(move |message: &str, level: HookReportLevel| {
            let _ = tx.send(UiMessage::Report {
                message: message.to_owned(),
                level,
            });
        })
    }

    /// The renderer handle for the caller's render loop.
    fn renderer(&self) -> InteractiveRenderer {
        InteractiveRenderer {
            cell: self.cell.clone(),
            core: self.cell.core(),
            generation: self.cell.generation(),
        }
    }

    // ------------------------------------------------------------------
    // ------------------------------------------------------------------

    fn session(&self) -> Arc<AgentSession> {
        self.runtime.session()
    }

    fn settings(&self) -> Arc<crate::core::settings_manager::SettingsManager> {
        self.session().settings_manager()
    }

    fn cwd(&self) -> String {
        self.session()
            .with_session_manager(|manager| manager.get_cwd().to_owned())
    }

    // ------------------------------------------------------------------
    // init
    // ------------------------------------------------------------------

    /// (autocomplete slice), the scoped-model startup line, the fullscreen
    /// layout root and the extension-free remainder of `rebindCurrentSession`.
    async fn init(&mut self) {
        if self.is_initialized {
            return;
        }

        self.record_version_seen();
        // fd feeds the file autocomplete, rg the grep tool and bash.
        let (fd_path, _) = futures::join!(
            ensure_tool(ManagedTool::Fd, true),
            ensure_tool(ManagedTool::Rg, true)
        );
        self.fd_path = fd_path;

        self.mount();
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.setup_key_handlers();
        self.setup_autocomplete_provider();
        self.subscribe_to_agent();

        self.cell.start();
        self.is_initialized = true;
        // `startTasksPanelRefresh()`
        self.next_panel_refresh = Some(Instant::now());
        self.refresh_tasks_panel();

        self.theme_controller.apply_from_settings().await;
        self.build_header();
        self.ui.request_render();

        self.render_initial_messages();

        // Deviation (class 1): the theme watcher fires from its own thread, so
        // the callback posts a repaint into the loop instead of touching the
        let tx = self.ui_tx.clone();
        on_theme_change(Arc::new(move || {
            let _ = tx.send(UiMessage::ThemeChanged);
        }));

        self.update_terminal_title();
        self.update_available_provider_count();
    }

    /// list `init` builds.
    fn mount(&mut self) {
        // `init()` builds the fullscreen layout: the transcript scrolls, the
        let transcript = component_ref(ScrollView::new(
            Rc::clone(&self.document_container) as ComponentRef,
            ScrollViewOptions {
                follow_end: true,
                primary: true,
                overscroll: Overscroll::Chain,
                scrollbar: scroll_view_scrollbar(self.settings().get_fullscreen_scrollbar()),
                scrollbar_style: Rc::new(|text: &str| theme().bg(ThemeBg::ScrollbarThumb, text)),
                ..ScrollViewOptions::default()
            },
        ));
        let mut dock = VStack::new(StackOptions::default());
        // panel sits below the footer, like the roster in notagent-main-rust,
        // instead of above the editor.
        for (component, min_size) in [
            (
                Rc::clone(&self.pending_messages_container) as ComponentRef,
                0,
            ),
            (Rc::clone(&self.status_container) as ComponentRef, 0),
            (Rc::clone(&self.todo_panel_container) as ComponentRef, 0),
            (Rc::clone(&self.widget_container_above) as ComponentRef, 0),
            (Rc::clone(&self.editor_container) as ComponentRef, 3),
            (Rc::clone(&self.widget_container_below) as ComponentRef, 0),
            (Rc::clone(&self.footer_container) as ComponentRef, 1),
            (Rc::clone(&self.subagent_panel_container) as ComponentRef, 0),
        ] {
            dock.add_child_with(
                component,
                StackEntryOptions {
                    shrink: Some(1),
                    min_size: Some(min_size),
                    ..StackEntryOptions::default()
                },
            );
        }
        let mut root = VStack::new(StackOptions::default());
        root.add_child_with(
            transcript,
            StackEntryOptions {
                basis: Some(StackBasis::Size(0)),
                grow: Some(1),
                shrink: Some(1),
                min_size: Some(1),
                ..StackEntryOptions::default()
            },
        );
        root.add_child_with(
            component_ref(dock),
            StackEntryOptions {
                grow: Some(0),
                shrink: Some(1),
                min_size: Some(1),
                ..StackEntryOptions::default()
            },
        );
        self.fullscreen_layout_root = Some(component_ref(root));

        let children: [ComponentRef; 9] = [
            Rc::clone(&self.document_container) as ComponentRef,
            Rc::clone(&self.pending_messages_container) as ComponentRef,
            Rc::clone(&self.status_container) as ComponentRef,
            Rc::clone(&self.todo_panel_container) as ComponentRef,
            Rc::clone(&self.widget_container_above) as ComponentRef,
            Rc::clone(&self.editor_container) as ComponentRef,
            Rc::clone(&self.widget_container_below) as ComponentRef,
            Rc::clone(&self.footer_container) as ComponentRef,
            Rc::clone(&self.subagent_panel_container) as ComponentRef,
        ];
        for child in children {
            self.ui.add_child(child);
        }
        self.cell
            .set_layout_root(self.fullscreen_layout_root.clone());
        self.show_idle_status();
    }

    fn build_header(&mut self) {
        let settings = self.settings();
        if self.options.verbose || !settings.get_quiet_startup() {
            let runtime = Arc::clone(&self.runtime);
            let footer_data = Arc::clone(&self.footer_data);
            let header = StartupHeader::from_provider(Rc::new(move || {
                let model = runtime
                    .session()
                    .model()
                    .map(|model| format!("{}/{}", model.provider, model.id));
                StartupHeaderData {
                    directory: runtime.cwd(),
                    git_branch: footer_data.get_git_branch(),
                    model,
                }
            }));
            let mut container = self.header_container.borrow_mut();
            container.add_child(component_ref(Spacer::new(1)));
            container.add_child(component_ref(header));
            container.add_child(component_ref(Spacer::new(1)));
        } else {
            self.header_container
                .borrow_mut()
                .add_child(component_ref(Text::new("", 0, 0)));
        }
    }

    fn update_terminal_title(&self) {
        let session = self.session();
        let (cwd, name) = session.with_session_manager(|manager| {
            (
                manager.get_cwd().to_owned(),
                manager.get_session_name().clone(),
            )
        });
        let basename = std::path::Path::new(&cwd)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.clone());
        let title = match name {
            Some(name) => format!("{APP_TITLE} - {name} - {basename}"),
            None => format!("{APP_TITLE} - {basename}"),
        };
        self.ui.with_terminal(|terminal| terminal.set_title(&title));
    }

    /// Every handler posts into the loop instead of mutating the mode: the
    /// editor holds the callback while the loop holds `&mut self`. The slices
    /// still to come add the remaining actions.
    fn setup_key_handlers(&mut self) {
        let mut editor = self.editor.borrow_mut();
        let tx = self.ui_tx.clone();
        editor.on_escape = Some(Box::new(move || {
            let _ = tx.send(UiMessage::Action(AppAction::Escape));
        }));
        let tx = self.ui_tx.clone();
        editor.on_ctrl_d = Some(Box::new(move || {
            let _ = tx.send(UiMessage::Action(AppAction::Exit));
        }));
        let tx = self.ui_tx.clone();
        editor.on_paste_image = Some(Box::new(move || {
            let _ = tx.send(UiMessage::Action(AppAction::PasteImage));
        }));
        let tx = self.ui_tx.clone();
        editor.on_up_arrow_empty = Some(Box::new(move || {
            tx.send(UiMessage::Action(AppAction::ScrollSideQuestion {
                up: true,
            }))
            .is_ok()
        }));
        let tx = self.ui_tx.clone();
        editor.on_down_arrow_empty = Some(Box::new(move || {
            tx.send(UiMessage::Action(AppAction::ScrollSideQuestion {
                up: false,
            }))
            .is_ok()
        }));
        for (action, message) in [
            ("app.clear", AppAction::Clear),
            ("app.message.followUp", AppAction::FollowUp),
            ("app.message.dequeue", AppAction::Dequeue),
            ("app.message.copy", AppAction::Copy),
            ("app.mode.cycle", AppAction::CycleMode),
            ("app.thinking.cycle", AppAction::CycleThinking),
            ("app.model.cycleForward", AppAction::CycleModelForward),
            ("app.model.cycleBackward", AppAction::CycleModelBackward),
            ("app.model.select", AppAction::SelectModel),
            ("app.tools.expand", AppAction::ExpandTools),
            ("app.thinking.toggle", AppAction::ToggleThinking),
            ("app.editor.external", AppAction::ExternalEditor),
            ("app.session.new", AppAction::NewSession),
            ("app.session.tree", AppAction::Tree),
            ("app.session.fork", AppAction::Fork),
            ("app.session.resume", AppAction::Resume),
            ("app.suspend", AppAction::Suspend),
        ] {
            let tx = self.ui_tx.clone();
            editor.on_action(
                action,
                Box::new(move || {
                    let _ = tx.send(UiMessage::Action(message));
                    true
                }),
            );
        }
        // `app.tasks.detach` is the only action that can decline the key: it
        // shares the cursor-left binding, and taking that away from someone who
        // was only editing text would be a poor trade. The flag is what the
        // panel refresh saw last.
        let tx = self.ui_tx.clone();
        let has_foreground_tasks = Rc::clone(&self.has_foreground_tasks);
        editor.on_action(
            "app.tasks.detach",
            Box::new(move || {
                if !has_foreground_tasks.get() {
                    return false;
                }
                let _ = tx.send(UiMessage::Action(AppAction::DetachTasks));
                true
            }),
        );
        drop(editor);

        // The wake-up for everything the editor records instead of calling back:
        // submissions and text changes are drained after the input was
        // dispatched (`Editor::take_submitted`/`take_changes`).
        let tx = self.ui_tx.clone();
        self.ui.add_input_listener(Box::new(move |_data| {
            let _ = tx.send(UiMessage::TerminalInput);
            None
        }));
    }

    /// The session emits from wherever the run happens to be; the events cross
    /// into the single-threaded UI over a channel and are handled in the loop.
    fn subscribe_to_agent(&mut self) {
        let tx = self.agent_tx.clone();
        self.agent_subscription = Some(self.session().subscribe(Arc::new(move |event| {
            let _ = tx.send(event);
        })));
    }

    /// (`setRebindSession`) because an extension could switch the session too;
    /// without extensions every switch starts in this loop, and the callback
    /// would have to be `Send` and would deadlock against the loop that is
    /// awaiting the switch. The mode therefore rebinds right where it switched.
    fn rebind_current_session(&mut self) {
        // Dropping the handle unsubscribes the listener of the old session.
        self.agent_subscription = None;
        self.apply_runtime_settings();
        self.render_current_session_state();
        self.subscribe_to_agent();
        self.update_available_provider_count();
        self.update_editor_border_color();
        self.update_terminal_title();
    }

    fn render_current_session_state(&mut self) {
        // The list belonged to the session being replaced.
        self.clear_todo_panel();
        self.loaded_resources_container.borrow_mut().clear();
        self.chat_container.borrow_mut().clear();
        self.pending_messages_container.borrow_mut().clear();
        self.streaming_component = None;
        self.streaming_message = None;
        self.pending_tools.clear();
        self.last_status_spacer = None;
        self.last_status_text = None;
        self.render_initial_messages();
    }

    // ------------------------------------------------------------------
    // run
    // ------------------------------------------------------------------

    /// check, package updates, tmux keyboard setup, the model-runtime refresh)
    pub async fn run(&mut self) -> i32 {
        self.init().await;

        let migrated = std::mem::take(&mut self.options.migrated_providers);
        if !migrated.is_empty() {
            self.show_warning(&format!(
                "Migrated credentials to auth.json: {}",
                migrated.join(", ")
            ));
        }
        if let Some(error) = self.runtime.services().model_runtime.get_error() {
            self.show_error(&format!("models.json error: {error}"));
        }
        if let Some(message) = self.options.model_fallback_message.take() {
            self.show_warning(&message);
        }

        // `void checkForNewPiVersion(...)`, `void checkForPackageUpdates(...)`,
        // `void checkTmuxKeyboardSetup(...)` — the loop drives all three.
        self.side_futures.push(Box::pin(async move {
            UiMessage::VersionChecked {
                release: check_for_new_pi_version(VERSION).await.map(Box::new),
            }
        }));
        let package_cwd = self.cwd();
        let package_settings = Arc::clone(&self.settings());
        self.side_futures.push(Box::pin(async move {
            let packages = DefaultPackageManager::new(PackageManagerOptions {
                cwd: package_cwd,
                agent_dir: get_agent_dir().to_string_lossy().into_owned(),
                settings_manager: package_settings,
                command_runner: None,
            })
            .check_for_available_updates()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|update| update.display_name)
            .collect();
            UiMessage::PackageUpdatesChecked { packages }
        }));
        // A dynamic provider's catalog entry is only as fresh as its last
        // network refresh, and startup restores from cache. Without this, a
        // context window the server changed stays stale until the model
        // picker happens to open — and the footer, the compaction threshold
        // and the picker all read it.
        let session_provider = self.session().model().map(|model| model.provider);
        if let Some(provider) = session_provider
            && self
                .runtime
                .services()
                .model_runtime
                .get_provider(&provider)
                .is_some_and(|provider| provider.is_dynamic())
        {
            let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
            self.side_futures.push(Box::pin(async move {
                let _ = model_runtime
                    .refresh(notagent_ai::models::ModelsRefreshOptions {
                        providers: Some(vec![provider]),
                        signal: Some(timeout_signal(15_000)),
                        ..notagent_ai::models::ModelsRefreshOptions::default()
                    })
                    .await;
                UiMessage::Noop
            }));
        }

        if let Some(warning) = Self::check_tmux_keyboard_setup() {
            self.show_warning(&warning);
        }
        self.maybe_warn_about_anthropic_subscription_auth(None)
            .await;
        self.load_mcp_servers().await;

        if let Some(initial) = self.options.initial_message.take() {
            self.pending_user_inputs.push_back(initial);
        }
        for message in std::mem::take(&mut self.options.initial_messages) {
            self.pending_user_inputs.push_back(message);
        }

        let shutdown_signal = self.options.shutdown_signal.clone();
        let mut ui_rx = self.ui_rx.take().expect("run called once");
        let mut agent_rx = self.agent_rx.take().expect("init subscribed");
        let mut bash_rx = self.bash_rx.take().expect("run called once");
        let mut approval_rx = self.approval_rx.take().expect("run called once");
        let mut prompt: Option<PromptFuture> = None;
        let mut compaction: Option<CompactionFuture> = None;
        let mut session_load: Option<RunningSessionLoad> = None;
        let mut side: futures::stream::FuturesUnordered<Pin<Box<dyn Future<Output = UiMessage>>>> =
            futures::stream::FuturesUnordered::new();

        loop {
            if let Some(code) = self.exit_code {
                return code;
            }
            if prompt.is_none()
                && let Some(text) = self.pending_user_inputs.pop_front()
            {
                let session = self.session();
                // Startup images ride the first message; pasted ones ride the
                // message that mentions their marker.
                let mut images = std::mem::take(&mut self.options.initial_images);
                images.extend(self.images_for_text(&text));
                self.clear_pasted_images();
                let submitted_text = text.clone();
                prompt = Some(Box::pin(async move {
                    let result = session
                        .prompt(
                            &text,
                            PromptOptions {
                                images,
                                ..PromptOptions::default()
                            },
                        )
                        .await;
                    (submitted_text, result)
                }));
            }
            if compaction.is_none()
                && let Some(instructions) = self.pending_compactions.pop_front()
            {
                let session = self.session();
                compaction = Some(Box::pin(async move {
                    let _ = session.compact(instructions.as_deref()).await;
                }));
            }

            for future in std::mem::take(&mut self.side_futures) {
                side.push(future);
            }
            let side_pending = !side.is_empty();

            // The session selector asks for its lists; the loop runs them, so
            // the list keeps rendering while a scope loads.
            if session_load.is_none()
                && let Some(selector) = self.session_selector.clone()
                && let Some(request) = selector.borrow_mut().take_pending_load()
            {
                let session = self.session();
                let (cwd, session_dir, uses_default_dir) =
                    session.with_session_manager(|manager| {
                        (
                            manager.get_cwd().to_owned(),
                            manager.get_session_dir().to_owned(),
                            manager.uses_default_session_dir(),
                        )
                    });
                let scope = request.scope;
                session_load = Some((
                    request,
                    Box::pin(async move {
                        match scope {
                            SessionScope::Current => {
                                SessionManager::list(&cwd, Some(&session_dir), None).await
                            }
                            SessionScope::All => {
                                let directory = (!uses_default_dir).then_some(session_dir.as_str());
                                SessionManager::list_all(directory, None).await
                            }
                        }
                    }) as Pin<Box<dyn Future<Output = Vec<SessionInfo>>>>,
                ));
            }
            let load_pending = session_load.is_some();

            let mut bash = self.pending_bash.take();
            let deadline = self.next_deadline();
            let outcome = tokio::select! {
                biased;
                result = async {
                    match prompt.as_mut() {
                        Some(prompt) => prompt.await,
                        None => std::future::pending().await,
                    }
                } => {
                    prompt = None;
                    let (submitted_text, result) = result;
                    if let Err(message) = result {
                        self.restore_rejected_prompt(&submitted_text);
                        self.show_error(&message);
                    }
                    None
                }
                () = async {
                    match compaction.as_mut() {
                        Some(compaction) => compaction.await,
                        None => std::future::pending().await,
                    }
                } => {
                    compaction = None;
                    None
                }
                result = async {
                    match bash.as_mut() {
                        Some(bash) => bash.run.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => bash.take().map(|bash| (bash, result)),
                request = approval_rx.recv() => {
                    if let Some((request, answer)) = request {
                        self.show_approval_selector(request, answer);
                    }
                    None
                }
                sessions = async {
                    let (_, load) = session_load.as_mut().expect("guarded by load_pending");
                    load.await
                }, if load_pending => {
                    let (request, _) = session_load.take().expect("guarded by load_pending");
                    if let Some(selector) = self.session_selector.clone() {
                        selector.borrow_mut().apply_load_result(request, Ok(sessions));
                        self.ui.request_render();
                    }
                    None
                }
                message = async {
                    use futures::StreamExt;
                    side.next().await
                }, if side_pending => {
                    if let Some(message) = message {
                        self.handle_ui_message(message).await;
                    }
                    None
                }
                chunk = bash_rx.recv() => {
                    // `executeBash`'s chunk callback: the row grows while the
                    // command runs.
                    if let (Some(chunk), Some(component)) = (chunk, self.bash_component.clone()) {
                        component.borrow_mut().append_output(&chunk);
                        self.ui.request_render();
                    }
                    None
                }
                message = ui_rx.recv() => {
                    match message {
                        Some(message) => self.handle_ui_message(message).await,
                        None => return 0,
                    }
                    None
                }
                event = agent_rx.recv() => {
                    match event {
                        Some(event) => self.handle_event(event).await,
                        None => return 0,
                    }
                    None
                }
                () = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.tick();
                    self.pump_editor_autocomplete().await;
                    None
                }
                // The signal handlers of `registerSignalHandlers()`: the binary
                // catches SIGTERM/SIGHUP and cancels the token, the shutdown
                // itself runs here, where the terminal is owned.
                () = async {
                    match shutdown_signal.as_ref() {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.shutdown_from_signal().await;
                    None
                }
            };

            // A bash run the select did not finish keeps going in the next pass.
            if let Some(bash) = bash {
                self.pending_bash = Some(bash);
            }
            if let Some((bash, result)) = outcome {
                self.finish_bash_command(&bash.command, bash.exclude_from_context, result);
            }
        }
    }

    /// The next animation deadline the loop has to wake for.
    fn next_deadline(&self) -> Option<Instant> {
        // `startTasksPanelRefresh` — the one-second interval of the panels.
        let mut deadline: Option<Instant> = self.next_panel_refresh;
        if let Some(notice) = self
            .tasks_browser
            .as_ref()
            .and_then(|(_, state)| state.notice_until)
        {
            deadline = Some(deadline.map_or(notice, |current: Instant| current.min(notice)));
        }
        if let Some(hide) = self.todo_visibility.hide_deadline() {
            deadline = Some(deadline.map_or(hide, |current: Instant| current.min(hide)));
        }
        if let Some(indicator) = self.active_status_indicator.as_ref() {
            let mut indicator = indicator.borrow_mut();
            for candidate in [indicator.loader_mut().next_frame_deadline()]
                .into_iter()
                .flatten()
            {
                deadline =
                    Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
            }
            if let Some(candidate) = indicator.countdown_deadline() {
                deadline =
                    Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
            }
            if let Some(candidate) = indicator.elapsed_deadline() {
                deadline =
                    Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
            }
        }
        if let Some(candidate) = self.editor.borrow().editor().autocomplete_deadline() {
            deadline = Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
        }
        deadline
    }

    /// The missing half of the `autocomplete_deadline` seam: the deadline in
    /// `next_deadline` only wakes the loop — the due request must be pumped
    /// here, exactly like the editor tests drive it
    /// (`crates/notagent-tui/tests/editor.rs`).
    //
    // The borrow is deliberately held across the await: the pump runs inside
    // the select arm, so no other arm can touch the editor, and the provider
    // it awaits (fd + command table) never calls back into the editor.
    #[allow(clippy::await_holding_refcell_ref)]
    async fn pump_editor_autocomplete(&mut self) {
        let due = self
            .editor
            .borrow()
            .editor()
            .autocomplete_deadline()
            .is_some_and(|deadline| deadline <= Instant::now());
        if !due {
            return;
        }
        self.editor
            .borrow_mut()
            .editor_mut()
            .pump_autocomplete()
            .await;
        self.ui.request_render();
    }

    /// One pass over everything a deadline was due for.
    fn tick(&mut self) {
        if self
            .next_panel_refresh
            .is_none_or(|deadline| deadline <= Instant::now())
        {
            self.next_panel_refresh = Some(Instant::now() + Duration::from_secs(1));
            self.refresh_tasks_panel();
            // `poll = setInterval(push, 1000)` of the task browser.
            if let Some(id) = self.active_selector.as_ref().map(|active| active.id)
                && self.tasks_browser.is_some()
            {
                self.push_tasks_browser(id);
            }
        }
        // The browser's notice fades after 2.5 seconds.
        if let Some(id) = self.active_selector.as_ref().map(|active| active.id)
            && self
                .tasks_browser
                .as_ref()
                .and_then(|(_, state)| state.notice_until)
                .is_some_and(|until| until <= Instant::now())
        {
            self.handle_task_browser_action(id, TaskBrowserAction::ClearNotice);
        }
        let mut needs_render = false;
        // The 5-second hide of an all-completed todo list: `hide_deadline`
        // only wakes the loop (next_deadline); the visibility flips here —
        // the same missing-half pattern the autocomplete pump had.
        if self.todo_visibility.tick() {
            self.todo_panel.borrow_mut().set_todos(Vec::new());
            needs_render = true;
        }
        if let Some(indicator) = self.active_status_indicator.clone() {
            let mut indicator = indicator.borrow_mut();
            if indicator
                .loader_mut()
                .next_frame_deadline()
                .is_some_and(|deadline| deadline <= Instant::now())
            {
                indicator.loader_mut().tick();
                needs_render |= indicator.loader_mut().take_render_request();
            }
            if indicator
                .countdown_deadline()
                .is_some_and(|deadline| deadline <= Instant::now())
            {
                needs_render |= indicator.tick_countdown();
            }
            needs_render |= indicator.tick_elapsed();
        }
        // The Thinking heading's live timer advances at most once per second.
        if let Some(component) = self.streaming_component.clone() {
            needs_render |= component.borrow_mut().tick_thinking_timer();
        }
        if needs_render {
            self.ui.request_render();
        }
    }

    /// Everything a component posted since the last pass.
    async fn handle_ui_message(&mut self, message: UiMessage) {
        match message {
            UiMessage::TerminalInput => self.drain_editor().await,
            UiMessage::Action(AppAction::Clear) => self.handle_ctrl_c().await,
            UiMessage::Action(AppAction::Exit) => self.handle_ctrl_d().await,
            UiMessage::Action(AppAction::FollowUp) => self.handle_follow_up().await,
            UiMessage::Action(AppAction::Dequeue) => self.handle_dequeue(),
            UiMessage::Action(AppAction::Copy) => self.handle_copy_command(true),
            UiMessage::Action(AppAction::PasteImage) => self.handle_clipboard_paste(),
            UiMessage::Action(AppAction::CycleMode) => self.cycle_operating_mode(),
            UiMessage::Action(AppAction::CycleThinking) => self.cycle_thinking_level(),
            UiMessage::Action(AppAction::CycleModelForward) => self.cycle_model(true),
            UiMessage::Action(AppAction::CycleModelBackward) => self.cycle_model(false),
            UiMessage::Action(AppAction::SelectModel) => self.show_model_selector(None),
            UiMessage::Action(AppAction::ExpandTools) => self.toggle_tool_output_expansion(),
            UiMessage::Action(AppAction::ToggleThinking) => self.toggle_thinking_block_visibility(),
            UiMessage::Action(AppAction::ExternalEditor) => self.handle_open_external_editor(),
            UiMessage::Action(AppAction::NewSession) => self.handle_clear_command().await,
            UiMessage::Action(AppAction::Tree) => self.show_tree_selector(None),
            UiMessage::Action(AppAction::Fork) => self.show_user_message_selector(),
            UiMessage::Action(AppAction::Resume) => self.show_session_selector(),
            UiMessage::Action(AppAction::Suspend) => self.handle_ctrl_z(),
            UiMessage::Action(AppAction::DetachTasks) => {
                self.detach_foreground_tasks();
            }
            UiMessage::Action(AppAction::Escape) => self.handle_escape(),
            UiMessage::Action(AppAction::ScrollSideQuestion { up }) => {
                if let Some(panel) = self.side_question_panel.clone()
                    && panel.borrow_mut().scroll(up)
                {
                    self.ui.request_render();
                }
            }
            UiMessage::Settings { id, effect } => {
                let _ = id;
                self.apply_settings_effect(effect).await;
            }
            UiMessage::ModelSelected { id, model } => {
                self.close_selector(id);
                self.apply_selected_model(*model).await;
            }
            UiMessage::EffortSelected { id, level } => {
                self.close_selector(id);
                self.session().set_thinking_level(level);
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
                self.show_status(&format!(
                    "Effort: {}",
                    thinking_level_name(self.session().thinking_level())
                ));
            }
            UiMessage::SubagentModelSelected { id, model } => {
                self.close_selector(id);
                self.apply_selected_subagent_model(*model);
            }
            UiMessage::IndexBuildProgress { message, done } => {
                if done {
                    self.index_build_running = false;
                }
                self.show_status(&message);
            }
            UiMessage::InitFinished { error } => {
                self.init_running = false;
                match error {
                    Some(error) => self.show_warning(&error),
                    None => self.show_status("AGENTS.md written."),
                }
            }
            UiMessage::SideQuestionEvent { event } => self.handle_side_question_event(*event),
            UiMessage::SideQuestionFinished { error } => self.handle_side_question_finished(error),
            UiMessage::ForkAt {
                id,
                entry_id,
                position,
            } => {
                self.close_selector(id);
                self.fork_session(&entry_id, position).await;
            }
            UiMessage::ResumeSession { id, path } => {
                self.close_selector(id);
                self.handle_resume_session(&path).await;
            }
            UiMessage::TreeNavigate { id, entry_id } => {
                self.close_selector(id);
                self.navigate_tree(&entry_id).await;
            }
            UiMessage::TreeLabel { entry_id, label } => {
                self.session().with_session_manager(|manager| {
                    let _ = manager.append_label_change(&entry_id, label.as_deref());
                });
                self.ui.request_render();
            }
            UiMessage::Noop => {}
            UiMessage::AuthPrompt { prompt, answer } => self.handle_auth_prompt(*prompt, answer),
            UiMessage::AuthEvent { event } => self.handle_auth_event(*event),
            UiMessage::RestoreLoginDialog => {
                if let Some(dialog) = self.login_dialog.clone() {
                    let mut container = self.editor_container.borrow_mut();
                    container.clear();
                    container.add_child(Rc::clone(&dialog) as ComponentRef);
                    drop(container);
                    self.ui.set_focus(Some(Rc::clone(&dialog) as ComponentRef));
                    self.ui.request_render();
                }
            }
            UiMessage::LoginAuthType {
                id,
                auth_type,
                providers,
            } => {
                self.close_selector(id);
                match providers {
                    Some(providers) => {
                        if let Some(provider) = providers
                            .into_iter()
                            .find(|provider| provider.auth_type == auth_type)
                        {
                            self.start_provider_login(provider);
                        }
                    }
                    None => self.show_login_provider_selector(Some(auth_type), None),
                }
            }
            UiMessage::LoginProvider { id, provider } => {
                self.close_selector(id);
                self.start_provider_login(*provider);
            }
            UiMessage::LoginFinished {
                id,
                provider_id,
                provider_name,
                auth_type,
                previous_model,
                result,
            } => {
                self.login_dialog = None;
                self.close_selector(id);
                match result {
                    Ok(()) => {
                        self.complete_provider_authentication(
                            &provider_id,
                            &provider_name,
                            auth_type,
                            previous_model.map(|model| *model),
                        )
                        .await
                    }
                    Err(message) => {
                        if message != "Login cancelled" {
                            self.show_error(&if auth_type == AuthType::OAuth {
                                format!("Failed to login to {provider_name}: {message}")
                            } else {
                                format!("Failed to save API key for {provider_name}: {message}")
                            });
                        }
                    }
                }
            }
            UiMessage::LogoutProvider { id, provider } => {
                self.close_selector(id);
                self.logout_provider(*provider).await;
            }
            UiMessage::LoginCatalogRefreshed {
                action_label,
                aborted,
                failed,
            } => {
                if aborted {
                    self.show_warning(&format!(
                        "{action_label}, but its model catalog refresh timed out; using cached models."
                    ));
                } else if failed {
                    self.show_warning(&format!(
                        "{action_label}, but its model catalog could not be refreshed; using cached models."
                    ));
                }
                self.update_available_provider_count();
                self.footer.borrow_mut().invalidate();
                self.ui.request_render();
            }
            UiMessage::TaskBrowser { id, action } => self.handle_task_browser_action(id, action),
            UiMessage::ScopedModelsChanged {
                enabled_ids,
                persist,
            } => self.apply_scoped_models(enabled_ids, persist),
            UiMessage::ScopedModelsRefreshed { aborted, errors } => {
                self.apply_scoped_models_refresh(aborted, errors)
            }
            UiMessage::ModelCatalogRefreshed { id, outcome } => {
                // An outcome from a superseded selector must not touch its
                // successor; `apply_refresh` itself handles a disposed one.
                if self
                    .active_selector
                    .as_ref()
                    .is_some_and(|active| active.id == id)
                    && let Some(selector) = &self.model_selector
                {
                    selector.borrow_mut().apply_refresh(outcome);
                }
            }
            UiMessage::TreeCopy { text } => match text {
                None => self.show_error("Selected entry has no text to copy"),
                Some(text) => match crate::utils::clipboard::copy_to_clipboard(&text) {
                    Ok(()) => self.show_status("Copied selected message to clipboard"),
                    Err(message) => self.show_error(&message),
                },
            },
            UiMessage::TrustDecision { id, selection } => {
                self.close_selector(id);
                self.apply_trust_decision(selection);
            }
            UiMessage::SelectorCancelled { id } => {
                self.close_selector(id);
                self.ui.request_render();
            }
            UiMessage::VersionChecked { release } => {
                if let Some(release) = release {
                    self.show_new_version_notification(*release);
                }
            }
            UiMessage::PackageUpdatesChecked { packages } => {
                if !packages.is_empty() {
                    self.show_package_update_notification(packages);
                }
            }
            UiMessage::Report { message, level } => match level {
                HookReportLevel::Error => self.show_error(&message),
                _ => self.show_warning(&message),
            },
            UiMessage::Notify { message, level } => match level {
                NotifyLevel::Error => self.show_error(&message),
                NotifyLevel::Warning => self.show_warning(&message),
                NotifyLevel::Info => self.show_status(&message),
            },
            UiMessage::LlamaFinished { error, saved_text } => {
                self.restore_editor(&saved_text);
                if let Some(error) = error {
                    self.show_error(&error);
                }
            }
            UiMessage::ThemeChanged => {
                self.ui.invalidate();
                self.update_editor_border_color();
                self.ui.request_render();
            }
        }
    }

    /// `onChange` and `onSubmit` of the default editor, drained after the input
    /// was dispatched.
    async fn drain_editor(&mut self) {
        let (changes, submitted) = {
            let mut editor = self.editor.borrow_mut();
            let editor = editor.editor_mut();
            (editor.take_changes(), editor.take_submitted())
        };
        if let Some(text) = changes.last() {
            let was_bash_mode = self.is_bash_mode;
            self.is_bash_mode = text.trim_start().starts_with('!');
            if was_bash_mode != self.is_bash_mode {
                self.update_editor_border_color();
            }
        }
        for text in submitted {
            self.handle_submit(text).await;
        }
    }

    /// The slash-command table, the bash mode and the compaction queue arrive
    /// with the slices that own them; what is wired here is the path a plain
    /// prompt takes.
    async fn handle_submit(&mut self, text: String) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }

        // While the panel is open the editor belongs to it: a line typed under
        // an open side question is a follow-up, not a prompt for the main
        // agent. A slash command still gets through, so `/btw` can replace the
        // panel and every other command keeps working.
        if self.side_question_takes_input() && !text.starts_with('/') {
            self.submit_to_side_question(&text);
            return;
        }

        if self.handle_slash_command(&text).await {
            return;
        }

        // Bash command (`!` runs it, `!!` keeps the output out of the context).
        if let Some(rest) = text.strip_prefix('!') {
            let is_excluded = rest.starts_with('!');
            let command = rest.trim_start_matches('!').trim().to_owned();
            if !command.is_empty() {
                if self.session().is_bash_running() {
                    self.show_warning(
                        "A bash command is already running. Press Esc to cancel it first.",
                    );
                    self.editor.borrow_mut().editor_mut().set_text(&text);
                    return;
                }
                self.editor.borrow_mut().editor_mut().add_to_history(&text);
                self.handle_bash_command(&command, is_excluded);
                return;
            }
        }

        // Queue input while a compaction runs.
        if self.session().is_compacting() {
            self.queue_compaction_message(&text, CompactionQueueMode::Steer);
            return;
        }

        // If streaming, `prompt()` with steering behaviour queues the message.
        if self.session().is_streaming() {
            self.editor.borrow_mut().editor_mut().add_to_history(&text);
            self.editor.borrow_mut().editor_mut().set_text("");
            let images = self.images_for_text(&text);
            self.clear_pasted_images();
            let session = self.session();
            if let Err(message) = session
                .prompt(
                    &text,
                    PromptOptions {
                        images,
                        streaming_behavior: Some(QueueBehavior::Steer),
                        ..PromptOptions::default()
                    },
                )
                .await
            {
                self.show_error(&message);
            }
            self.update_pending_messages_display();
            self.ui.request_render();
            return;
        }

        // Normal submission: the bash rows of this turn move into the transcript.
        self.flush_pending_bash_components();
        self.pending_user_inputs.push_back(text.clone());
        self.editor.borrow_mut().editor_mut().add_to_history(&text);
    }

    fn record_version_seen(&self) {
        if !self.session().messages().is_empty() {
            return;
        }
        let settings = self.settings();
        let Some(last_version) = settings.get_last_changelog_version() else {
            settings.set_last_changelog_version(VERSION);
            crate::core::telemetry::report_install_telemetry(&settings, VERSION);
            return;
        };
        let entries =
            crate::utils::changelog::parse_changelog(&crate::config::get_changelog_path());
        if crate::utils::changelog::get_new_entries(&entries, &last_version).is_empty() {
            return;
        }
        settings.set_last_changelog_version(VERSION);
        crate::core::telemetry::report_install_telemetry(&settings, VERSION);
    }

    fn check_tmux_keyboard_setup() -> Option<String> {
        std::env::var("TMUX")
            .ok()
            .filter(|value| !value.is_empty())?;
        let show = |option: &str| -> Option<String> {
            let output = std::process::Command::new("tmux")
                .args(["show", "-gv", option])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        // Could not query tmux (sandbox, timeout): no warning.
        let extended_keys = show("extended-keys")?;
        if extended_keys != "on" && extended_keys != "always" {
            return Some(
                "tmux extended-keys is off. Modified Enter keys may not work. Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux."
                    .to_owned(),
            );
        }
        if show("extended-keys-format").as_deref() == Some("xterm") {
            return Some(
                "tmux extended-keys-format is xterm. Notagent works best with csi-u. Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux."
                    .to_owned(),
            );
        }
        None
    }

    fn show_new_version_notification(&mut self, release: LatestPiRelease) {
        let command = match detect_install_method(&InstallEnv::current()) {
            InstallMethod::Homebrew => "brew upgrade notagent".to_owned(),
            _ => format!("{APP_NAME} update"),
        };
        let action = theme().fg(ThemeColor::Accent, &command);
        let update_instruction = format!(
            "{}{action}",
            theme().fg(
                ThemeColor::Muted,
                &format!("New version {} is available. Run ", release.version)
            )
        );
        let changelog_url = "https://notagent.dev/changelog";
        let changelog_line = format!(
            "{}{}",
            theme().fg(ThemeColor::Muted, "Changelog: "),
            theme().fg(ThemeColor::Accent, changelog_url)
        );
        let border = || {
            component_ref(DynamicBorder::new(Some(Rc::new(|text: &str| {
                theme().fg(ThemeColor::Warning, text)
            }))))
        };
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(border());
        chat.add_child(component_ref(Text::new(
            format!(
                "{}\n{update_instruction}",
                theme().bold(&theme().fg(ThemeColor::Warning, "Update Available"))
            ),
            1,
            0,
        )));
        if let Some(note) = release
            .note
            .as_ref()
            .map(|note| note.trim())
            .filter(|note| !note.is_empty())
        {
            chat.add_child(component_ref(Spacer::new(1)));
            chat.add_child(component_ref(Markdown::new(
                note,
                1,
                0,
                get_markdown_theme(),
                None,
                None,
            )));
            chat.add_child(component_ref(Spacer::new(1)));
        }
        chat.add_child(component_ref(Text::new(changelog_line, 1, 0)));
        chat.add_child(border());
        drop(chat);
        self.ui.request_render();
    }

    fn show_package_update_notification(&mut self, packages: Vec<String>) {
        let action = theme().fg(
            ThemeColor::Accent,
            &format!("{APP_NAME} update --extensions"),
        );
        let update_instruction = format!(
            "{}{action}",
            theme().fg(ThemeColor::Muted, "Package updates are available. Run ")
        );
        let package_lines = packages
            .iter()
            .map(|package| format!("- {package}"))
            .collect::<Vec<_>>()
            .join("\n");
        let border = || {
            component_ref(DynamicBorder::new(Some(Rc::new(|text: &str| {
                theme().fg(ThemeColor::Warning, text)
            }))))
        };
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(border());
        chat.add_child(component_ref(Text::new(
            format!(
                "{}\n{update_instruction}\n{}\n{package_lines}",
                theme().bold(&theme().fg(ThemeColor::Warning, "Package Updates Available")),
                theme().fg(ThemeColor::Muted, "Packages:")
            ),
            1,
            0,
        )));
        chat.add_child(border());
        drop(chat);
        self.ui.request_render();
    }

    /// `maybeShowCacheMissNotice(message)` and `addCacheMissNotice(miss)`
    fn maybe_show_cache_miss_notice(&mut self, message: &AssistantMessage) {
        if !self.settings().get_show_cache_miss_notices() {
            return;
        }
        // Entries do not contain `message` yet: `message_end` fires before it is
        // persisted.
        let entries = self
            .session()
            .with_session_manager(|manager| manager.get_entries());
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        if let Some(miss) = detect_cache_miss(&entries, message, &*model_runtime) {
            self.add_cache_miss_notice(miss);
        }
    }

    fn add_cache_miss_notice(&mut self, miss: CacheMiss) {
        if miss.missed_tokens < 20_000 && miss.missed_cost < 0.1 {
            return;
        }
        let cost = if miss.missed_cost >= 0.01 {
            format!(" (~${:.2})", miss.missed_cost)
        } else {
            String::new()
        };
        let re_billed = format!(
            "{} tokens re-billed{cost}",
            format_tokens(miss.missed_tokens)
        );
        let label = if miss.model_changed {
            "Cache miss after model switch".to_owned()
        } else if miss.idle_ms >= CACHE_TTL_MS {
            format!(
                "Cache miss after {}m idle",
                (miss.idle_ms as f64 / 60_000.0).round() as i64
            )
        } else {
            "Cache miss".to_owned()
        };
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(Text::new(
            theme().fg(ThemeColor::Warning, &format!("{label}: {re_billed}")),
            1,
            0,
        )));
    }

    /// `maybeWarnAboutAnthropicSubscriptionAuth(model)`
    async fn maybe_warn_about_anthropic_subscription_auth(&mut self, model: Option<Model>) {
        if self.settings().get_warnings().anthropic_extra_usage == Some(false)
            || self.anthropic_subscription_warning_shown
        {
            return;
        }
        let model = model.or_else(|| self.session().model());
        let Some(model) = model.filter(|model| model.provider == "anthropic") else {
            return;
        };
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        if model_runtime
            .check_auth("anthropic", None)
            .await
            .ok()
            .flatten()
            .is_some_and(|check| check.check_type == AuthType::OAuth)
        {
            self.anthropic_subscription_warning_shown = true;
            self.show_warning(ANTHROPIC_SUBSCRIPTION_AUTH_WARNING);
            return;
        }
        let api_key = model_runtime
            .get_auth_for_provider(&model.provider, None)
            .await
            .ok()
            .flatten()
            .and_then(|result| result.auth.api_key);
        if api_key.is_some_and(|key| key.starts_with("sk-ant-oat")) {
            self.anthropic_subscription_warning_shown = true;
            self.show_warning(ANTHROPIC_SUBSCRIPTION_AUTH_WARNING);
        }
    }

    // ------------------------------------------------------------------
    // The loaded resources
    // ------------------------------------------------------------------

    fn format_display_path(&self, path: &str) -> String {
        match dirs::home_dir() {
            Some(home) => {
                let home = home.to_string_lossy().into_owned();
                match path.strip_prefix(&home) {
                    Some(rest) => format!("~{rest}"),
                    None => path.to_owned(),
                }
            }
            None => path.to_owned(),
        }
    }

    fn format_context_path(&self, path: &str) -> String {
        let cwd = self.cwd();
        match crate::utils::paths::get_cwd_relative_path(path, &cwd) {
            Ok(Some(relative)) => relative,
            _ => self.format_display_path(path),
        }
    }

    fn startup_expansion_state(&self) -> bool {
        self.options.verbose || self.tool_output_expanded
    }

    fn is_package_source(source_info: Option<&SourceInfo>) -> bool {
        source_info.is_some_and(|source_info| {
            source_info.source.starts_with("npm:") || source_info.source.starts_with("git:")
        })
    }

    fn scope_group(source_info: Option<&SourceInfo>) -> &'static str {
        let source = source_info.map_or("local", |source_info| source_info.source.as_str());
        let scope = source_info.map_or(SourceScope::Project, |source_info| source_info.scope);
        if source == "cli" || scope == SourceScope::Temporary {
            return "path";
        }
        match scope {
            SourceScope::User => "user",
            SourceScope::Project => "project",
            SourceScope::Temporary => "path",
        }
    }

    /// checkout layouts.
    fn short_path(&self, full_path: &str, source_info: Option<&SourceInfo>) -> String {
        let normalized = full_path.replace('\\', "/");
        if Self::is_package_source(source_info)
            && let Some(base_dir) =
                source_info.and_then(|source_info| source_info.base_dir.as_ref())
        {
            let normalized_base = base_dir.replace('\\', "/");
            if let Some(rest) = normalized.strip_prefix(&format!("{normalized_base}/"))
                && !rest.is_empty()
            {
                return rest.to_owned();
            }
        }
        let source = source_info.map_or("", |source_info| source_info.source.as_str());
        if source.starts_with("npm:")
            && let Some(index) = normalized.find("node_modules/")
        {
            let rest = &normalized[index + "node_modules/".len()..];
            // `node_modules/<scope>/<name>/<rest>` or `node_modules/<name>/<rest>`
            let segments: Vec<&str> = rest.split('/').collect();
            let skip = if rest.starts_with('@') { 2 } else { 1 };
            if segments.len() > skip {
                return segments[skip..].join("/");
            }
        }
        if source.starts_with("git:")
            && let Some(index) = normalized.find("git/")
        {
            let rest = &normalized[index + "git/".len()..];
            let segments: Vec<&str> = rest.split('/').collect();
            if segments.len() > 2 {
                return segments[2..].join("/");
            }
        }
        self.format_display_path(full_path)
    }

    /// `buildScopeGroups(items)` and `formatScopeGroups(groups, options)`
    fn format_scope_groups(
        &self,
        items: &[(String, Option<SourceInfo>)],
        format_path: &dyn Fn(&str, Option<&SourceInfo>) -> String,
        format_package_path: &dyn Fn(&str, Option<&SourceInfo>) -> String,
    ) -> String {
        let mut lines: Vec<String> = Vec::new();
        for scope in ["project", "user", "path"] {
            let members: Vec<&(String, Option<SourceInfo>)> = items
                .iter()
                .filter(|(_, source_info)| Self::scope_group(source_info.as_ref()) == scope)
                .collect();
            if members.is_empty() {
                continue;
            }
            lines.push(format!("  {}", theme().fg(ThemeColor::Accent, scope)));

            let mut paths: Vec<&(String, Option<SourceInfo>)> = members
                .iter()
                .copied()
                .filter(|(_, source_info)| !Self::is_package_source(source_info.as_ref()))
                .collect();
            paths.sort_by(|left, right| left.0.cmp(&right.0));
            for (path, source_info) in paths {
                lines.push(theme().fg(
                    ThemeColor::Dim,
                    &format!("    {}", format_path(path, source_info.as_ref())),
                ));
            }

            let mut sources: Vec<&str> = members
                .iter()
                .filter(|(_, source_info)| Self::is_package_source(source_info.as_ref()))
                .map(|(_, source_info)| {
                    source_info
                        .as_ref()
                        .map_or("local", |source_info| source_info.source.as_str())
                })
                .collect();
            sources.sort_unstable();
            sources.dedup();
            for source in sources {
                lines.push(format!("    {}", theme().fg(ThemeColor::MdLink, source)));
                let mut package_paths: Vec<&(String, Option<SourceInfo>)> = members
                    .iter()
                    .copied()
                    .filter(|(_, source_info)| {
                        source_info
                            .as_ref()
                            .is_some_and(|source_info| source_info.source == source)
                    })
                    .collect();
                package_paths.sort_by(|left, right| left.0.cmp(&right.0));
                for (path, source_info) in package_paths {
                    lines.push(theme().fg(
                        ThemeColor::Dim,
                        &format!("      {}", format_package_path(path, source_info.as_ref())),
                    ));
                }
            }
        }
        lines.join("\n")
    }

    fn format_diagnostics(
        &self,
        diagnostics: &[ResourceDiagnostic],
        source_infos: &[(String, SourceInfo)],
    ) -> String {
        let find_source_info = |path: &str| -> Option<&SourceInfo> {
            if let Some((_, source_info)) =
                source_infos.iter().find(|(candidate, _)| candidate == path)
            {
                return Some(source_info);
            }
            let mut current = path.to_owned();
            while let Some(index) = current.rfind('/') {
                current.truncate(index);
                if let Some((_, source_info)) = source_infos
                    .iter()
                    .find(|(candidate, _)| *candidate == current)
                {
                    return Some(source_info);
                }
            }
            None
        };
        let format_path_with_source = |path: &str| -> String {
            match find_source_info(path) {
                Some(source_info) => {
                    let short_path = self.short_path(path, Some(source_info));
                    let (label, scope_label) = display_source_label(source_info);
                    let label_text = match scope_label {
                        Some(scope_label) => format!("{label} ({scope_label})"),
                        None => label.to_owned(),
                    };
                    format!("{label_text} {short_path}")
                }
                None => self.format_display_path(path),
            }
        };

        let mut lines: Vec<String> = Vec::new();
        // Collisions are grouped by name, in first-seen order.
        let mut collision_names: Vec<String> = Vec::new();
        for diagnostic in diagnostics {
            if let Some(collision) = diagnostic.collision.as_ref()
                && !collision_names.contains(&collision.name)
            {
                collision_names.push(collision.name.clone());
            }
        }
        for name in &collision_names {
            let group: Vec<&ResourceDiagnostic> = diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic
                        .collision
                        .as_ref()
                        .is_some_and(|collision| &collision.name == name)
                })
                .collect();
            let Some(first) = group.first().and_then(|first| first.collision.as_ref()) else {
                continue;
            };
            lines.push(theme().fg(ThemeColor::Warning, &format!("  \"{name}\" collision:")));
            lines.push(theme().fg(
                ThemeColor::Dim,
                &format!(
                    "    {} {}",
                    theme().fg(ThemeColor::Success, "✓"),
                    format_path_with_source(&first.winner_path)
                ),
            ));
            for diagnostic in group {
                if let Some(collision) = diagnostic.collision.as_ref() {
                    lines.push(theme().fg(
                        ThemeColor::Dim,
                        &format!(
                            "    {} {} (skipped)",
                            theme().fg(ThemeColor::Warning, "✗"),
                            format_path_with_source(&collision.loser_path)
                        ),
                    ));
                }
            }
        }

        for diagnostic in diagnostics {
            if diagnostic.collision.is_some() {
                continue;
            }
            let color = if diagnostic.level == DiagnosticLevel::Error {
                ThemeColor::Error
            } else {
                ThemeColor::Warning
            };
            match diagnostic.path.as_ref() {
                Some(path) => {
                    lines.push(theme().fg(color, &format!("  {}", format_path_with_source(path))));
                    lines.push(theme().fg(color, &format!("    {}", diagnostic.message)));
                }
                None => lines.push(theme().fg(color, &format!("  {}", diagnostic.message))),
            }
        }
        lines.join("\n")
    }

    /// the extension sections and their diagnostics (class 2).
    fn show_loaded_resources(&mut self, force: bool, show_diagnostics_when_quiet: bool) {
        self.loaded_resources_container.borrow_mut().clear();
        let settings = self.settings();
        let show_listing = force || self.options.verbose || !settings.get_quiet_startup();
        let show_diagnostics = show_listing || show_diagnostics_when_quiet;
        if !show_listing && !show_diagnostics {
            return;
        }

        let resource_loader = self.session().resource_loader();
        let (skills, skill_diagnostics) = resource_loader.get_skills();
        let (prompts, prompt_diagnostics) = resource_loader.get_prompts();
        let (themes, theme_diagnostics) = resource_loader.get_themes();

        let mut source_infos: Vec<(String, SourceInfo)> = Vec::new();
        for skill in &skills {
            source_infos.push((skill.file_path.clone(), skill.source_info.clone()));
        }
        for prompt in &prompts {
            source_infos.push((prompt.file_path.clone(), prompt.source_info.clone()));
        }
        for loaded_theme in &themes {
            if let (Some(path), Some(source_info)) =
                (&loaded_theme.source_path, &loaded_theme.source_info)
            {
                source_infos.push((path.clone(), source_info.clone()));
            }
        }

        if show_listing {
            let mut context_files: Vec<String> = Vec::new();
            if let Some(system_prompt) = resource_loader.get_system_prompt_source() {
                context_files.push(system_prompt);
            }
            context_files.extend(resource_loader.get_append_system_prompt_sources());
            context_files.extend(
                resource_loader
                    .get_agents_files()
                    .into_iter()
                    .map(|file| file.path),
            );
            if !context_files.is_empty() {
                self.loaded_resources_container
                    .borrow_mut()
                    .add_child(component_ref(Spacer::new(1)));
                let expanded = context_files
                    .iter()
                    .map(|path| {
                        theme().fg(
                            ThemeColor::Dim,
                            &format!("  {}", self.format_display_path(path)),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let collapsed = compact_list(
                    &context_files
                        .iter()
                        .map(|path| self.format_context_path(path))
                        .collect::<Vec<_>>(),
                    false,
                );
                self.add_loaded_section("Context", &collapsed, &expanded);
            }

            if !skills.is_empty() {
                let items: Vec<(String, Option<SourceInfo>)> = skills
                    .iter()
                    .map(|skill| (skill.file_path.clone(), Some(skill.source_info.clone())))
                    .collect();
                let expanded = self.format_scope_groups(
                    &items,
                    &|path, _| self.format_display_path(path),
                    &|path, source_info| self.short_path(path, source_info),
                );
                let collapsed = compact_list(
                    &skills
                        .iter()
                        .map(|skill| skill.name.clone())
                        .collect::<Vec<_>>(),
                    true,
                );
                self.add_loaded_section("Skills", &collapsed, &expanded);
            }

            let templates = self.session().prompt_templates();
            if !templates.is_empty() {
                let items: Vec<(String, Option<SourceInfo>)> = templates
                    .iter()
                    .map(|template| {
                        (
                            template.file_path.clone(),
                            Some(template.source_info.clone()),
                        )
                    })
                    .collect();
                let names: Vec<(String, String)> = templates
                    .iter()
                    .map(|template| (template.file_path.clone(), format!("/{}", template.name)))
                    .collect();
                let by_path = |path: &str, _: Option<&SourceInfo>| -> String {
                    names
                        .iter()
                        .find(|(file_path, _)| file_path == path)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_else(|| self.format_display_path(path))
                };
                let expanded = self.format_scope_groups(&items, &by_path, &by_path);
                let collapsed = compact_list(
                    &templates
                        .iter()
                        .map(|template| format!("/{}", template.name))
                        .collect::<Vec<_>>(),
                    true,
                );
                self.add_loaded_section("Prompts", &collapsed, &expanded);
            }

            let custom_themes: Vec<&crate::modes::interactive::theme::theme::Theme> = themes
                .iter()
                .filter(|loaded_theme| loaded_theme.source_path.is_some())
                .collect();
            if !custom_themes.is_empty() {
                let items: Vec<(String, Option<SourceInfo>)> = custom_themes
                    .iter()
                    .map(|loaded_theme| {
                        (
                            loaded_theme.source_path.clone().unwrap_or_default(),
                            loaded_theme.source_info.clone(),
                        )
                    })
                    .collect();
                let expanded = self.format_scope_groups(
                    &items,
                    &|path, _| self.format_display_path(path),
                    &|path, source_info| self.short_path(path, source_info),
                );
                let collapsed = compact_list(
                    &custom_themes
                        .iter()
                        .map(|loaded_theme| {
                            loaded_theme.name.clone().unwrap_or_else(|| {
                                let path = loaded_theme.source_path.clone().unwrap_or_default();
                                compact_path_label(
                                    &self.short_path(&path, loaded_theme.source_info.as_ref()),
                                )
                            })
                        })
                        .collect::<Vec<_>>(),
                    true,
                );
                self.add_loaded_section("Themes", &collapsed, &expanded);
            }
        }

        if show_diagnostics {
            for (title, diagnostics) in [
                ("[Skill conflicts]", &skill_diagnostics),
                ("[Prompt conflicts]", &prompt_diagnostics),
                ("[Theme conflicts]", &theme_diagnostics),
            ] {
                if diagnostics.is_empty() {
                    continue;
                }
                let warning_lines = self.format_diagnostics(diagnostics, &source_infos);
                let mut container = self.loaded_resources_container.borrow_mut();
                container.add_child(component_ref(Text::new(
                    format!(
                        "{}\n{warning_lines}",
                        theme().fg(ThemeColor::Warning, title)
                    ),
                    0,
                    0,
                )));
                container.add_child(component_ref(Spacer::new(1)));
            }
        }
        self.ui.request_render();
    }

    /// `addLoadedSection(name, collapsedBody, expandedBody)` of
    /// `showLoadedResources`.
    fn add_loaded_section(&mut self, name: &str, collapsed: &str, expanded: &str) {
        let header = theme().fg(ThemeColor::MdHeading, &format!("[{name}]"));
        let section = Rc::new(RefCell::new(ExpandableText::new(
            format!("{header}\n{collapsed}"),
            format!("{header}\n{expanded}"),
            self.startup_expansion_state(),
            0,
        )));
        let mut container = self.loaded_resources_container.borrow_mut();
        container.add_child(Rc::clone(&section) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));
        drop(container);
        self.chat_expandables
            .push(section as Rc<RefCell<dyn Expandable>>);
    }

    // ------------------------------------------------------------------
    // Panels
    // ------------------------------------------------------------------

    /// `refreshSubagentPanel` — driven by the loop's one-second tick instead of
    /// `setInterval`.
    fn refresh_tasks_panel(&mut self) {
        let manager = self.session().task_manager();
        // Settled work is included, which is what lets the lifecycle lines
        // report an outcome at all: `list(true, …)` drops a task the moment it
        // ends, so the announcement never saw one finish. The panel filters it
        // back out itself — it shows what is running, the transcript reports
        // what happened. Shell tasks have no panel of their own any more: the
        // footer counts them and `/tasks` holds the detail (user decision
        // 2026-08-31).
        let all_tasks = manager
            .as_ref()
            .map(|manager| manager.list(false, None))
            .unwrap_or_default();
        self.refresh_subagent_panel(&all_tasks);

        self.has_foreground_tasks.set(
            all_tasks
                .iter()
                .any(|info| info.base().detached == Some(false)),
        );
    }

    /// Appends one lifecycle line to the transcript. It is a non-search
    /// addition, so it also ends an open search block.
    fn append_task_entry(&mut self, line: String) {
        self.close_explore_block();
        let mut chat = self.chat_container.borrow_mut();
        if !chat.children.is_empty() {
            chat.add_child(component_ref(Spacer::new(1)));
        }
        chat.add_child(component_ref(Text::new(line, 0, 0)));
        drop(chat);
        self.ui.request_render();
    }

    fn append_task_lifecycle(&mut self, record: &TaskLifecycleRecord) {
        let line = task_lifecycle_line(record, &theme(), block_style() == BlockStyle::Badge);
        self.append_task_entry(line);
    }

    fn refresh_subagent_panel(&mut self, tasks: &[TaskInfo]) {
        // The signature includes settled children while they linger, so the
        // marker turning green or red repaints, and so does the row leaving
        // once its linger runs out.
        let now = crate::modes::interactive::components::tasks_panel::now_ms();
        let listed: Vec<&TaskInfo> = tasks
            .iter()
            .filter(|info| {
                crate::modes::interactive::components::subagent_panel::is_listed_subagent(info, now)
            })
            .collect();
        let any_running = listed
            .iter()
            .any(|info| info.status() == TaskStatus::Running);
        let signature = listed
            .iter()
            .map(|info| format!("{}:{}", info.task_id(), info.status().as_str()))
            .collect::<Vec<_>>()
            .join("|");
        self.subagent_panel.borrow_mut().set_tasks(tasks.to_vec());
        if !any_running && signature == self.subagent_panel_signature {
            return;
        }
        self.subagent_panel_signature = signature;
        self.ui.request_render();
    }

    fn sync_todo_panel(&mut self, todos: Vec<Todo>, from_tool_call: bool) {
        self.todo_visibility.update(todos, from_tool_call);
        let rows = self.ui.rows();
        let visible = if self.todo_visibility.visible() {
            self.todo_visibility.current().to_vec()
        } else {
            Vec::new()
        };
        let mut panel = self.todo_panel.borrow_mut();
        panel.set_terminal_rows(rows);
        panel.set_todos(visible);
    }

    fn clear_todo_panel(&mut self) {
        self.todo_visibility.reset();
        self.todo_panel.borrow_mut().set_todos(Vec::new());
    }

    /// Returns `false` when there is nothing to move, so the key falls through
    /// to the editor — it is also the cursor-left binding.
    fn detach_foreground_tasks(&mut self) -> bool {
        let Some(manager) = self.session().task_manager() else {
            return false;
        };
        let running: Vec<String> = manager
            .list(true, None)
            .into_iter()
            .filter(|info| info.base().detached == Some(false))
            .map(|info| info.task_id().to_owned())
            .collect();
        if running.is_empty() {
            return false;
        }
        for task_id in running {
            manager.detach(&task_id);
        }
        self.refresh_tasks_panel();
        true
    }

    // ------------------------------------------------------------------
    // Key actions
    // ------------------------------------------------------------------

    fn cycle_operating_mode(&mut self) {
        let Some(mode) = self.session().cycle_mode() else {
            self.show_status("No operating modes available");
            return;
        };
        self.footer.borrow_mut().invalidate();
        let notice = format_mode_switch_notice(
            &mode.id,
            mode.shell,
            self.session().active_mode_injected_tokens(),
        );
        self.show_status(&notice);
    }

    fn cycle_thinking_level(&mut self) {
        match self.session().cycle_thinking_level() {
            None => self.show_status("Current model does not support thinking"),
            Some(level) => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
                self.show_status(&format!("Thinking level: {}", thinking_level_name(level)));
            }
        }
    }

    fn cycle_model(&mut self, forward: bool) {
        match self.session().cycle_model(forward) {
            None => {
                let message = if self.session().scoped_models().is_empty() {
                    "Only one model available"
                } else {
                    "Only one model in scope"
                };
                self.show_status(message);
            }
            Some(result) => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
                let thinking = if result.model.reasoning
                    && result.thinking_level != notagent_agent::types::ThinkingLevel::Off
                {
                    format!(
                        " (thinking: {})",
                        thinking_level_name(result.thinking_level)
                    )
                } else {
                    String::new()
                };
                let name = if result.model.name.is_empty() {
                    result.model.id.clone()
                } else {
                    result.model.name.clone()
                };
                self.show_status(&format!("Switched to {name}{thinking}"));
                self.check_daxnuts_easter_egg(&result.model);
            }
        }
    }

    /// `toggleToolOutputExpansion()`/`setToolsExpanded(expanded)`
    fn toggle_tool_output_expansion(&mut self) {
        let expanded = !self.tool_output_expanded;
        self.tool_output_expanded = expanded;
        for component in self.chat_expandables.iter() {
            component.borrow_mut().set_expanded(expanded);
        }
        self.show_status(&format!(
            "Tool output: {}",
            if expanded { "expanded" } else { "collapsed" }
        ));
    }

    fn toggle_thinking_block_visibility(&mut self) {
        self.hide_thinking_block = !self.hide_thinking_block;
        self.settings()
            .set_hide_thinking_block(self.hide_thinking_block);
        self.rebuild_chat_from_messages();
        if let (Some(component), Some(message)) = (
            self.streaming_component.clone(),
            self.streaming_message.clone(),
        ) {
            {
                let mut component = component.borrow_mut();
                component.set_hide_thinking_block(self.hide_thinking_block);
                component.update_content(message, None);
            }
            self.chat_container
                .borrow_mut()
                .add_child(Rc::clone(&component) as ComponentRef);
        }
        self.show_status(&format!(
            "Thinking blocks: {}",
            if self.hide_thinking_block {
                "hidden"
            } else {
                "visible"
            }
        ));
    }

    /// caller's render loop finds the screen back up afterwards.
    fn handle_open_external_editor(&mut self) {
        let command = self.settings().get_external_editor_command();
        let content = self.editor.borrow().editor().get_expanded_text();
        self.cell.stop(TuiStopOptions::default());
        let result = edit_in_external_editor(&ExternalEditorOptions { command, content });
        if let ExternalEditorResult::Complete(content) = result {
            self.editor.borrow_mut().editor_mut().set_text(&content);
        }
        self.cell.start();
        self.cell.request_render(true);
    }

    /// The double-escape branch of `setupKeyHandlers`
    fn handle_double_escape(&mut self) {
        let action = self.settings().get_double_escape_action();
        if action == DoubleEscapeAction::None {
            return;
        }
        let now = Instant::now();
        match self.last_escape_time {
            Some(last) if now.duration_since(last) < Duration::from_millis(500) => {
                self.last_escape_time = None;
                match action {
                    DoubleEscapeAction::Tree => self.show_tree_selector(None),
                    _ => self.show_user_message_selector(),
                }
            }
            _ => self.last_escape_time = Some(now),
        }
    }

    // ------------------------------------------------------------------
    // Selectors
    // ------------------------------------------------------------------

    /// The selector takes the editor's place; `done` — here
    /// [`Self::close_selector`] with the id the selector was opened under —
    /// token: a message from a selector that has already been replaced must not
    /// close its successor.
    fn show_selector(
        &mut self,
        component: ComponentRef,
        focus: ComponentRef,
        dispose: Option<Box<dyn FnOnce()>>,
    ) -> u64 {
        self.dispose_active_selector();
        self.selector_id += 1;
        let id = self.selector_id;
        self.active_selector = Some(ActiveSelector { id, dispose });
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(component);
        }
        self.ui.set_focus(Some(focus));
        self.ui.request_render();
        id
    }

    fn dispose_active_selector(&mut self) {
        if let Some(selector) = self.active_selector.take()
            && let Some(dispose) = selector.dispose
        {
            dispose();
        }
    }

    /// The `done` callback of `showSelector`.
    fn close_selector(&mut self, id: u64) {
        if self
            .active_selector
            .as_ref()
            .is_none_or(|active| active.id != id)
        {
            return;
        }
        self.dispose_active_selector();
        self.session_selector = None;
        self.model_selector = None;
        self.scoped_models_selector = None;
        if let Some((browser, _)) = self.tasks_browser.take() {
            browser.borrow_mut().dispose();
        }
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.ui.request_render();
    }

    fn show_model_selector(&mut self, initial_search_input: Option<&str>) {
        self.show_model_selector_impl(initial_search_input, false);
    }

    fn show_effort_selector(&mut self) {
        if !self.session().supports_thinking() {
            self.show_status("Current model does not support reasoning effort");
            return;
        }
        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let selector = Rc::new(RefCell::new(ThinkingSelectorComponent::new(
            self.session().thinking_level(),
            self.session().get_available_thinking_levels(),
            Box::new(move |level| {
                let _ = select_tx.send(UiMessage::EffortSelected { id, level });
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
        )));
        let focus = selector.borrow().get_select_list() as ComponentRef;
        self.show_selector(selector, focus, None);
    }

    /// The same selector aimed at the `/subagent-model` setting (port
    /// addition, v0.1.6): only the message the choice sends back differs.
    fn show_subagent_model_selector(&mut self, initial_search_input: Option<&str>) {
        self.show_model_selector_impl(initial_search_input, true);
    }

    fn show_model_selector_impl(&mut self, initial_search_input: Option<&str>, for_subagent: bool) {
        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let selector = Rc::new(RefCell::new(ModelSelectorComponent::new(
            {
                let core = self.ui.clone();
                Rc::new(move || core.request_render())
            },
            self.session().model(),
            self.settings(),
            Arc::clone(&self.runtime.services().model_runtime),
            // The two `ScopedModel` types are structurally identical; the
            // selector takes the one of `model_resolver`, the session hands out
            self.session()
                .scoped_models()
                .into_iter()
                .map(|scoped| crate::core::model_resolver::ScopedModel {
                    model: scoped.model,
                    thinking_level: scoped.thinking_level,
                })
                .collect(),
            Box::new(move |model| {
                let message = if for_subagent {
                    UiMessage::SubagentModelSelected {
                        id,
                        model: Box::new(model),
                    }
                } else {
                    UiMessage::ModelSelected {
                        id,
                        model: Box::new(model),
                    }
                };
                let _ = select_tx.send(message);
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
            initial_search_input,
        )));
        self.model_selector = Some(Rc::clone(&selector));
        // constructor cannot own that task, so the loop drives the future and
        // the outcome returns as a message. Without this the picker only ever
        // shows the startup snapshot and its "Refreshing model catalogs…"
        // line never resolves.
        let refresh = selector.borrow().refresh_models();
        self.side_futures.push(Box::pin(async move {
            UiMessage::ModelCatalogRefreshed {
                id,
                outcome: refresh.await,
            }
        }));
        let dispose_target = Rc::clone(&selector);
        self.show_selector(
            Rc::clone(&selector) as ComponentRef,
            Rc::clone(&selector) as ComponentRef,
            Some(Box::new(move || dispose_target.borrow_mut().dispose())),
        );
    }

    /// The `onSelect` half of the model selector and of `/model <term>`.
    async fn apply_selected_model(&mut self, model: Model) {
        match self.session().set_model(model.clone()).await {
            Ok(()) => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
                self.show_status(&format!("Model: {}", model.id));
                self.check_daxnuts_easter_egg(&model);
            }
            Err(message) => self.show_error(&message),
        }
    }

    /// The `onSelect` half of the subagent-model selector and of
    /// `/model`; the child resolves it at spawn time and inherits the main
    /// model when the setting is absent or names a model that no longer
    /// exists.
    fn apply_selected_subagent_model(&mut self, model: Model) {
        self.settings()
            .set_subagent_model_and_provider(&model.provider, &model.id);
        self.show_status(&format!("Subagent model: {}", model.id));
    }

    /// selector, `default`/`inherit` clears the setting, anything else is
    /// matched like `/model <term>`.
    async fn handle_subagent_model_command(&mut self, search_term: Option<&str>) {
        let Some(search_term) = search_term else {
            self.show_subagent_model_selector(None);
            return;
        };
        if search_term.eq_ignore_ascii_case("default")
            || search_term.eq_ignore_ascii_case("inherit")
        {
            self.settings().clear_subagent_model();
            self.show_status("Subagent model: inherits the main model");
            return;
        }
        if let Some(model) = self.find_exact_model_match(search_term).await {
            self.apply_selected_subagent_model(model);
            return;
        }
        self.show_subagent_model_selector(Some(search_term));
    }

    /// Indexing settings section: no argument rebuilds the `find_codebase`
    /// index with progress in the status line, `on`/`off` toggles the
    /// setting. Enabling kicks off a build immediately, so the toggle is
    /// never a dead switch (like the reference's `toggle_cb_search`).
    fn handle_index_command(&mut self, argument: Option<&str>) {
        match argument {
            Some(argument) if argument.eq_ignore_ascii_case("off") => {
                self.settings().set_find_codebase_enabled(false);
                self.show_status("Codebase index: disabled");
            }
            Some(argument) if argument.eq_ignore_ascii_case("on") => {
                self.settings().set_find_codebase_enabled(true);
                self.start_index_build();
            }
            Some(other) => {
                self.show_error(&format!("Unknown /index argument: {other} (use on|off)"));
            }
            None => {
                if !self.settings().get_find_codebase_enabled() {
                    self.show_status("Codebase index: disabled (enable with /index on)");
                    return;
                }
                self.start_index_build();
            }
        }
    }

    /// reference's Atomic leases setting: `on`/`off` set the gate, an omitted
    /// argument toggles it, as the reference's own toggle does. Off by default
    /// (user decision 2026-08-17); the gate is read at call time, so the next
    /// `write`, `edit` or `patch_minified` already follows the new state.
    fn handle_leases_command(&mut self, argument: Option<&str>) {
        let enabled = match argument {
            Some(argument) if argument.eq_ignore_ascii_case("on") => true,
            Some(argument) if argument.eq_ignore_ascii_case("off") => false,
            Some(other) => {
                self.show_error(&format!("Unknown /leases argument: {other} (use on|off)"));
                return;
            }
            None => !self.settings().get_atomic_leases_enabled(),
        };
        self.settings().set_atomic_leases_enabled(enabled);
        self.show_status(if enabled {
            "Atomic file leases: enabled"
        } else {
            "Atomic file leases: disabled"
        });
    }

    /// Connects the configured MCP servers and registers their tools
    /// A project-local `.mcp.json` names programs to run on this machine, so
    /// one with no remembered answer is accepted only when the project itself
    /// is already trusted. Anything else waits for `/mcp` rather than
    /// interrupting startup with a dialog the user did not ask for.
    async fn load_mcp_servers(&mut self) {
        let session = Arc::clone(&self.runtime.session());
        let project_trusted = self.settings().is_project_trusted();
        let outcome = session
            .load_mcp_servers(|file| match file.scope {
                crate::core::mcp::McpConfigScope::User => {
                    Some(crate::core::mcp::McpTrustResponse::Accept)
                }
                crate::core::mcp::McpConfigScope::Project if project_trusted => {
                    Some(crate::core::mcp::McpTrustResponse::Accept)
                }
                crate::core::mcp::McpConfigScope::Project => None,
            })
            .await;
        if let Err(error) = outcome {
            self.show_error(&format!("MCP configuration ignored: {error}"));
            return;
        }
        let entries = session.mcp().entries().await;
        let untrusted = std::path::Path::new(&self.cwd()).join(".mcp.json");
        if entries.is_empty() && untrusted.exists() && !project_trusted {
            self.show_status(
                "This project has an .mcp.json, which names programs to run on your machine. Trust the project with /trust to use them.",
            );
            return;
        }
        let failed = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.status,
                    crate::core::mcp::manager::McpServerStatus::Failed
                        | crate::core::mcp::manager::McpServerStatus::NeedsAuth
                )
            })
            .count();
        if failed > 0 {
            self.show_warning(&format!(
                "{failed} of {} MCP servers need attention — see /mcp",
                entries.len()
            ));
        }
    }

    /// Bare `/mcp` reports each server's state and tool count; `reconnect
    /// <server>` retries one whose cause has been fixed. Six states are worth
    /// nothing if the user cannot see which one a server is in, and a failed
    /// server with no way to retry means a restart.
    async fn handle_mcp_command(&mut self, argument: Option<&str>) {
        use crate::core::mcp::ServerName;

        let session = Arc::clone(&self.runtime.session());
        let manager = session.mcp();
        match argument.map(str::trim) {
            None => {
                // Probed rather than read: a server that died while idle would
                // otherwise be listed as connected.
                let entries = manager.probed_entries().await;
                if entries.is_empty() {
                    self.show_status(
                        "No MCP servers configured. Add them to .mcp.json in this project or in the agent directory.",
                    );
                    return;
                }
                let mut lines = vec!["MCP servers:".to_owned()];
                for entry in entries {
                    let detail = match entry.error.as_deref() {
                        Some(error) => format!(" — {error}"),
                        None => String::new(),
                    };
                    // The stored sign-in is a separate fact from the connection
                    // state: a server can be connected on a token that is about
                    // to need renewing, and the user should see that before it
                    // starts failing.
                    let auth = match manager.auth_status(&entry.name).await {
                        Some(status) => format!(" · {}", status.as_str()),
                        None => String::new(),
                    };
                    lines.push(format!(
                        "  {} [{}] {} · {} tools{auth}{detail}",
                        entry.name,
                        entry.transport,
                        entry.status.as_str(),
                        entry.tool_count
                    ));
                }
                self.show_status(&lines.join("\n"));
            }
            Some(argument) => {
                if let Some(name) = argument.strip_prefix("reconnect").map(str::trim) {
                    if name.is_empty() {
                        self.show_error("Usage: /mcp reconnect <server>");
                        return;
                    }
                    let server = ServerName::from(name);
                    match manager.reconnect(&server).await {
                        Ok(count) => {
                            session.refresh_mcp_tools().await;
                            self.show_status(&format!("MCP `{name}`: connected, {count} tools"));
                        }
                        Err(error) => self.show_error(&format!("MCP `{name}`: {error}")),
                    }
                    return;
                }
                if let Some(name) = argument.strip_prefix("logout").map(str::trim) {
                    if name.is_empty() {
                        self.show_error("Usage: /mcp logout <server|all>");
                        return;
                    }
                    // `all` is spelled out rather than implied by a bare
                    // `logout`: forgetting every sign-in is not something to do
                    // by leaving a word off.
                    if name == "all" {
                        // The credential file is the user's, not the project's,
                        // so this works in a session with no `.mcp.json` at all
                        // — which is exactly where someone tidying up their
                        // stored sign-ins is likely to be standing.
                        let path = crate::config::get_mcp_credentials_path();
                        match crate::core::mcp::auth::forget_all(&path).await {
                            Ok(count) => {
                                manager.drop_authenticated_connections().await;
                                session.refresh_mcp_tools().await;
                                self.show_status(&format!(
                                    "MCP: forgot {count} stored sign-in{}",
                                    if count == 1 { "" } else { "s" }
                                ));
                            }
                            Err(error) => self.show_error(&format!("MCP: {error}")),
                        }
                        return;
                    }
                    match manager.sign_out(&ServerName::from(name)).await {
                        Ok(()) => {
                            session.refresh_mcp_tools().await;
                            self.show_status(&format!("MCP `{name}`: signed out"));
                        }
                        Err(error) => self.show_error(&format!("MCP `{name}`: {error}")),
                    }
                    return;
                }
                if let Some(rest) = argument.strip_prefix("lend").map(str::trim) {
                    self.handle_mcp_lend(rest).await;
                    return;
                }
                if let Some(rest) = argument.strip_prefix("import").map(str::trim) {
                    self.handle_mcp_import(rest).await;
                    return;
                }
                if let Some(rest) = argument.strip_prefix("remove").map(str::trim) {
                    self.handle_mcp_remove(rest).await;
                    return;
                }
                if let Some(name) = argument.strip_prefix("show").map(str::trim) {
                    self.handle_mcp_show(name).await;
                    return;
                }
                if argument == "reload" {
                    // Re-reads `.mcp.json` from disk, so a server added or
                    // edited during the session takes effect without a restart.
                    match session.reload_mcp_servers().await {
                        Ok(count) => self.show_status(&format!(
                            "MCP: reloaded, {count} server{} configured",
                            if count == 1 { "" } else { "s" }
                        )),
                        Err(error) => self.show_error(&format!("MCP: {error}")),
                    }
                    return;
                }
                let Some(name) = argument.strip_prefix("login").map(str::trim) else {
                    self.show_error(&format!(
                        "Unknown /mcp argument: {argument} (use `lend [off]`, `import [user] <json>`, \
                         `remove [user] <server>`, `show <server>`, \
                         `reconnect <server>`, `login <server>`, \
                         `logout <server|all>` or `reload`)"
                    ));
                    return;
                };
                if name.is_empty() {
                    self.show_error("Usage: /mcp login <server>");
                    return;
                }
                let server = ServerName::from(name);
                // The URL is printed the moment it exists rather than after the
                // wait: a browser that did not come up otherwise leaves the
                // user watching a login they cannot reach.
                let (announce_tx, announce_rx) = tokio::sync::oneshot::channel();
                let announce = move |prompt| {
                    let _ = announce_tx.send(prompt);
                };
                let login = manager.authenticate(&server, announce);
                tokio::pin!(login);
                let mut pending = Some(announce_rx);
                let outcome = loop {
                    let Some(mut announce_rx) = pending.take() else {
                        break login.await;
                    };
                    tokio::select! {
                        outcome = &mut login => break outcome,
                        announced = &mut announce_rx => {
                            if let Ok(prompt) = announced {
                                let prompt: crate::core::mcp::auth::McpAuthPrompt = prompt;
                                self.show_status(&format!(
                                    "Opening the browser to sign in to `{name}`.\n{}",
                                    prompt.authorization_url
                                ));
                            }
                        }
                    }
                };
                match outcome {
                    Ok(count) => {
                        session.refresh_mcp_tools().await;
                        self.show_status(&format!("MCP `{name}`: signed in, {count} tools"));
                    }
                    Err(error) => self.show_error(&format!("MCP `{name}`: {error}")),
                }
            }
        }
    }

    /// `/mcp lend [off]` — serves this session's tools to an external agent.
    /// Prints the `.mcp.json` entry rather than only the URL, because the token
    /// belongs in a header and a URL alone invites pasting it somewhere that
    /// drops the header and then fails with an unexplained 401.
    async fn handle_mcp_lend(&mut self, argument: &str) {
        if argument == "off" {
            let message = match self.session().stop_lending_tools() {
                true => "MCP: stopped lending; the token is revoked.",
                false => "MCP: nothing was being lent.",
            };
            self.show_status(message);
            return;
        }
        if !argument.is_empty() {
            self.show_error("Usage: /mcp lend [off]");
            return;
        }

        let session = Arc::clone(&self.runtime.session());
        match session.lend_tools().await {
            Ok(endpoint) => {
                let count = session.get_active_tool_names().len();
                self.show_status(&format!(
                    "MCP: lending {count} tool(s) on {}\n\
                     The token is revoked when the session ends or on `/mcp lend off`.\n{}",
                    endpoint.url,
                    endpoint.as_config_entry(crate::config::APP_NAME)
                ));
            }
            Err(error) => self.show_error(&format!("MCP: {error}")),
        }
    }

    /// `/mcp import [user] <json>` — adds servers to a configuration file.
    /// The JSON is the shape `.mcp.json` already has, so what a server's
    /// documentation prints can be pasted straight in. Only the named scope's
    /// own file is touched: merging into the other one would move a server
    /// between "shared with the repository" and "mine alone" without saying so.
    async fn handle_mcp_import(&mut self, argument: &str) {
        use crate::core::mcp::{McpConfig, McpConfigScope, read_mcp_config, write_mcp_config};

        let (scope, json) = split_scope(argument);
        if json.is_empty() {
            self.show_error("Usage: /mcp import [user] <json>");
            return;
        }
        let incoming: McpConfig = match serde_json::from_str(json) {
            Ok(config) => config,
            Err(error) => {
                self.show_error(&format!(
                    "Not usable MCP configuration: {error}. \
                     Expected {{\"mcpServers\": {{ … }}}}."
                ));
                return;
            }
        };
        if incoming.mcp_servers.is_empty() {
            self.show_error("That configuration names no servers.");
            return;
        }

        let path = self.mcp_config_path(scope);
        let mut config = match read_mcp_config(&path, scope) {
            Ok(Some(file)) => file.config,
            Ok(None) => McpConfig::default(),
            Err(error) => {
                self.show_error(&format!("MCP: {error}"));
                return;
            }
        };
        let mut added = Vec::new();
        for (name, server) in incoming.mcp_servers {
            config.mcp_servers.insert(name.clone(), server);
            added.push(name.to_string());
        }
        if let Err(error) = write_mcp_config(&path, &config) {
            self.show_error(&format!("MCP: {error}"));
            return;
        }

        // Writing the project file changes its hash, so the trust decision that
        // covered the old contents no longer applies. Saying so is the
        // difference between a server that is simply missing and one the user
        // knows to accept.
        let note = match scope {
            McpConfigScope::Project => "\nThe project file changed, so it needs accepting again.",
            McpConfigScope::User => "",
        };
        let reloaded = self.reload_after_config_change().await;
        self.show_status(&format!(
            "MCP: added {} to {}{note}{reloaded}",
            added.join(", "),
            path.display()
        ));
    }

    /// `/mcp remove [user] <server>` — drops a server from a file.
    async fn handle_mcp_remove(&mut self, argument: &str) {
        use crate::core::mcp::{McpConfig, ServerName, read_mcp_config, write_mcp_config};

        let (scope, name) = split_scope(argument);
        if name.is_empty() {
            self.show_error("Usage: /mcp remove [user] <server>");
            return;
        }
        let path = self.mcp_config_path(scope);
        let mut config = match read_mcp_config(&path, scope) {
            Ok(Some(file)) => file.config,
            Ok(None) => McpConfig::default(),
            Err(error) => {
                self.show_error(&format!("MCP: {error}"));
                return;
            }
        };
        if config.mcp_servers.remove(&ServerName::from(name)).is_none() {
            self.show_error(&format!("MCP: `{name}` is not in {}", path.display()));
            return;
        }
        if let Err(error) = write_mcp_config(&path, &config) {
            self.show_error(&format!("MCP: {error}"));
            return;
        }
        let reloaded = self.reload_after_config_change().await;
        self.show_status(&format!(
            "MCP: removed `{name}` from {}{reloaded}",
            path.display()
        ));
    }

    /// `/mcp show <server>` — the configuration a server is running under.
    /// Reads the merged view rather than one file, because that is the one the
    /// session actually uses, and prints the error alongside when there is one:
    /// the configuration and the reason it did not work belong together.
    async fn handle_mcp_show(&mut self, name: &str) {
        use crate::core::mcp::{ServerName, read_all_mcp_configs};

        if name.is_empty() {
            self.show_error("Usage: /mcp show <server>");
            return;
        }
        let files = match read_all_mcp_configs(std::path::Path::new(&self.cwd())) {
            Ok(files) => files,
            Err(error) => {
                self.show_error(&format!("MCP: {error}"));
                return;
            }
        };
        let wanted = ServerName::from(name);
        // Project first, as everywhere else, so what is shown is what wins.
        let Some((file, server)) = files
            .iter()
            .find_map(|file| file.config.mcp_servers.get(&wanted).map(|s| (file, s)))
        else {
            self.show_error(&format!("MCP: no server named `{name}` is configured"));
            return;
        };
        let rendered = serde_json::to_string_pretty(server)
            .unwrap_or_else(|error| format!("(could not be rendered: {error})"));
        let mut lines = vec![
            format!("MCP `{name}` from {}:", file.path.display()),
            rendered,
        ];
        if let Some(entry) = self
            .session()
            .mcp()
            .entries()
            .await
            .into_iter()
            .find(|entry| entry.name == wanted)
        {
            lines.push(format!("status: {}", entry.status.as_str()));
            if let Some(error) = entry.error {
                lines.push(format!("error: {error}"));
            }
        }
        self.show_status(&lines.join("\n"));
    }

    /// The file a scope writes to.
    fn mcp_config_path(&self, scope: crate::core::mcp::McpConfigScope) -> std::path::PathBuf {
        crate::core::mcp::mcp_config_paths(std::path::Path::new(&self.cwd()))
            .into_iter()
            .find(|(_, candidate)| *candidate == scope)
            .map(|(path, _)| path)
            .unwrap_or_default()
    }

    /// Re-reads the configuration after it was edited, as a sentence to append.
    async fn reload_after_config_change(&mut self) -> String {
        match self.session().reload_mcp_servers().await {
            Ok(count) => format!("\nReloaded: {count} server(s) configured."),
            Err(error) => format!("\nThe reload failed: {error}"),
        }
    }

    /// toward an objective across turns until it completes it, reports itself
    /// blocked, or a budget runs out.
    /// A create that finds a goal already running reports the running one and
    /// leaves it alone; `replace` is how the user says otherwise. Overwriting
    /// silently would discard work in progress on a typo.
    fn handle_goal_command(&mut self, argument: Option<&str>) {
        use crate::core::goal::{GoalCommand, ThreadGoal, describe_goal, parse_goal_command};

        let state = self.session().goal_state();
        let mut state = state.lock().expect("goal state");
        match parse_goal_command(argument.unwrap_or_default()) {
            GoalCommand::Invalid(message) => self.show_error(&message),
            GoalCommand::Show => {
                let text = match state.goal.as_ref() {
                    Some(goal) => describe_goal(goal),
                    None => "No goal. Set one with /goal <objective>.".to_owned(),
                };
                self.show_status(&text);
            }
            GoalCommand::Pause => match state.goal.as_mut() {
                Some(goal) => {
                    goal.pause();
                    self.show_status(&format!("Goal paused: {}", goal.objective));
                }
                None => self.show_status("No goal to pause."),
            },
            GoalCommand::Resume => match state.goal.as_mut() {
                Some(goal) => {
                    goal.resume();
                    let text = describe_goal(goal);
                    state.completion.clear();
                    self.show_status(&text);
                }
                None => self.show_status("No goal to resume."),
            },
            GoalCommand::Clear => match state.goal.take() {
                Some(goal) => {
                    state.completion.clear();
                    self.show_status(&format!("Goal cleared: {}", goal.objective));
                }
                None => self.show_status("No goal to clear."),
            },
            GoalCommand::Create {
                objective,
                token_budget,
                turn_budget,
                replace,
                strict,
            } => {
                if let Some(existing) = state.goal.as_ref()
                    && !replace
                {
                    let text = format!(
                        "{}\nUse `/goal replace <objective>` to set a different one, or `/goal clear`.",
                        describe_goal(existing)
                    );
                    self.show_status(&text);
                    return;
                }
                let goal = ThreadGoal::new(objective, token_budget, turn_budget).strict(strict);
                let text = describe_goal(&goal);
                state.goal = Some(goal);
                state.completion.clear();
                self.show_status(&text);
            }
        }
    }

    /// reference's shell-output filter setting: `on`/`off` set the gate, an
    /// omitted argument toggles it. Off by default (user decision 2026-08-17);
    /// the gate is read at call time, so the next `bash` call already follows
    /// the new state.
    fn handle_bash_filter_command(&mut self, argument: Option<&str>) {
        let enabled = match argument {
            Some(argument) if argument.eq_ignore_ascii_case("on") => true,
            Some(argument) if argument.eq_ignore_ascii_case("off") => false,
            Some(other) => {
                self.show_error(&format!(
                    "Unknown /bash-filter argument: {other} (use on|off)"
                ));
                return;
            }
            None => !self.settings().get_bash_filter_enabled(),
        };
        self.settings().set_bash_filter_enabled(enabled);
        self.show_status(if enabled {
            "Bash filter: enabled"
        } else {
            "Bash filter: disabled"
        });
    }

    /// Runs the build on a blocking thread and streams progress into the
    /// status line via [`UiMessage::IndexBuildProgress`].
    fn start_index_build(&mut self) {
        use crate::core::tools::find_codebase::{IndexBuildPhase, rebuild_index_blocking};

        if self.index_build_running {
            self.show_status("Codebase index: build already running");
            return;
        }
        self.index_build_running = true;
        self.show_status("Codebase index: scanning…");
        let cwd = self.cwd();
        let progress_tx = self.ui_tx.clone();
        let done_tx = self.ui_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = rebuild_index_blocking(&cwd, &crate::config::get_agent_dir(), {
                let progress_tx = progress_tx.clone();
                move |progress| {
                    if progress.phase == IndexBuildPhase::Indexing {
                        let _ = progress_tx.send(UiMessage::IndexBuildProgress {
                            message: format!(
                                "Codebase index: {}/{} files",
                                progress.indexed_files, progress.total_files
                            ),
                            done: false,
                        });
                    }
                }
            });
            let message = match result {
                Ok(()) => "Codebase index: up to date".to_owned(),
                Err(error) => format!("Codebase index failed: {error}"),
            };
            let _ = done_tx.send(UiMessage::IndexBuildProgress {
                message,
                done: true,
            });
        });
    }

    async fn handle_model_command(&mut self, search_term: Option<&str>) {
        let Some(search_term) = search_term else {
            self.show_model_selector(None);
            return;
        };
        if let Some(model) = self.find_exact_model_match(search_term).await {
            self.apply_selected_model(model).await;
            return;
        }
        self.show_model_selector(Some(search_term));
    }

    async fn find_exact_model_match(&mut self, search_term: &str) -> Option<Model> {
        let session = self.session();
        let scoped = session.scoped_models();
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let cached_models: Vec<Model> = if scoped.is_empty() {
            model_runtime.get_available_snapshot()
        } else {
            scoped.iter().map(|scoped| scoped.model.clone()).collect()
        };
        let cached_match = find_exact_model_reference_match(search_term, &cached_models);
        if cached_match.is_some() || !scoped.is_empty() {
            return cached_match;
        }

        self.show_status("Refreshing model catalogs…");
        let signal = timeout_signal(15_000);
        let result = model_runtime
            .refresh(notagent_ai::models::ModelsRefreshOptions {
                signal: Some(signal.clone()),
                ..notagent_ai::models::ModelsRefreshOptions::default()
            })
            .await;
        if result.aborted {
            self.show_warning("Model refresh timed out; searching cached models.");
        } else if !result.errors.is_empty() {
            self.show_warning(&format!(
                "Could not refresh {}; searching cached models.",
                result.errors.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        find_exact_model_reference_match(search_term, &model_runtime.get_available_snapshot())
    }

    fn check_daxnuts_easter_egg(&mut self, model: &Model) {
        if model.provider == "opencode" && model.id.to_lowercase().contains("kimi-k2.5") {
            let mut chat = self.chat_container.borrow_mut();
            chat.add_child(component_ref(Spacer::new(1)));
            chat.add_child(component_ref(DaxnutsComponent::new()));
            drop(chat);
            self.ui.request_render();
        }
    }

    fn update_available_provider_count(&self) {
        let session = self.session();
        let scoped = session.scoped_models();
        let models: Vec<Model> = if scoped.is_empty() {
            self.runtime
                .services()
                .model_runtime
                .get_available_snapshot()
        } else {
            scoped.iter().map(|scoped| scoped.model.clone()).collect()
        };
        let providers: std::collections::BTreeSet<String> =
            models.into_iter().map(|model| model.provider).collect();
        self.footer_data
            .set_available_provider_count(providers.len() as u64);
    }

    /// Deviation (class 1): the callbacks that only write a setting do so
    /// directly — the settings manager is shared and needs no loop. The ones
    /// that also change mode state (images in the transcript, thinking blocks,
    /// padding, the TUI mode, the theme) post an effect into the loop, where
    /// `&mut self` is available.
    fn show_settings_selector(&mut self) {
        let id = self.selector_id + 1;
        let settings = self.settings();
        let session = self.session();
        fn effect(
            tx: &tokio::sync::mpsc::UnboundedSender<UiMessage>,
            id: u64,
            effect: SettingsEffect,
        ) {
            let _ = tx.send(UiMessage::Settings { id, effect });
        }

        let config = SettingsConfig {
            auto_compact: session.auto_compaction_enabled(),
            show_images: settings.get_show_images(),
            image_width_cells: settings.get_image_width_cells(),
            auto_resize_images: settings.get_image_auto_resize(),
            block_images: settings.get_block_images(),
            enable_skill_commands: settings.get_enable_skill_commands(),
            steering_mode: settings_queue_mode(session.steering_mode()),
            follow_up_mode: settings_queue_mode(session.follow_up_mode()),
            transport: parse_transport(&settings.get_transport()),
            http_idle_timeout_ms: settings.get_http_idle_timeout_ms().unwrap_or_default(),
            current_theme: settings
                .get_theme_setting()
                .unwrap_or_else(|| "dark".to_owned()),
            terminal_theme: self.theme_controller.get_terminal_theme(),
            available_themes: get_available_themes(),
            hide_thinking_block: self.hide_thinking_block,
            mermaid_rendering_mode: settings.get_mermaid_rendering_mode(),
            show_cache_miss_notices: settings.get_show_cache_miss_notices(),
            enable_install_telemetry: settings.get_enable_install_telemetry(),
            double_escape_action: settings.get_double_escape_action(),
            tree_filter_mode: settings.get_tree_filter_mode(),
            show_hardware_cursor: settings.get_show_hardware_cursor(),
            editor_padding_x: settings.get_editor_padding_x(),
            output_pad: settings.get_output_pad(),
            autocomplete_max_visible: settings.get_autocomplete_max_visible(),
            quiet_startup: settings.get_quiet_startup(),
            default_project_trust: settings.get_default_project_trust(),
            clear_on_shrink: settings.get_clear_on_shrink(),
            show_terminal_progress: settings.get_show_terminal_progress(),
            show_workspace_in_footer: settings.get_show_workspace_in_footer(),
            tiered_thinking: settings.get_tiered_thinking(),
            tui_mode: self.cell.mode(),
            fullscreen_exit_output: settings.get_fullscreen_exit_output(),
            fullscreen_scrollbar: scroll_view_scrollbar(settings.get_fullscreen_scrollbar()),
            warnings: settings.get_warnings(),
            atomic_leases: settings.get_atomic_leases_enabled(),
            bash_filter: settings.get_bash_filter_enabled(),
        };

        let callbacks = SettingsCallbacks {
            on_auto_compact_change: {
                let session = Arc::clone(&session);
                let tx = self.ui_tx.clone();
                Box::new(move |enabled| {
                    session.set_auto_compaction_enabled(enabled);
                    effect(&tx, id, SettingsEffect::AutoCompact(enabled));
                })
            },
            // The gate is read at call time, so the next mutating tool call
            // already follows the new state — no session restart, and no
            // effect to send.
            on_atomic_leases_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |enabled| settings.set_atomic_leases_enabled(enabled))
            },
            on_bash_filter_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |enabled| settings.set_bash_filter_enabled(enabled))
            },
            on_show_images_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |enabled| {
                    settings.set_show_images(enabled);
                    effect(&tx, id, SettingsEffect::ShowImages(enabled));
                })
            },
            on_image_width_cells_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |width| {
                    settings.set_image_width_cells(width as f64);
                    effect(&tx, id, SettingsEffect::ImageWidthCells(width));
                })
            },
            on_auto_resize_images_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |enabled| settings.set_image_auto_resize(enabled))
            },
            on_block_images_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |blocked| settings.set_block_images(blocked))
            },
            on_enable_skill_commands_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |enabled| {
                    settings.set_enable_skill_commands(enabled);
                    effect(&tx, id, SettingsEffect::RebuildAutocomplete);
                })
            },
            on_steering_mode_change: {
                let session = Arc::clone(&session);
                Box::new(move |mode| session.set_steering_mode(agent_queue_mode(mode)))
            },
            on_follow_up_mode_change: {
                let session = Arc::clone(&session);
                Box::new(move |mode| session.set_follow_up_mode(agent_queue_mode(mode)))
            },
            on_transport_change: {
                let settings = Arc::clone(&settings);
                let session = Arc::clone(&session);
                Box::new(move |transport| {
                    settings.set_transport(transport_wire_name(transport));
                    // `session.agent.transport = transport` — the next run picks
                    session
                        .agent()
                        .update_options(|options| options.transport = Some(transport));
                })
            },
            on_http_idle_timeout_ms_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |timeout_ms| {
                    let _ = settings.set_http_idle_timeout_ms(timeout_ms as f64);
                    effect(&tx, id, SettingsEffect::HttpIdleTimeout(timeout_ms));
                })
            },
            on_theme_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |theme_setting| {
                    settings.set_theme(theme_setting);
                    effect(&tx, id, SettingsEffect::ThemeApplied);
                })
            },
            on_theme_preview: Some({
                let controller = self.theme_controller.clone();
                Box::new(move |theme_name: &str| controller.preview(theme_name))
            }),
            on_hide_thinking_block_change: {
                let tx = self.ui_tx.clone();
                Box::new(move |hidden| effect(&tx, id, SettingsEffect::HideThinkingBlock(hidden)))
            },
            on_mermaid_rendering_mode_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |mode| {
                    settings.set_mermaid_rendering_mode(mode);
                    effect(&tx, id, SettingsEffect::InvalidateChat);
                })
            },
            on_show_cache_miss_notices_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |shown| {
                    settings.set_show_cache_miss_notices(shown);
                    effect(&tx, id, SettingsEffect::RebuildChat);
                })
            },
            on_enable_install_telemetry_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |enabled| settings.set_enable_install_telemetry(enabled))
            },
            on_double_escape_action_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |action| settings.set_double_escape_action(action))
            },
            on_tree_filter_mode_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |mode| settings.set_tree_filter_mode(mode))
            },
            on_show_hardware_cursor_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |enabled| {
                    settings.set_show_hardware_cursor(enabled);
                    effect(&tx, id, SettingsEffect::ShowHardwareCursor(enabled));
                })
            },
            on_editor_padding_x_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |padding| {
                    settings.set_editor_padding_x(padding as f64);
                    effect(&tx, id, SettingsEffect::EditorPaddingX(padding));
                })
            },
            on_output_pad_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |padding| {
                    settings.set_output_pad(padding);
                    effect(&tx, id, SettingsEffect::OutputPad(padding));
                })
            },
            on_autocomplete_max_visible_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |max_visible| {
                    settings.set_autocomplete_max_visible(max_visible as f64);
                    effect(&tx, id, SettingsEffect::AutocompleteMaxVisible(max_visible));
                })
            },
            on_quiet_startup_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |enabled| settings.set_quiet_startup(enabled))
            },
            on_default_project_trust_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |trust| settings.set_default_project_trust(trust))
            },
            on_clear_on_shrink_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |enabled| {
                    settings.set_clear_on_shrink(enabled);
                    effect(&tx, id, SettingsEffect::ClearOnShrink(enabled));
                })
            },
            on_show_terminal_progress_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |enabled| settings.set_show_terminal_progress(enabled))
            },
            on_show_workspace_in_footer_change: {
                let settings = Arc::clone(&settings);
                let tx = self.ui_tx.clone();
                Box::new(move |enabled| {
                    settings.set_show_workspace_in_footer(enabled);
                    effect(&tx, id, SettingsEffect::WorkspaceInFooter(enabled));
                })
            },
            // Read at the start of every step, so the next one already follows
            // the new setting — no session restart, and no effect to send.
            on_tiered_thinking_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |enabled| settings.set_tiered_thinking(enabled))
            },
            on_tui_mode_change: {
                let tx = self.ui_tx.clone();
                Box::new(move |mode| effect(&tx, id, SettingsEffect::TuiMode(mode)))
            },
            on_fullscreen_exit_output_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |output| settings.set_fullscreen_exit_output(output))
            },
            on_fullscreen_scrollbar_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |mode| settings.set_fullscreen_scrollbar(settings_scrollbar(mode)))
            },
            on_warnings_change: {
                let settings = Arc::clone(&settings);
                Box::new(move |warnings| settings.set_warnings(&warnings))
            },
            on_cancel: {
                let tx = self.ui_tx.clone();
                Box::new(move || {
                    let _ = tx.send(UiMessage::SelectorCancelled { id });
                })
            },
        };

        let selector = Rc::new(RefCell::new(SettingsSelectorComponent::new(
            config, callbacks,
        )));
        let focus = selector.borrow().settings_list();
        self.show_selector(
            Rc::clone(&selector) as ComponentRef,
            focus as ComponentRef,
            None,
        );
    }

    /// The half of a settings change that needs the mode.
    async fn apply_settings_effect(&mut self, effect: SettingsEffect) {
        match effect {
            SettingsEffect::AutoCompact(_) => {
                // The footer no longer shows an "(auto)" marker (user
                // decision 2026-08-20); the setting itself lives on the
                // session and needs no mode-side effect anymore.
            }
            SettingsEffect::ShowImages(enabled) => {
                self.for_each_tool_row(|row| row.set_show_images(enabled));
            }
            SettingsEffect::ImageWidthCells(width) => {
                self.for_each_tool_row(|row| row.set_image_width_cells(width as usize));
            }
            SettingsEffect::RebuildAutocomplete => self.setup_autocomplete_provider(),
            SettingsEffect::HttpIdleTimeout(timeout_ms) => {
                self.show_status(&format!(
                    "HTTP idle timeout: {}",
                    format_http_idle_timeout_ms(timeout_ms)
                ));
            }
            SettingsEffect::WorkspaceInFooter(show) => {
                self.footer.borrow_mut().set_show_workspace(show);
                self.ui.request_render();
            }
            SettingsEffect::ThemeApplied => {
                self.theme_controller.apply_from_settings().await;
            }
            SettingsEffect::HideThinkingBlock(hidden) => {
                self.hide_thinking_block = hidden;
                self.settings().set_hide_thinking_block(hidden);
                self.rebuild_chat_from_messages();
            }
            SettingsEffect::InvalidateChat => {
                self.chat_container.borrow_mut().invalidate();
                self.ui.request_render();
            }
            SettingsEffect::RebuildChat => self.rebuild_chat_from_messages(),
            SettingsEffect::ShowHardwareCursor(enabled) => {
                self.ui.set_show_hardware_cursor(enabled);
            }
            SettingsEffect::EditorPaddingX(padding) => {
                self.editor.borrow_mut().set_padding_x(padding as usize);
            }
            SettingsEffect::OutputPad(padding) => {
                self.output_pad = padding as usize;
                self.rebuild_chat_from_messages();
            }
            SettingsEffect::AutocompleteMaxVisible(max_visible) => {
                self.editor
                    .borrow_mut()
                    .editor_mut()
                    .set_autocomplete_max_visible(max_visible as usize);
            }
            SettingsEffect::ClearOnShrink(enabled) => {
                self.ui.set_clear_on_shrink(enabled);
                if !enabled && self.active_status_indicator.is_none() {
                    self.status_container.borrow_mut().clear();
                }
            }
            SettingsEffect::TuiMode(mode) => self.switch_tui_mode(mode),
        }
        self.ui.request_render();
    }

    /// Runs `body` for every tool row in the transcript.
    /// instead, because a `dyn Component` cannot be downcast (deviation
    /// class 1, same set of rows).
    fn for_each_tool_row(&self, mut body: impl FnMut(&mut ToolExecutionComponent)) {
        for component in self.chat_tool_rows.iter() {
            body(&mut component.borrow_mut());
        }
    }

    fn show_user_message_selector(&mut self) {
        let messages = self.session().get_user_messages_for_forking();
        if messages.is_empty() {
            self.show_status("No messages to fork from");
            return;
        }
        let initial_selected_id = messages.last().map(|(entry_id, _)| entry_id.clone());
        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let selector = Rc::new(RefCell::new(UserMessageSelectorComponent::new(
            messages
                .into_iter()
                .map(|(entry_id, text)| UserMessageItem {
                    id: entry_id,
                    text,
                    timestamp: None,
                })
                .collect(),
            Box::new(move |entry_id| {
                let _ = select_tx.send(UiMessage::ForkAt {
                    id,
                    entry_id: entry_id.to_owned(),
                    position: ForkPosition::Before,
                });
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
            initial_selected_id.as_deref(),
        )));
        let focus = selector.borrow().get_message_list();
        self.show_selector(Rc::clone(&selector) as ComponentRef, focus, None);
    }

    /// The `onSelect` half of the fork selector and of `/clone`.
    async fn fork_session(&mut self, entry_id: &str, position: ForkPosition) {
        match self.runtime.fork(entry_id, position).await {
            Ok(selected_text) => {
                self.rebind_current_session();
                self.editor
                    .borrow_mut()
                    .editor_mut()
                    .set_text(selected_text.as_deref().unwrap_or(""));
                self.show_status(if position == ForkPosition::At {
                    "Cloned to new session"
                } else {
                    "Forked to new session"
                });
            }
            Err(message) => self.show_error(&message),
        }
    }

    async fn handle_clone_command(&mut self) {
        let leaf_id = self
            .session()
            .with_session_manager(|manager| manager.get_leaf_id().map(str::to_owned));
        let Some(leaf_id) = leaf_id else {
            self.show_status("Nothing to clone yet");
            return;
        };
        self.fork_session(&leaf_id, ForkPosition::At).await;
    }

    /// `maybeSaveImplicitProjectTrustAfterReload()`
    /// A session that started in a folder without trust-requiring resources was
    /// trusted implicitly. If the reload brought such resources in, that
    /// implicit decision is written down once, so the next start does not ask.
    fn maybe_save_implicit_project_trust_after_reload(&mut self) -> bool {
        let cwd = self.cwd();
        if self.options.auto_trust_on_reload_cwd.as_deref() != Some(cwd.as_str()) {
            return false;
        }
        if !self.settings().is_project_trusted() || !has_trust_requiring_project_resources(&cwd) {
            return false;
        }

        let trust_store = ProjectTrustStore::new(&self.runtime.services().agent_dir);
        match trust_store.get(&cwd) {
            Ok(Some(_)) => {
                self.options.auto_trust_on_reload_cwd = None;
                false
            }
            Ok(None) => match trust_store.set(&cwd, Some(true)) {
                Ok(()) => {
                    self.options.auto_trust_on_reload_cwd = None;
                    true
                }
                Err(error) => {
                    self.show_warning(&format!(
                        "Could not save project trust after reload: {error}"
                    ));
                    false
                }
            },
            Err(error) => {
                self.show_warning(&format!(
                    "Could not save project trust after reload: {error}"
                ));
                false
            }
        }
    }

    fn show_trust_selector(&mut self) {
        let cwd = self.cwd();
        let trust_store = ProjectTrustStore::new(&self.runtime.services().agent_dir);
        let saved_decision = trust_store.get_entry(&cwd).ok().flatten();
        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let selector = component_ref(TrustSelectorComponent::new(TrustSelectorOptions {
            cwd: cwd.clone(),
            saved_decision,
            project_trusted: self.settings().is_project_trusted(),
            on_select: Box::new(move |selection| {
                let _ = select_tx.send(UiMessage::TrustDecision { id, selection });
            }),
            on_cancel: Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
        }));
        self.show_selector(Rc::clone(&selector), selector, None);
    }

    /// The `onSelect` half of the trust selector.
    fn apply_trust_decision(&mut self, selection: TrustSelection) {
        let trust_store = ProjectTrustStore::new(&self.runtime.services().agent_dir);
        let _ = trust_store.set_many(&selection.updates);
        self.show_status(&format!(
            "Saved trust decision: {}. Restart notagent for this to take effect.",
            if selection.trusted {
                "trusted"
            } else {
                "untrusted"
            }
        ));
    }

    /// The tool call stays open until the dialog answers; dismissing it denies,
    /// so a call can never proceed because a prompt was closed.
    fn show_approval_selector(
        &mut self,
        request: ApprovalRequest,
        answer: tokio::sync::oneshot::Sender<ApprovalAnswer>,
    ) {
        let id = self.selector_id + 1;
        let tx = self.ui_tx.clone();
        let answer = Rc::new(RefCell::new(Some(answer)));
        let selector = Rc::new(RefCell::new(ApprovalSelectorComponent::new(
            &request,
            Box::new(move |given| {
                if let Some(answer) = answer.borrow_mut().take() {
                    let _ = answer.send(given);
                }
                let _ = tx.send(UiMessage::SelectorCancelled { id });
            }),
        )));
        let focus = Rc::clone(selector.borrow().get_select_list()) as ComponentRef;
        self.show_selector(Rc::clone(&selector) as ComponentRef, focus, None);
    }

    /// The extension commands are gone with the extension system (class 2);
    /// what remains are the built-in commands with their two argument
    /// completions, the prompt templates and the skill commands.
    fn create_base_autocomplete_provider(&mut self) -> CombinedAutocompleteProvider {
        let session = self.session();
        let mut commands: Vec<SlashCommand> = BUILTIN_SLASH_COMMANDS
            .iter()
            .map(|command| SlashCommand {
                name: command.name.to_owned(),
                description: Some(command.description.to_owned()),
                argument_hint: command.argument_hint.map(str::to_owned),
                get_argument_completions: None,
            })
            .collect();

        {
            let session = Arc::clone(&session);
            let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
            let model_completions: ArgumentCompletions = Rc::new(move |prefix: &str| {
                let scoped = session.scoped_models();
                let models: Vec<Model> = if scoped.is_empty() {
                    model_runtime.get_available_snapshot()
                } else {
                    scoped.iter().map(|scoped| scoped.model.clone()).collect()
                };
                let prefix = prefix.to_owned();
                Box::pin(async move {
                    if models.is_empty() {
                        return None;
                    }
                    let filtered = fuzzy_filter(&models, &prefix, |model: &Model| {
                        get_model_search_text(&ModelSearchItem::new(
                            model.id.clone(),
                            model.provider.clone(),
                            Some(model.name.clone()),
                        ))
                    });
                    if filtered.is_empty() {
                        return None;
                    }
                    Some(
                        filtered
                            .into_iter()
                            .map(|model| AutocompleteItem {
                                value: format!("{}/{}", model.provider, model.id),
                                label: model.id.clone(),
                                description: Some(model.provider.clone()),
                            })
                            .collect(),
                    )
                }) as Pin<Box<dyn Future<Output = Option<Vec<AutocompleteItem>>>>>
            });
            // same list of available models as `/model`.
            for name in ["model", "subagent-model"] {
                if let Some(command) = commands.iter_mut().find(|command| command.name == name) {
                    command.get_argument_completions = Some(Rc::clone(&model_completions));
                }
            }
        }

        // Prompt templates.
        for template in session.prompt_templates() {
            commands.push(SlashCommand {
                name: template.name.clone(),
                description: self.prefix_autocomplete_description(
                    Some(template.description.clone()),
                    &template.source_info,
                ),
                argument_hint: template.argument_hint.clone(),
                get_argument_completions: None,
            });
        }

        // the prompt templates and the skill commands; the source tag its
        // description carried (`[t]`, from the synthetic source info of an
        // inline extension) goes with the extension system (class 2).
        commands.push(SlashCommand {
            name: "llama".to_owned(),
            description: Some(LLAMA_COMMAND_DESCRIPTION.to_owned()),
            argument_hint: None,
            get_argument_completions: None,
        });

        // Skill commands, when they are enabled.
        self.skill_commands.clear();
        if self.settings().get_enable_skill_commands() {
            for skill in session.resource_loader().get_skills().0 {
                let command_name = format!("skill:{}", skill.name);
                self.skill_commands
                    .push((command_name.clone(), skill.file_path.clone()));
                commands.push(SlashCommand {
                    name: command_name,
                    description: self.prefix_autocomplete_description(
                        Some(skill.description.clone()),
                        &skill.source_info,
                    ),
                    argument_hint: None,
                    get_argument_completions: None,
                });
            }
        }

        CombinedAutocompleteProvider::new(
            commands.into_iter().map(CommandEntry::Command).collect(),
            self.cwd(),
            self.fd_path.clone(),
        )
    }

    /// `prefixAutocompleteDescription(description, sourceInfo)`
    fn prefix_autocomplete_description(
        &self,
        description: Option<String>,
        source_info: &SourceInfo,
    ) -> Option<String> {
        let tag = self.autocomplete_source_tag(source_info)?;
        Some(match description {
            Some(description) if !description.is_empty() => format!("[{tag}] {description}"),
            _ => format!("[{tag}]"),
        })
    }

    fn autocomplete_source_tag(&self, source_info: &SourceInfo) -> Option<String> {
        let scope_prefix = match source_info.scope {
            SourceScope::User => "u",
            SourceScope::Project => "p",
            _ => "t",
        };
        let source = source_info.source.trim();
        if source == "auto" || source == "local" || source == "cli" {
            return Some(scope_prefix.to_owned());
        }
        Some(format!("{scope_prefix}:{source}"))
    }

    fn setup_autocomplete_provider(&mut self) {
        let provider = self.create_base_autocomplete_provider();
        self.editor
            .borrow_mut()
            .editor_mut()
            .set_autocomplete_provider(Rc::new(provider));
    }

    /// The component asks for its session lists instead of loading them itself
    /// main loop runs the loads — the same shape `cli/session_picker.rs` uses.
    fn show_session_selector(&mut self) {
        let id = self.selector_id + 1;
        let session = self.session();
        let current_file =
            session.with_session_manager(|manager| manager.get_session_file().map(str::to_owned));
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let exit_tx = self.ui_tx.clone();
        let core = self.ui.clone();
        let selector = Rc::new(RefCell::new(SessionSelectorComponent::new(
            Box::new(move |session_path: &str| {
                let _ = select_tx.send(UiMessage::ResumeSession {
                    id,
                    path: session_path.to_owned(),
                });
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
            Box::new(move || {
                let _ = exit_tx.send(UiMessage::Action(AppAction::Exit));
            }),
            Rc::new(move || core.request_render()),
            SessionSelectorOptions {
                rename_session: Some(Box::new(|path: &str, next_name: &str| {
                    let next = next_name.trim();
                    if next.is_empty() {
                        return;
                    }
                    if let Ok(mut manager) = SessionManager::open(path, None, None) {
                        let _ = manager.append_session_info(next);
                    }
                })),
                show_rename_hint: Some(true),
                keybindings: Some(Rc::clone(&self.keybindings)),
            },
            current_file.as_deref(),
        )));
        let focus = Rc::clone(selector.borrow().get_session_list()) as ComponentRef;
        // The loop drives this selector's loads while it is open.
        self.session_selector = Some(Rc::clone(&selector));
        self.show_selector(Rc::clone(&selector) as ComponentRef, focus, None);
    }

    async fn handle_resume_session(&mut self, session_path: &str) {
        self.clear_status_indicator(None);
        match self.runtime.switch_session(session_path, None).await {
            Ok(()) => {
                self.rebind_current_session();
                self.show_status("Resumed session");
            }
            Err(SessionOpenError::MissingCwd(error)) => {
                let Some(selected_cwd) = self.prompt_for_missing_session_cwd(&error).await else {
                    self.show_status("Resume cancelled");
                    return;
                };
                match self
                    .runtime
                    .switch_session(session_path, Some(&selected_cwd))
                    .await
                {
                    Ok(()) => {
                        self.rebind_current_session();
                        self.show_status("Resumed session in current cwd");
                    }
                    Err(error) => {
                        self.show_error(&format!("Failed to resume session: {error}"));
                    }
                }
            }
            Err(error) => self.show_error(&format!("Failed to resume session: {error}")),
        }
    }

    async fn prompt_for_missing_session_cwd(
        &mut self,
        error: &MissingSessionCwdError,
    ) -> Option<String> {
        let confirmed = self
            .confirm(
                "Session cwd not found",
                &format_missing_session_cwd_prompt(&error.0),
            )
            .await;
        confirmed.then(|| error.0.fallback_cwd.clone())
    }

    fn show_tree_selector(&mut self, initial_selected_id: Option<String>) {
        let session = self.session();
        let (tree, real_leaf_id) = session.with_session_manager(|manager| {
            (manager.get_tree(), manager.get_leaf_id().map(str::to_owned))
        });
        if tree.is_empty() {
            self.show_status("No entries in session");
            return;
        }
        let initial_filter_mode = self.settings().get_tree_filter_mode();
        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let label_tx = self.ui_tx.clone();
        let copy_tx = self.ui_tx.clone();
        let rows = self.ui.rows();
        let mut selector = TreeSelectorComponent::new(
            &tree,
            real_leaf_id.as_deref(),
            rows,
            Box::new(move |entry_id: &str| {
                let _ = select_tx.send(UiMessage::TreeNavigate {
                    id,
                    entry_id: entry_id.to_owned(),
                });
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
            TreeSelectorOptions {
                on_label_change: Some(Box::new(move |entry_id: &str, label: Option<&str>| {
                    let _ = label_tx.send(UiMessage::TreeLabel {
                        entry_id: entry_id.to_owned(),
                        label: label.map(str::to_owned),
                    });
                })),
                initial_selected_id,
                initial_filter_mode: Some(tree_filter_mode(initial_filter_mode)),
            },
        );
        selector.on_copy = Some(Box::new(move |text: Option<&str>| {
            let _ = copy_tx.send(UiMessage::TreeCopy {
                text: text.map(str::to_owned),
            });
        }));
        let selector = component_ref(selector);
        self.show_selector(Rc::clone(&selector), selector, None);
    }

    /// The `onSelect` half of the tree selector.
    async fn navigate_tree(&mut self, entry_id: &str) {
        if self
            .session()
            .with_session_manager(|manager| manager.get_leaf_id().map(str::to_owned))
            .as_deref()
            == Some(entry_id)
        {
            self.show_status("Already at this point");
            return;
        }

        let mut wants_summary = false;
        let mut custom_instructions: Option<String> = None;
        if !self.settings().get_branch_summary_skip_prompt() {
            loop {
                let choice = self
                    .ask(
                        "Summarize branch?",
                        vec![
                            "No summary".to_owned(),
                            "Summarize".to_owned(),
                            "Summarize with custom prompt".to_owned(),
                        ],
                    )
                    .await;
                let Some(answer) = choice else {
                    self.show_tree_selector(Some(entry_id.to_owned()));
                    return;
                };
                wants_summary = answer != "No summary";
                if answer == "Summarize with custom prompt" {
                    custom_instructions = self.ask_text("Custom summarization instructions").await;
                    if custom_instructions.is_none() {
                        // Cancelled: back to the summary question.
                        continue;
                    }
                }
                break;
            }
        }

        // The user committed to navigating: stop the active response first.
        if self.session().is_streaming() {
            self.restore_queued_messages_to_editor(false);
            self.session().abort().await;
        }

        let mut showing_summary_indicator = false;
        if wants_summary {
            self.escape_target = EscapeTarget::BranchSummary;
            self.chat_container
                .borrow_mut()
                .add_child(component_ref(Spacer::new(1)));
            self.show_status_indicator(StatusIndicator::branch_summary());
            showing_summary_indicator = true;
            self.ui.request_render();
        }

        let result = self
            .session()
            .navigate_tree(
                entry_id,
                NavigateTreeOptions {
                    summarize: wants_summary,
                    custom_instructions,
                    ..NavigateTreeOptions::default()
                },
            )
            .await;
        match result {
            Ok(result) if result.aborted => {
                self.show_status("Branch summarization cancelled");
                self.show_tree_selector(Some(entry_id.to_owned()));
            }
            Ok(result) if result.cancelled => self.show_status("Navigation cancelled"),
            Ok(result) => {
                self.chat_container.borrow_mut().clear();
                self.render_initial_messages();
                if let Some(editor_text) = result.editor_text
                    && self.editor.borrow().editor().get_text().trim().is_empty()
                {
                    self.editor.borrow_mut().editor_mut().set_text(&editor_text);
                }
                self.show_status("Navigated to selected point");
                self.flush_compaction_queue(false).await;
            }
            Err(message) => self.show_error(&message),
        }
        if showing_summary_indicator {
            self.clear_status_indicator(Some(StatusIndicatorKind::BranchSummary));
        }
        if self.escape_target == EscapeTarget::BranchSummary {
            self.escape_target = EscapeTarget::Default;
        }
    }

    /// `showExtensionSelector(title, options)` for the answers the tree flow
    /// needs — the list dialog of [`Self::confirm`] with free labels.
    async fn ask(&mut self, title: &str, options: Vec<String>) -> Option<String> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Option<String>>();
        let done_tx = Rc::new(RefCell::new(Some(done_tx)));
        let select_tx = Rc::clone(&done_tx);
        let cancel_tx = Rc::clone(&done_tx);
        let selector = component_ref(ListSelectorComponent::new(
            title,
            options,
            Box::new(move |option| {
                if let Some(sender) = select_tx.borrow_mut().take() {
                    let _ = sender.send(Some(option));
                }
            }),
            Box::new(move || {
                if let Some(sender) = cancel_tx.borrow_mut().take() {
                    let _ = sender.send(None);
                }
            }),
            None,
        ));
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(Rc::clone(&selector));
        }
        self.ui.set_focus(Some(Rc::clone(&selector)));
        self.ui.request_render();

        let answer = done_rx.await.unwrap_or(None);

        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.ui.request_render();
        answer
    }

    /// — the free-text dialog of the tree's third answer, under the neutral
    async fn ask_text(&mut self, title: &str) -> Option<String> {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Option<String>>();
        let done_tx = Rc::new(RefCell::new(Some(done_tx)));
        let submit_tx = Rc::clone(&done_tx);
        let cancel_tx = Rc::clone(&done_tx);
        let cell = self.cell.clone();
        let dialog = component_ref(TextInputDialogComponent::new(
            self.ui.clone(),
            Rc::clone(&self.keybindings),
            title,
            Box::new(move |value| {
                if let Some(sender) = submit_tx.borrow_mut().take() {
                    let _ = sender.send(Some(value));
                }
            }),
            Box::new(move || {
                if let Some(sender) = cancel_tx.borrow_mut().take() {
                    let _ = sender.send(None);
                }
            }),
            Some(TextInputDialogOptions {
                external_editor_command: Some(self.settings().get_external_editor_command()),
                // `tui.stop()`/`start()` hang off the input pump, which a
                // component cannot reach (A-27).
                on_external_editor: Some(Box::new(move |command: &str, content: &str| {
                    cell.stop(TuiStopOptions::default());
                    let result = edit_in_external_editor(&ExternalEditorOptions {
                        command: command.to_owned(),
                        content: content.to_owned(),
                    });
                    cell.start();
                    cell.request_render(true);
                    match result {
                        ExternalEditorResult::Complete(text) => Some(text),
                        _ => None,
                    }
                })),
                ..TextInputDialogOptions::default()
            }),
        ));
        self.dispose_active_selector();
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(Rc::clone(&dialog));
        }
        self.ui.set_focus(Some(Rc::clone(&dialog)));
        self.ui.request_render();

        let answer = done_rx.await.unwrap_or(None);

        // `hideExtensionEditor()`.
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.ui.request_render();
        answer
    }

    fn show_models_selector(&mut self) {
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let available_models = model_runtime.get_available_snapshot();
        let session = self.session();
        let session_scoped = session.scoped_models();
        let configured_patterns = self.settings().get_enabled_models();

        let current_enabled_ids = if session_scoped.is_empty() {
            configured_enabled_ids(configured_patterns.as_deref(), &available_models)
        } else {
            Some(
                session_scoped
                    .iter()
                    .map(|scoped| format!("{}/{}", scoped.model.provider, scoped.model.id))
                    .collect(),
            )
        };

        let id = self.selector_id + 1;
        let change_tx = self.ui_tx.clone();
        let persist_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let selector = Rc::new(RefCell::new(ScopedModelsSelectorComponent::new(
            ModelsConfig {
                all_models: available_models.clone(),
                enabled_model_ids: current_enabled_ids.clone(),
                refresh_status: Some("Refreshing model catalogs…".to_owned()),
            },
            ModelsCallbacks {
                on_change: Box::new(move |enabled_ids| {
                    let _ = change_tx.send(UiMessage::ScopedModelsChanged {
                        enabled_ids,
                        persist: false,
                    });
                }),
                on_persist: Box::new(move |enabled_ids| {
                    let _ = persist_tx.send(UiMessage::ScopedModelsChanged {
                        enabled_ids,
                        persist: true,
                    });
                }),
                on_cancel: Box::new(move || {
                    let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
                }),
            },
        )));
        self.scoped_models_selector = Some(Rc::clone(&selector));

        // `void modelRuntime.refresh(...)` — the loop drives it and the answer
        // comes back as a message.
        let signal = timeout_signal(15_000);
        self.side_futures.push(Box::pin(async move {
            let result = model_runtime
                .refresh(notagent_ai::models::ModelsRefreshOptions {
                    signal: Some(signal),
                    ..notagent_ai::models::ModelsRefreshOptions::default()
                })
                .await;
            UiMessage::ScopedModelsRefreshed {
                aborted: result.aborted,
                errors: result.errors.keys().cloned().collect(),
            }
        }));

        self.show_selector(
            Rc::clone(&selector) as ComponentRef,
            Rc::clone(&selector) as ComponentRef,
            None,
        );
    }

    /// `updateSessionModels(enabledIds)` of the models selector.
    fn apply_scoped_models(&mut self, enabled_ids: Option<Vec<String>>, persist: bool) {
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let available_models = model_runtime.get_available_snapshot();
        let available_ids: Vec<String> = available_models
            .iter()
            .map(|model| format!("{}/{}", model.provider, model.id))
            .collect();

        if persist {
            let all_enabled = enabled_ids.as_ref().is_some_and(|ids| {
                ids.len() == available_models.len()
                    && ids.iter().all(|id| available_ids.contains(id))
            });
            let patterns = match (&enabled_ids, all_enabled) {
                (None, _) | (_, true) => None,
                (Some(ids), false) => Some(ids.clone()),
            };
            self.settings().set_enabled_models(patterns.as_deref());
            self.show_status("Model selection saved to settings");
            return;
        }

        let has_enabled_available = enabled_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| available_ids.contains(id)));
        let all_available_enabled = enabled_ids
            .as_ref()
            .is_some_and(|ids| available_ids.iter().all(|id| ids.contains(id)));
        match &enabled_ids {
            Some(ids) if has_enabled_available && !all_available_enabled => {
                let resolved = resolve_model_scope_from_models(ids, &available_models);
                self.session().set_scoped_models(
                    resolved
                        .scoped_models
                        .into_iter()
                        .map(|scoped| crate::core::agent_session::ScopedModel {
                            model: scoped.model,
                            thinking_level: scoped.thinking_level,
                        })
                        .collect(),
                );
            }
            _ => self.session().set_scoped_models(Vec::new()),
        }
        self.update_available_provider_count();
        self.ui.request_render();
    }

    /// The tail of the models selector's catalog refresh.
    fn apply_scoped_models_refresh(&mut self, aborted: bool, errors: Vec<String>) {
        let Some(selector) = self.scoped_models_selector.clone() else {
            return;
        };
        let available_models = self
            .runtime
            .services()
            .model_runtime
            .get_available_snapshot();
        selector.borrow_mut().update_models(&available_models, None);
        if aborted {
            selector.borrow_mut().set_refresh_status(
                "Model refresh timed out; showing cached models.",
                RefreshStatusKind::Warning,
            );
        } else if !errors.is_empty() {
            selector.borrow_mut().set_refresh_status(
                &format!(
                    "Could not refresh {}; showing cached models.",
                    errors.join(", ")
                ),
                RefreshStatusKind::Warning,
            );
        } else {
            selector
                .borrow_mut()
                .set_refresh_status("Model catalogs refreshed.", RefreshStatusKind::Success);
        }
        self.ui.request_render();
    }

    /// The polling, the output reads and the stop calls are driven by the main
    fn show_tasks_browser(&mut self) {
        let Some(manager) = self.session().task_manager() else {
            self.show_status("No task manager in this session");
            return;
        };
        let id = self.selector_id + 1;
        let selected_task_id = manager
            .list(false, None)
            .into_iter()
            .find(|info| info.status() == TaskStatus::Running)
            .map(|info| info.task_id().to_owned());
        let state = TasksBrowserState {
            filter: TasksFilter::All,
            selected_task_id: selected_task_id.clone(),
            output: None,
            output_loading: false,
            notice: None,
            notice_until: None,
            request: 0,
        };
        let rows = self.ui.rows().max(3) - 2;
        let browser = Rc::new(RefCell::new(TasksBrowserComponent::new(
            self.tasks_browser_props(&manager, &state, id),
            rows,
        )));
        self.tasks_browser = Some((Rc::clone(&browser), state));
        if let Some(task_id) = selected_task_id {
            self.load_task_output(&task_id, id);
        }
        self.show_selector(
            Rc::clone(&browser) as ComponentRef,
            Rc::clone(&browser) as ComponentRef,
            None,
        );
    }

    /// `push()` of the browser: hand it the current tasks and state.
    fn tasks_browser_props(
        &self,
        manager: &TaskManager,
        state: &TasksBrowserState,
        id: u64,
    ) -> TasksBrowserProps {
        let select_tx = self.ui_tx.clone();
        let filter_tx = self.ui_tx.clone();
        let refresh_tx = self.ui_tx.clone();
        let close_tx = self.ui_tx.clone();
        let stop_tx = self.ui_tx.clone();
        let refused_tx = self.ui_tx.clone();
        TasksBrowserProps {
            tasks: manager.list(false, None),
            filter: state.filter,
            selected_task_id: state.selected_task_id.clone(),
            output: state.output.clone(),
            output_loading: state.output_loading,
            notice: state.notice.clone(),
            on_select: Box::new(move |task_id: &str| {
                let _ = select_tx.send(UiMessage::TaskBrowser {
                    id,
                    action: TaskBrowserAction::Select(task_id.to_owned()),
                });
            }),
            on_toggle_filter: Box::new(move || {
                let _ = filter_tx.send(UiMessage::TaskBrowser {
                    id,
                    action: TaskBrowserAction::ToggleFilter,
                });
            }),
            on_refresh: Box::new(move || {
                let _ = refresh_tx.send(UiMessage::TaskBrowser {
                    id,
                    action: TaskBrowserAction::Refresh,
                });
            }),
            on_close: Box::new(move || {
                let _ = close_tx.send(UiMessage::SelectorCancelled { id });
            }),
            on_stop: Box::new(move |task_id: &str| {
                let _ = stop_tx.send(UiMessage::TaskBrowser {
                    id,
                    action: TaskBrowserAction::Stop(task_id.to_owned()),
                });
            }),
            on_stop_refused: Some(Box::new(move |task_id: &str| {
                let _ = refused_tx.send(UiMessage::TaskBrowser {
                    id,
                    action: TaskBrowserAction::StopRefused(task_id.to_owned()),
                });
            })),
        }
    }

    /// `push()` — re-feed the browser from the manager and the state.
    fn push_tasks_browser(&mut self, id: u64) {
        let (Some(manager), Some((browser, state))) =
            (self.session().task_manager(), self.tasks_browser.clone())
        else {
            return;
        };
        let rows = self.ui.rows().max(3) - 2;
        let props = self.tasks_browser_props(&manager, &state, id);
        {
            let mut browser = browser.borrow_mut();
            browser.set_rows(rows);
            browser.set_props(props);
        }
        self.ui.request_render();
    }

    /// `loadOutput(taskId)` — the read runs in the loop; a slower earlier read
    /// must not overwrite a newer selection, which the request counter guards.
    fn load_task_output(&mut self, task_id: &str, id: u64) {
        let Some(manager) = self.session().task_manager() else {
            return;
        };
        let Some((_, state)) = self.tasks_browser.as_mut() else {
            return;
        };
        state.request += 1;
        state.output_loading = true;
        let request = state.request;
        let task_id = task_id.to_owned();
        self.side_futures.push(Box::pin(async move {
            let output = manager.read_output(&task_id, Some(8000)).await;
            UiMessage::TaskBrowser {
                id,
                action: TaskBrowserAction::Output { request, output },
            }
        }));
        self.push_tasks_browser(id);
    }

    /// Everything the browser posted.
    fn handle_task_browser_action(&mut self, id: u64, action: TaskBrowserAction) {
        if self
            .active_selector
            .as_ref()
            .is_none_or(|active| active.id != id)
        {
            return;
        }
        match action {
            TaskBrowserAction::Select(task_id) => {
                let already_selected = self
                    .tasks_browser
                    .as_ref()
                    .is_some_and(|(_, state)| state.selected_task_id.as_deref() == Some(&task_id));
                if already_selected {
                    return;
                }
                if let Some((_, state)) = self.tasks_browser.as_mut() {
                    state.selected_task_id = Some(task_id.clone());
                    state.output = None;
                }
                self.load_task_output(&task_id, id);
            }
            TaskBrowserAction::ToggleFilter => {
                if let Some((_, state)) = self.tasks_browser.as_mut() {
                    state.filter = match state.filter {
                        TasksFilter::All => TasksFilter::Running,
                        TasksFilter::Running => TasksFilter::All,
                    };
                }
                self.push_tasks_browser(id);
            }
            TaskBrowserAction::Refresh => {
                let selected = self
                    .tasks_browser
                    .as_ref()
                    .and_then(|(_, state)| state.selected_task_id.clone());
                match selected {
                    Some(task_id) => self.load_task_output(&task_id, id),
                    None => self.push_tasks_browser(id),
                }
            }
            TaskBrowserAction::Output { request, output } => {
                let stale = self
                    .tasks_browser
                    .as_ref()
                    .is_some_and(|(_, state)| state.request != request);
                if stale {
                    return;
                }
                if let Some((_, state)) = self.tasks_browser.as_mut() {
                    state.output = Some(output);
                    state.output_loading = false;
                }
                self.push_tasks_browser(id);
            }
            TaskBrowserAction::Stop(task_id) => {
                self.flash_task_browser(id, &format!("Stopping {task_id}…"));
                if let Some(manager) = self.session().task_manager() {
                    self.side_futures.push(Box::pin(async move {
                        manager
                            .stop(&task_id, Some("Stopped from the task browser"))
                            .await;
                        UiMessage::TaskBrowser {
                            id,
                            action: TaskBrowserAction::Refresh,
                        }
                    }));
                }
            }
            TaskBrowserAction::StopRefused(task_id) => {
                self.flash_task_browser(id, &format!("{task_id} has already finished."));
            }
            TaskBrowserAction::ClearNotice => {
                if let Some((_, state)) = self.tasks_browser.as_mut() {
                    state.notice = None;
                    state.notice_until = None;
                }
                self.push_tasks_browser(id);
            }
        }
    }

    /// `flash(message)` of the browser: a footer notice for 2.5 seconds.
    fn flash_task_browser(&mut self, id: u64, message: &str) {
        if let Some((_, state)) = self.tasks_browser.as_mut() {
            state.notice = Some(message.to_owned());
            state.notice_until = Some(Instant::now() + Duration::from_millis(2500));
        }
        self.push_tasks_browser(id);
    }

    // ------------------------------------------------------------------
    // Login and logout
    // ------------------------------------------------------------------

    fn login_provider_options(&self, auth_type: Option<AuthType>) -> Vec<AuthSelectorProvider> {
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let mut options: Vec<AuthSelectorProvider> = Vec::new();
        for provider in model_runtime.get_providers() {
            let auth_status = model_runtime.get_provider_auth_status(provider.id());
            let status = auth_status.configured.then(|| AuthCheck {
                check_type: if model_runtime.is_using_oauth(provider.id()) {
                    AuthType::OAuth
                } else {
                    AuthType::ApiKey
                },
                source: auth_status.label.clone().or_else(|| {
                    auth_status
                        .source
                        .map(|source| format!("{source:?}").to_lowercase())
                }),
            });
            let auth = provider.auth();
            let experimental = provider.id() == mtplx::LOCAL_PROVIDER_ID;
            if auth_type.is_none_or(|wanted| wanted == AuthType::OAuth)
                && let Some(oauth) = auth.oauth.clone()
            {
                options.push(AuthSelectorProvider {
                    id: provider.id().to_owned(),
                    name: provider.name().to_owned(),
                    auth_type: AuthType::OAuth,
                    method: Some(AuthSelectorMethod::OAuth(oauth)),
                    status: status.clone(),
                    experimental,
                });
            }
            if auth_type.is_none_or(|wanted| wanted == AuthType::ApiKey)
                && let Some(api_key) = auth.api_key.clone()
            {
                options.push(AuthSelectorProvider {
                    id: provider.id().to_owned(),
                    name: provider.name().to_owned(),
                    auth_type: AuthType::ApiKey,
                    method: Some(AuthSelectorMethod::ApiKey(api_key)),
                    status: status.clone(),
                    experimental,
                });
            }
        }
        options.sort_by(|left, right| left.name.cmp(&right.name));
        options
    }

    fn find_login_provider_options(&self, provider_ref: &str) -> Vec<AuthSelectorProvider> {
        let normalized = provider_ref.trim().to_lowercase();
        if normalized.is_empty() {
            return Vec::new();
        }
        self.login_provider_options(None)
            .into_iter()
            .filter(|provider| {
                provider.id.to_lowercase() == normalized
                    || provider.name.to_lowercase() == normalized
            })
            .collect()
    }

    fn handle_login_command(&mut self, provider_ref: Option<&str>) {
        let Some(provider_ref) = provider_ref else {
            self.show_login_auth_type_selector(None);
            return;
        };
        let options = self.find_login_provider_options(provider_ref);
        if options.len() == 1 {
            self.start_provider_login(options[0].clone());
            return;
        }
        if options.len() > 1 {
            let first_id = options[0].id.as_str();
            if options
                .iter()
                .all(|provider| provider.id.as_str() == first_id)
            {
                self.show_login_auth_type_selector(Some(options));
                return;
            }
        }
        self.show_login_provider_selector(None, Some(provider_ref));
    }

    fn start_provider_login(&mut self, provider: AuthSelectorProvider) {
        match (&provider.auth_type, &provider.method) {
            (AuthType::OAuth, _) => self.show_login_dialog(provider, AuthType::OAuth),
            // `providerOption.method?.login` — a method without an interactive
            // setup is ambient-only.
            (AuthType::ApiKey, Some(AuthSelectorMethod::ApiKey(api_key)))
                if has_api_key_login(api_key.as_ref()) =>
            {
                self.show_login_dialog(provider, AuthType::ApiKey)
            }
            _ => self.show_ambient_auth_dialog(provider),
        }
    }

    fn show_login_auth_type_selector(
        &mut self,
        provider_options: Option<Vec<AuthSelectorProvider>>,
    ) {
        let oauth_login_label = provider_options.as_ref().and_then(|options| {
            options
                .iter()
                .find(|provider| provider.auth_type == AuthType::OAuth)
                .and_then(|provider| match &provider.method {
                    Some(AuthSelectorMethod::OAuth(oauth)) => {
                        oauth.login_label().map(str::to_owned)
                    }
                    _ => None,
                })
        });
        let subscription_label =
            oauth_login_label.unwrap_or_else(|| "Sign in with an account".to_owned());
        let api_key_label = "Sign in with an API key".to_owned();
        let available: Vec<AuthType> = match &provider_options {
            Some(options) => options.iter().map(|provider| provider.auth_type).collect(),
            None => vec![AuthType::OAuth, AuthType::ApiKey],
        };
        let mut labels: Vec<String> = Vec::new();
        if available.contains(&AuthType::OAuth) {
            labels.push(subscription_label.clone());
        }
        if available.contains(&AuthType::ApiKey) {
            labels.push(api_key_label.clone());
        }

        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let options_for_message = provider_options.clone();
        let selector = component_ref(ListSelectorComponent::new(
            "How do you want to sign in?",
            labels,
            Box::new(move |label| {
                let auth_type = if label == api_key_label {
                    AuthType::ApiKey
                } else {
                    AuthType::OAuth
                };
                let _ = select_tx.send(UiMessage::LoginAuthType {
                    id,
                    auth_type,
                    providers: options_for_message.clone(),
                });
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
            None,
        ));
        self.show_selector(Rc::clone(&selector), selector, None);
    }

    /// `showLoginProviderSelector(authType, initialSearchInput)`
    fn show_login_provider_selector(
        &mut self,
        auth_type: Option<AuthType>,
        initial_search_input: Option<&str>,
    ) {
        let providers = self.login_provider_options(auth_type);
        if providers.is_empty() {
            self.show_status("No providers available for that sign-in method");
            return;
        }
        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let by_id = providers.clone();
        let selector = component_ref(OAuthSelectorComponent::new(
            AuthSelectorMode::Login,
            providers,
            Box::new(move |provider_id: &str, auth_type: AuthType| {
                if let Some(provider) = by_id
                    .iter()
                    .find(|provider| provider.id == provider_id && provider.auth_type == auth_type)
                    .or_else(|| by_id.iter().find(|provider| provider.id == provider_id))
                    .cloned()
                {
                    let _ = select_tx.send(UiMessage::LoginProvider {
                        id,
                        provider: Box::new(provider),
                    });
                }
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
            initial_search_input,
        ));
        self.show_selector(Rc::clone(&selector), selector, None);
    }

    async fn show_logout_selector(&mut self) {
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let credentials = model_runtime
            .list_credentials(Some(AuthOperationOptions {
                signal: Some(timeout_signal(15_000)),
            }))
            .await;
        let credentials = match credentials {
            Ok(credentials) => credentials,
            Err(error) => {
                self.show_error(&format!("Could not read stored credentials: {error}"));
                return;
            }
        };
        if credentials.is_empty() {
            self.show_status(
                "No stored credentials to remove. /logout only removes credentials saved by /login; environment variables and models.json config are unchanged.",
            );
            return;
        }
        let mut providers: Vec<AuthSelectorProvider> = credentials
            .into_iter()
            .map(|credential| AuthSelectorProvider {
                name: model_runtime
                    .get_provider(&credential.provider_id)
                    .map(|provider| provider.name().to_owned())
                    .unwrap_or_else(|| credential.provider_id.clone()),
                experimental: credential.provider_id == mtplx::LOCAL_PROVIDER_ID,
                id: credential.provider_id,
                auth_type: credential.credential_type,
                method: None,
                status: Some(AuthCheck {
                    check_type: credential.credential_type,
                    source: Some("stored credential".to_owned()),
                }),
            })
            .collect();
        providers.sort_by(|left, right| left.name.cmp(&right.name));

        let id = self.selector_id + 1;
        let select_tx = self.ui_tx.clone();
        let cancel_tx = self.ui_tx.clone();
        let by_id = providers.clone();
        let selector = component_ref(OAuthSelectorComponent::new(
            AuthSelectorMode::Logout,
            providers,
            Box::new(move |provider_id: &str, _auth_type: AuthType| {
                if let Some(provider) = by_id
                    .iter()
                    .find(|provider| provider.id == provider_id)
                    .cloned()
                {
                    let _ = select_tx.send(UiMessage::LogoutProvider {
                        id,
                        provider: Box::new(provider),
                    });
                }
            }),
            Box::new(move || {
                let _ = cancel_tx.send(UiMessage::SelectorCancelled { id });
            }),
            None,
        ));
        self.show_selector(Rc::clone(&selector), selector, None);
    }

    /// The `onSelect` half of the logout selector.
    async fn logout_provider(&mut self, provider: AuthSelectorProvider) {
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let result = model_runtime
            .logout(
                &provider.id,
                Some(AuthOperationOptions {
                    signal: Some(timeout_signal(15_000)),
                }),
            )
            .await;
        match result {
            Ok(()) => {
                self.update_available_provider_count();
                let message = if provider.auth_type == AuthType::OAuth {
                    format!("Logged out of {}", provider.name)
                } else {
                    format!(
                        "Removed stored API key for {}. Environment variables and models.json config are unchanged.",
                        provider.name
                    )
                };
                self.show_status(&message);
            }
            Err(error) => self.show_error(&format!("Logout failed: {error}")),
        }
    }

    fn show_ambient_auth_dialog(&mut self, provider: AuthSelectorProvider) {
        let id = self.selector_id + 1;
        let tx = self.ui_tx.clone();
        let core = self.ui.clone();
        let dialog = Rc::new(RefCell::new(LoginDialogComponent::new(
            Rc::new(move || core.request_render()),
            &provider.id,
            Box::new(move |_success, _message| {
                let _ = tx.send(UiMessage::SelectorCancelled { id });
            }),
            Some(&provider.name),
            Some(&format!("{} setup", provider.name)),
        )));
        dialog.borrow_mut().show_info(
            &format!(
                "{} is configured outside notagent.",
                provider
                    .method
                    .as_ref()
                    .map(AuthSelectorMethod::name)
                    .unwrap_or("Authentication")
            ),
            &[],
            true,
        );
        self.show_selector(
            Rc::clone(&dialog) as ComponentRef,
            Rc::clone(&dialog) as ComponentRef,
            None,
        );
    }

    /// `showLoginDialog(providerId, providerName)` and `showApiKeyLoginDialog`
    /// Deviation (class 1): the auth flow runs wherever the runtime puts it and
    /// needs a `Send` interaction, while the dialog is `!Send`. The interaction
    /// therefore posts its prompts and events into this loop and waits on
    /// `oneshot`s, the same bridge the approval dialog uses.
    /// The login itself runs as a side-future beside this loop, like the free
    /// loop, which must keep serving that prompt/event bridge — the dialog
    /// then never got past its title and no browser opened (found 2026-08-18).
    fn show_login_dialog(&mut self, provider: AuthSelectorProvider, auth_type: AuthType) {
        let previous_model = self.session().model();
        let id = self.selector_id + 1;
        let complete_tx = self.ui_tx.clone();
        let core = self.ui.clone();
        let dialog = Rc::new(RefCell::new(LoginDialogComponent::new(
            Rc::new(move || core.request_render()),
            &provider.id,
            Box::new(move |_success, _message| {
                let _ = complete_tx.send(UiMessage::SelectorCancelled { id });
            }),
            Some(&provider.name),
            None,
        )));
        if provider.id == "amazon-bedrock" {
            dialog.borrow_mut().show_details(&[
                theme().fg(
                    ThemeColor::Text,
                    "You can also use an AWS profile, IAM keys, or role-based credentials.",
                ),
                theme().fg(ThemeColor::Muted, "See:"),
                theme().fg(
                    ThemeColor::Accent,
                    &format!("  {PROVIDER_DOCUMENTATION_URL}"),
                ),
            ]);
        }
        let signal = dialog.borrow().signal();
        self.login_dialog = Some(Rc::clone(&dialog));
        self.show_selector(
            Rc::clone(&dialog) as ComponentRef,
            Rc::clone(&dialog) as ComponentRef,
            None,
        );

        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let interaction = LoopAuthInteraction {
            tx: self.ui_tx.clone(),
            signal: signal.clone(),
        };
        let provider_id = provider.id.clone();
        let provider_name = provider.name.clone();
        self.side_futures.push(Box::pin(async move {
            let result = model_runtime
                .login(&provider_id, auth_type, &interaction)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());
            UiMessage::LoginFinished {
                id,
                provider_id,
                provider_name,
                auth_type,
                previous_model: previous_model.map(Box::new),
                result,
            }
        }));
    }

    async fn complete_provider_authentication(
        &mut self,
        provider_id: &str,
        provider_name: &str,
        auth_type: AuthType,
        previous_model: Option<Model>,
    ) {
        let action_label = if auth_type == AuthType::OAuth {
            format!("Logged in to {provider_name}")
        } else {
            format!("Saved API key for {provider_name}")
        };

        let mut selected_model: Option<Model> = None;
        let mut selection_error: Option<String> = None;
        if is_unknown_model(previous_model.as_ref()) {
            let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
            let provider_models: Vec<Model> = model_runtime
                .get_available_snapshot()
                .into_iter()
                .filter(|model| model.provider == provider_id)
                .collect();
            match default_model_for_provider(provider_id) {
                None => {
                    selection_error = Some(format!(
                        "{action_label}, but no default model is configured for provider \"{provider_id}\". Use /model to select a model."
                    ));
                }
                Some(_) if provider_models.is_empty() => {
                    selection_error = Some(format!(
                        "{action_label}, but no models are available for that provider. Use /model to select a model."
                    ));
                }
                Some(default_model_id) => {
                    match provider_models
                        .into_iter()
                        .find(|model| model.id == default_model_id)
                    {
                        None => {
                            selection_error = Some(format!(
                                "{action_label}, but its default model \"{default_model_id}\" is not available. Use /model to select a model."
                            ));
                        }
                        Some(model) => match self.session().set_model(model.clone()).await {
                            Ok(()) => selected_model = Some(model),
                            Err(message) => {
                                selection_error = Some(format!(
                                    "{action_label}, but selecting its default model failed: {message}. Use /model to select a model."
                                ));
                            }
                        },
                    }
                }
            }
        }

        self.update_available_provider_count();
        self.footer.borrow_mut().invalidate();
        self.update_editor_border_color();
        match selected_model {
            Some(model) => {
                self.show_status(&format!(
                    "{action_label}. Selected {}. Credentials saved to {}",
                    model.id,
                    get_auth_path().display()
                ));
                self.check_daxnuts_easter_egg(&model);
            }
            None => {
                self.show_status(&format!(
                    "{action_label}. Credentials saved to {}",
                    get_auth_path().display()
                ));
                if let Some(selection_error) = selection_error {
                    self.show_error(&selection_error);
                }
            }
        }

        // `void modelRuntime.refresh({providers: [providerId]})`
        let model_runtime = Arc::clone(&self.runtime.services().model_runtime);
        let provider_id = provider_id.to_owned();
        let signal = timeout_signal(15_000);
        self.side_futures.push(Box::pin(async move {
            let result = model_runtime
                .refresh(notagent_ai::models::ModelsRefreshOptions {
                    providers: Some(vec![provider_id]),
                    signal: Some(signal),
                    ..notagent_ai::models::ModelsRefreshOptions::default()
                })
                .await;
            UiMessage::LoginCatalogRefreshed {
                action_label,
                aborted: result.aborted,
                failed: !result.errors.is_empty(),
            }
        }));
    }

    /// The prompt half of the login interaction, on this loop.
    fn handle_auth_prompt(
        &mut self,
        prompt: AuthPrompt,
        answer: tokio::sync::oneshot::Sender<Result<String, String>>,
    ) {
        let Some(dialog) = self.login_dialog.clone() else {
            let _ = answer.send(Err("Login cancelled".to_owned()));
            return;
        };
        match prompt.kind {
            AuthPromptKind::Select { message, options } => {
                // `showAuthSelect`: the list replaces the dialog and hands it back.
                let labels: Vec<String> =
                    options.iter().map(|option| option.label.clone()).collect();
                let answer = Rc::new(RefCell::new(Some(answer)));
                let select_answer = Rc::clone(&answer);
                let cancel_answer = Rc::clone(&answer);
                let tx = self.ui_tx.clone();
                let cancel_tx = self.ui_tx.clone();
                let selector = component_ref(ListSelectorComponent::new(
                    &message,
                    labels,
                    Box::new(move |label| {
                        let id = options
                            .iter()
                            .find(|option| option.label == label)
                            .map(|option| option.id.clone());
                        if let Some(answer) = select_answer.borrow_mut().take() {
                            let _ = answer.send(match id {
                                Some(id) => Ok(id),
                                None => Err("Login cancelled".to_owned()),
                            });
                        }
                        let _ = tx.send(UiMessage::RestoreLoginDialog);
                    }),
                    Box::new(move || {
                        if let Some(answer) = cancel_answer.borrow_mut().take() {
                            let _ = answer.send(Err("Login cancelled".to_owned()));
                        }
                        let _ = cancel_tx.send(UiMessage::RestoreLoginDialog);
                    }),
                    None,
                ));
                let mut container = self.editor_container.borrow_mut();
                container.clear();
                container.add_child(Rc::clone(&selector));
                drop(container);
                self.ui.set_focus(Some(selector));
                self.ui.request_render();
            }
            kind => {
                let input = {
                    let mut dialog = dialog.borrow_mut();
                    match kind {
                        AuthPromptKind::ManualCode { message, .. } => {
                            dialog.show_manual_input(&message)
                        }
                        AuthPromptKind::Text {
                            message,
                            placeholder,
                        }
                        | AuthPromptKind::Secret {
                            message,
                            placeholder,
                        } => dialog.show_prompt(&message, placeholder.as_deref()),
                        AuthPromptKind::Select { .. } => unreachable!("handled above"),
                    }
                };
                // The dialog answers on its own channel; the loop forwards it.
                self.side_futures.push(Box::pin(async move {
                    let value = input
                        .await
                        .unwrap_or(Err(LoginCancelled))
                        .map_err(|_| "Login cancelled".to_owned());
                    let _ = answer.send(value);
                    UiMessage::Noop
                }));
            }
        }
    }

    /// The notify half of the login interaction (`notifyAuthDialog`).
    fn handle_auth_event(&mut self, event: AuthEvent) {
        let Some(dialog) = self.login_dialog.clone() else {
            return;
        };
        let mut dialog = dialog.borrow_mut();
        match event {
            AuthEvent::AuthUrl { url, instructions } => {
                dialog.show_auth(&url, instructions.as_deref())
            }
            AuthEvent::DeviceCode {
                user_code,
                verification_uri,
                interval_seconds,
                expires_in_seconds,
            } => {
                dialog.show_device_code(&OAuthDeviceCodeInfo {
                    user_code,
                    verification_uri,
                    interval_seconds,
                    expires_in_seconds,
                });
                dialog.show_waiting("Waiting for authentication...");
            }
            AuthEvent::Info { message, links } => dialog.show_info(&message, &links, false),
            AuthEvent::Progress { message } => dialog.show_progress(&message),
        }
        self.ui.request_render();
    }

    // ------------------------------------------------------------------
    // Bash mode
    // ------------------------------------------------------------------

    /// `handleBashCommand(command, excludeFromContext)`
    /// and the result it could hand back (class 2).
    /// Deviation (class 1): the run is not awaited here but driven by the main
    /// loop, because its chunk callback is `Send` and has to cross into the
    /// loop to reach the `!Send` component — awaiting inline would collect the
    /// whole output and show it in one go.
    fn handle_bash_command(&mut self, command: &str, exclude_from_context: bool) {
        let component = Rc::new(RefCell::new(BashExecutionComponent::new(
            command,
            exclude_from_context,
        )));
        let is_deferred = self.session().is_streaming();
        if is_deferred {
            self.pending_messages_container
                .borrow_mut()
                .add_child(Rc::clone(&component) as ComponentRef);
            self.pending_bash_components.push(Rc::clone(&component));
        } else {
            self.chat_container
                .borrow_mut()
                .add_child(Rc::clone(&component) as ComponentRef);
        }
        self.bash_component = Some(component);
        self.ui.request_render();

        let session = self.session();
        let command = command.to_owned();
        let tx = self.bash_tx.clone();
        self.pending_bash = Some(PendingBash {
            command: command.clone(),
            exclude_from_context,
            run: Box::pin(async move {
                session
                    .execute_bash(
                        &command,
                        Some(Arc::new(move |chunk: &str| {
                            let _ = tx.send(chunk.to_owned());
                        })),
                        ExecuteBashOptions {
                            exclude_from_context,
                            ..ExecuteBashOptions::default()
                        },
                    )
                    .await
            }),
        });
    }

    /// The tail of `handleBashCommand` after the `await`.
    fn finish_bash_command(
        &mut self,
        command: &str,
        exclude_from_context: bool,
        result: BashResult,
    ) {
        if let Some(component) = self.bash_component.take() {
            component.borrow_mut().set_complete(
                result.exit_code.map(i64::from),
                result.cancelled,
                // (`{ truncated: true, content } as TruncationResult`); the row
                // reads exactly those two fields, the rest is zero here
                result.truncated.then(|| TruncationResult {
                    content: result.output.clone(),
                    truncated: true,
                    truncated_by: None,
                    total_lines: 0,
                    total_bytes: 0,
                    output_lines: 0,
                    output_bytes: 0,
                    last_line_partial: false,
                    first_line_exceeds_limit: false,
                    max_lines: 0,
                    max_bytes: 0,
                }),
                result.full_output_path.clone(),
            );
        }
        // records it on the extension path.
        let _ = (command, exclude_from_context);
        // `onSubmit` resets the bash border once the command is done.
        self.is_bash_mode = false;
        self.update_editor_border_color();
        self.ui.request_render();
    }

    fn flush_pending_bash_components(&mut self) {
        for component in std::mem::take(&mut self.pending_bash_components) {
            self.pending_messages_container
                .borrow_mut()
                .remove_child(&(Rc::clone(&component) as ComponentRef));
            self.chat_container
                .borrow_mut()
                .add_child(component as ComponentRef);
        }
    }

    // ------------------------------------------------------------------
    // Queues
    // ------------------------------------------------------------------

    fn all_queued_messages(&self) -> (Vec<String>, Vec<String>) {
        let session = self.session();
        let mut steering = session.get_steering_messages();
        steering.extend(
            self.compaction_queued_messages
                .iter()
                .filter(|message| message.mode == CompactionQueueMode::Steer)
                .map(|message| message.text.clone()),
        );
        let mut follow_up = session.get_follow_up_messages();
        follow_up.extend(
            self.compaction_queued_messages
                .iter()
                .filter(|message| message.mode == CompactionQueueMode::FollowUp)
                .map(|message| message.text.clone()),
        );
        (steering, follow_up)
    }

    fn clear_all_queues(&mut self) -> (Vec<String>, Vec<String>) {
        let (mut steering, mut follow_up) = self.session().clear_queue();
        for message in std::mem::take(&mut self.compaction_queued_messages) {
            match message.mode {
                CompactionQueueMode::Steer => steering.push(message.text),
                CompactionQueueMode::FollowUp => follow_up.push(message.text),
            }
        }
        (steering, follow_up)
    }

    fn update_pending_messages_display(&mut self) {
        let (steering, follow_up) = self.all_queued_messages();
        let mut container = self.pending_messages_container.borrow_mut();
        container.clear();
        // The bash components live in the same container and outlive the queue.
        for component in &self.pending_bash_components {
            container.add_child(Rc::clone(component) as ComponentRef);
        }
        if steering.is_empty() && follow_up.is_empty() {
            return;
        }
        container.add_child(component_ref(Spacer::new(1)));
        for message in &steering {
            container.add_child(component_ref(TruncatedText::new(
                theme().fg(ThemeColor::Dim, &format!("Steering: {message}")),
                1,
                0,
            )));
        }
        for message in &follow_up {
            container.add_child(component_ref(TruncatedText::new(
                theme().fg(ThemeColor::Dim, &format!("Follow-up: {message}")),
                1,
                0,
            )));
        }
        let hint = format!(
            "↳ {} to edit all queued messages",
            key_display_text("app.message.dequeue")
        );
        container.add_child(component_ref(TruncatedText::new(
            theme().fg(ThemeColor::Dim, &hint),
            1,
            0,
        )));
    }

    fn restore_queued_messages_to_editor(&mut self, abort: bool) -> usize {
        let (steering, follow_up) = self.clear_all_queues();
        let mut all_queued = steering;
        all_queued.extend(follow_up);
        if all_queued.is_empty() {
            self.update_pending_messages_display();
            if abort {
                self.session().request_abort();
            }
            return 0;
        }
        let queued_text = all_queued.join("\n\n");
        let current_text = self.editor.borrow().editor().get_text();
        let combined = [queued_text, current_text]
            .into_iter()
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.editor.borrow_mut().editor_mut().set_text(&combined);
        self.update_pending_messages_display();
        if abort {
            self.session().request_abort();
        }
        all_queued.len()
    }

    fn handle_dequeue(&mut self) {
        let restored = self.restore_queued_messages_to_editor(false);
        if restored == 0 {
            self.show_status("No queued messages to restore");
        } else {
            self.show_status(&format!(
                "Restored {restored} queued message{} to editor",
                if restored > 1 { "s" } else { "" }
            ));
        }
    }

    async fn handle_follow_up(&mut self) {
        let text = self.editor.borrow().editor().get_expanded_text();
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }

        if self.session().is_compacting() {
            self.queue_compaction_message(&text, CompactionQueueMode::FollowUp);
            return;
        }

        if self.session().is_streaming() {
            self.editor.borrow_mut().editor_mut().add_to_history(&text);
            self.clear_editor_text();
            let session = self.session();
            if let Err(message) = session
                .prompt(
                    &text,
                    PromptOptions {
                        streaming_behavior: Some(QueueBehavior::FollowUp),
                        ..PromptOptions::default()
                    },
                )
                .await
            {
                self.show_error(&message);
            }
            self.update_pending_messages_display();
            self.ui.request_render();
        } else {
            // Not streaming: Alt+Enter behaves like Enter.
            self.clear_editor_text();
            self.handle_submit(text).await;
        }
    }

    fn queue_compaction_message(&mut self, text: &str, mode: CompactionQueueMode) {
        self.compaction_queued_messages
            .push(CompactionQueuedMessage {
                text: text.to_owned(),
                mode,
            });
        self.editor.borrow_mut().editor_mut().add_to_history(text);
        self.clear_editor_text();
        self.update_pending_messages_display();
        self.show_status("Queued message for after compaction");
    }

    /// `isExtensionCommand` is constantly false without the extension system
    /// (class 2), which removes the pre-command branches; what stays is the
    /// first queued message as the prompt and the rest back into the queues.
    async fn flush_compaction_queue(&mut self, will_retry: bool) {
        if self.compaction_queued_messages.is_empty() {
            return;
        }
        let queued: Vec<CompactionQueuedMessage> =
            std::mem::take(&mut self.compaction_queued_messages);
        self.update_pending_messages_display();
        let session = self.session();

        let restore = |mode: &mut Self, error: String, queued: Vec<CompactionQueuedMessage>| {
            mode.session().clear_queue();
            let count = queued.len();
            mode.compaction_queued_messages = queued;
            mode.update_pending_messages_display();
            mode.show_error(&format!(
                "Failed to send queued message{}: {error}",
                if count > 1 { "s" } else { "" }
            ));
        };

        if will_retry {
            // When a retry is pending the messages go into the queues of the
            // upcoming turn. Both calls are synchronous and infallible in the
            // port, so the `restoreQueue` branch around them cannot trigger.
            for message in &queued {
                match message.mode {
                    CompactionQueueMode::FollowUp => session.follow_up(&message.text, &[]),
                    CompactionQueueMode::Steer => session.steer(&message.text, &[]),
                }
            }
            self.update_pending_messages_display();
            return;
        }

        let mut queued = queued.into_iter();
        let first = queued.next().expect("queue is not empty");
        let rest: Vec<CompactionQueuedMessage> = queued.collect();
        let prompt = session
            .prompt(
                &first.text,
                PromptOptions {
                    streaming_behavior: Some(match first.mode {
                        CompactionQueueMode::Steer => QueueBehavior::Steer,
                        CompactionQueueMode::FollowUp => QueueBehavior::FollowUp,
                    }),
                    ..PromptOptions::default()
                },
            )
            .await;
        if let Err(error) = prompt {
            let mut restored = vec![first];
            restored.extend(rest);
            restore(self, error, restored);
            return;
        }
        for message in &rest {
            match message.mode {
                CompactionQueueMode::FollowUp => session.follow_up(&message.text, &[]),
                CompactionQueueMode::Steer => session.steer(&message.text, &[]),
            }
        }
        self.update_pending_messages_display();
    }

    // ------------------------------------------------------------------
    // Slash commands
    // ------------------------------------------------------------------

    /// must not reach the model.
    /// The commands that open a selector (`/settings`, `/model`,
    /// `/scoped-models`, `/tasks`, `/fork`, `/clone`, `/tree`, `/trust`,
    /// `/login`, `/logout`, `/resume`) are recognised here and answered with a
    /// notice until the selector slice wires them; they must never fall through
    /// to the model, which is what the fall-through of an unknown `/word` does
    async fn handle_slash_command(&mut self, text: &str) -> bool {
        if !text.starts_with('/') {
            return false;
        }
        let argument = |prefix: &str| -> Option<String> {
            text.strip_prefix(prefix)
                .map(|rest| rest.trim().to_owned())
                .filter(|rest| !rest.is_empty())
        };

        match text {
            "/effort" => {
                self.clear_editor_text();
                self.show_effort_selector();
            }
            "/settings" => {
                self.show_settings_selector();
                self.clear_editor_text();
            }
            "/scoped-models" => {
                self.clear_editor_text();
                self.show_models_selector();
            }
            _ if text == "/model" || text.starts_with("/model ") => {
                let search_term = argument("/model ");
                self.clear_editor_text();
                self.handle_model_command(search_term.as_deref()).await;
            }
            _ if text == "/subagent-model" || text.starts_with("/subagent-model ") => {
                let search_term = argument("/subagent-model ");
                self.clear_editor_text();
                self.handle_subagent_model_command(search_term.as_deref())
                    .await;
            }
            _ if text == "/index" || text.starts_with("/index ") => {
                let index_argument = argument("/index ");
                self.clear_editor_text();
                self.handle_index_command(index_argument.as_deref());
            }
            // v0.1.19): the command form of the reference's Atomic leases
            // setting.
            _ if text == "/leases" || text.starts_with("/leases ") => {
                let leases_argument = argument("/leases ");
                self.clear_editor_text();
                self.handle_leases_command(leases_argument.as_deref());
            }
            // v0.1.20): the command form of the reference's shell-output
            // filter setting.
            _ if text == "/bash-filter" || text.starts_with("/bash-filter ") => {
                let filter_argument = argument("/bash-filter ");
                self.clear_editor_text();
                self.handle_bash_filter_command(filter_argument.as_deref());
            }
            // v0.1.21): goal mode.
            _ if text == "/goal" || text.starts_with("/goal ") => {
                let goal_argument = argument("/goal ");
                self.clear_editor_text();
                self.handle_goal_command(goal_argument.as_deref());
            }
            // v0.1.22): the configured MCP servers.
            _ if text == "/mcp" || text.starts_with("/mcp ") => {
                let mcp_argument = argument("/mcp ");
                self.clear_editor_text();
                self.handle_mcp_command(mcp_argument.as_deref()).await;
            }
            // v0.1.11): the command form of the thinking-block toggle, which
            // until now only existed as a keybinding.
            "/thinking" => {
                self.clear_editor_text();
                self.toggle_thinking_block_visibility();
            }
            _ if text == "/export" || text.starts_with("/export ") => {
                self.handle_export_command(text).await;
                self.clear_editor_text();
            }
            _ if text == "/import" || text.starts_with("/import ") => {
                self.handle_import_command(text).await;
                self.clear_editor_text();
            }
            "/share" => {
                self.handle_share_command().await;
                self.clear_editor_text();
            }
            "/copy" => {
                self.handle_copy_command(false);
                self.clear_editor_text();
            }
            _ if text == "/name" || text.starts_with("/name ") => {
                self.handle_name_command(text);
                self.clear_editor_text();
            }
            "/session" => {
                self.handle_session_command();
                self.clear_editor_text();
            }
            "/changelog" => {
                self.handle_changelog_command();
                self.clear_editor_text();
            }
            "/tasks" | "/task" => {
                self.show_tasks_browser();
                self.clear_editor_text();
            }
            "/hotkeys" => {
                self.handle_hotkeys_command();
                self.clear_editor_text();
            }
            "/fork" => {
                self.show_user_message_selector();
                self.clear_editor_text();
            }
            "/clone" => {
                self.clear_editor_text();
                self.handle_clone_command().await;
            }
            "/tree" => {
                self.show_tree_selector(None);
                self.clear_editor_text();
            }
            "/trust" => {
                self.show_trust_selector();
                self.clear_editor_text();
            }
            _ if text == "/login" || text.starts_with("/login ") => {
                let provider_ref = argument("/login ");
                self.clear_editor_text();
                self.handle_login_command(provider_ref.as_deref());
            }
            "/logout" => {
                self.show_logout_selector().await;
                self.clear_editor_text();
            }
            "/llama" => {
                // model layer through `session.prompt`, which is why it lands
                // in the editor history like a prompt does; the built-in
                // commands above return before that.
                self.editor.borrow_mut().editor_mut().add_to_history(text);
                self.clear_editor_text();
                self.handle_llama_command().await;
            }
            "/new" => {
                self.clear_editor_text();
                self.handle_clear_command().await;
            }
            _ if text == "/compact" || text.starts_with("/compact ") => {
                let instructions = argument("/compact ");
                self.clear_editor_text();
                self.handle_compact_command(instructions);
            }
            "/init" => {
                self.clear_editor_text();
                self.handle_init_command().await;
            }
            _ if text == "/btw" || text.starts_with("/btw ") => {
                let question = argument("/btw ");
                self.clear_editor_text();
                self.handle_side_question_command(question.as_deref());
            }
            "/reload" => {
                self.clear_editor_text();
                self.handle_reload_command().await;
            }
            "/debug" => {
                self.handle_debug_command();
                self.clear_editor_text();
            }
            "/arminsayshi" => {
                self.handle_armin_says_hi();
                self.clear_editor_text();
            }
            "/dementedelves" => {
                self.handle_demented_delves();
                self.clear_editor_text();
            }
            "/resume" => {
                self.show_session_selector();
                self.clear_editor_text();
            }
            "/quit" => {
                self.clear_editor_text();
                self.shutdown().await;
            }
            // Anything else is not a command: skill commands and prompt
            // templates are expanded by `session.prompt`.
            _ => return false,
        }
        true
    }

    fn clear_editor_text(&mut self) {
        self.editor.borrow_mut().editor_mut().set_text("");
    }

    fn restore_rejected_prompt(&mut self, submitted_text: &str) {
        let current = self.editor.borrow().editor().get_text();
        let restored = if current.trim().is_empty() {
            submitted_text.to_owned()
        } else {
            format!("{submitted_text}\n\n{current}")
        };
        self.editor.borrow_mut().editor_mut().set_text(&restored);
        self.ui.request_render();
    }

    /// `showExtensionCustom` here: the editor's text is saved, the view takes
    /// the editor's place, and the text comes back when the flow is done.
    async fn handle_llama_command(&mut self) {
        let notify_tx = self.ui_tx.clone();
        let notify: Notify = Rc::new(move |message: &str, level: NotifyLevel| {
            let _ = notify_tx.send(UiMessage::Notify {
                message: message.to_owned(),
                level,
            });
        });
        let services = self.runtime.services();
        let registry = ModelRegistry::new(Arc::clone(&services.model_runtime));
        let client = match llama_client_for_command(&registry, &notify).await {
            Ok(Some(client)) => client,
            // `configuredClient` already told the user to run `/login`.
            Ok(None) => return,
            Err(error) => {
                self.show_error(&error);
                return;
            }
        };
        let command = LlamaCommand {
            client,
            provider: Arc::clone(&services.llama),
            registry,
            notify,
        };

        let core = self.ui.clone();
        let (view, mut ui) = create_llama_ui(Rc::new(move || core.request_render()));
        let saved_text = self.editor.borrow().editor().get_text();
        self.dispose_active_selector();
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(Rc::clone(&view) as ComponentRef);
        }
        self.ui.set_focus(Some(Rc::clone(&view) as ComponentRef));
        self.ui.request_render();
        self.side_futures.push(Box::pin(async move {
            let error = run_llama_command(&command, &mut ui).await.err();
            // The view has to outlive the flow: it holds the answer channel.
            drop(view);
            UiMessage::LlamaFinished { error, saved_text }
        }));
    }

    /// `restoreEditor()` of `showExtensionCustom`
    fn restore_editor(&mut self, saved_text: &str) {
        {
            let mut container = self.editor_container.borrow_mut();
            container.clear();
            container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.editor.borrow_mut().editor_mut().set_text(saved_text);
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.ui.request_render();
    }

    async fn handle_export_command(&mut self, text: &str) {
        let output_path = path_command_argument(text, "/export");
        let session = self.session();
        let result = if output_path
            .as_deref()
            .is_some_and(|path| path.ends_with(".jsonl"))
        {
            session.export_to_jsonl(output_path.as_deref())
        } else {
            self.export_to_html(output_path.as_deref())
        };
        match result {
            Ok(path) => self.show_status(&format!("Session exported to: {path}")),
            Err(message) => self.show_error(&format!("Failed to export session: {message}")),
        }
    }

    /// `session.exportToHtml(outputPath)` — the session-side half lives in
    fn export_to_html(&self, output_path: Option<&str>) -> Result<String, String> {
        let session = self.session();
        // pre-rendered through the same renderers the transcript uses, so the
        // page shows what the session showed.
        let definitions = Arc::clone(&session);
        let renderer = ToolDefinitionHtmlRenderer::new(
            Box::new(move |name: &str| definitions.get_tool_definition(name)),
            theme(),
            self.cwd(),
        );
        let theme_name = self.settings().get_theme().filter(|name| {
            crate::modes::interactive::theme::theme::get_theme_by_name(name).is_some()
        });
        session.with_session_manager(|manager| {
            crate::core::export_html::export_session_to_html(
                manager,
                None,
                crate::core::export_html::ExportOptions {
                    output_path: output_path.map(str::to_owned),
                    theme_name,
                    tool_renderer: Some(&renderer),
                },
            )
            .map_err(|error| error.to_string())
        })
    }

    async fn handle_import_command(&mut self, text: &str) {
        let Some(input_path) = path_command_argument(text, "/import") else {
            self.show_error("Usage: /import <path.jsonl>");
            return;
        };
        let confirmed = self
            .confirm(
                "Import session",
                &format!("Replace current session with {input_path}?"),
            )
            .await;
        if !confirmed {
            self.show_status("Import cancelled");
            return;
        }
        self.clear_status_indicator(None);
        match self.runtime.import_from_jsonl(&input_path, None).await {
            Ok(()) => {
                self.rebind_current_session();
                self.show_status(&format!("Session imported from: {input_path}"));
            }
            Err(SessionOpenError::MissingCwd(error)) => {
                let Some(selected_cwd) = self.prompt_for_missing_session_cwd(&error).await else {
                    self.show_status("Import cancelled");
                    return;
                };
                match self
                    .runtime
                    .import_from_jsonl(&input_path, Some(&selected_cwd))
                    .await
                {
                    Ok(()) => {
                        self.rebind_current_session();
                        self.show_status(&format!("Session imported from: {input_path}"));
                    }
                    Err(error) => {
                        self.show_error(&format!("Failed to import session: {error}"));
                    }
                }
            }
            Err(error) => self.show_error(&format!("Failed to import session: {error}")),
        }
    }

    /// editor swap and the temp file are here.
    async fn handle_share_command(&mut self) {
        let runner = crate::core::share::ProcessShareCommandRunner;
        if let Err(error) = crate::core::share::check_gh_auth(&runner) {
            self.show_error(&error.to_string());
            return;
        }

        let temp_file = crate::core::share::share_temp_html_path();
        if let Err(message) = self.export_to_html(Some(&temp_file.to_string_lossy())) {
            self.show_error(&format!("Failed to export session: {message}"));
            return;
        }

        let loader = Rc::new(RefCell::new(BorderedLoader::new(
            &theme(),
            "Creating gist...",
            None,
        )));
        let signal = loader.borrow().signal();
        {
            let mut editor_container = self.editor_container.borrow_mut();
            editor_container.clear();
            editor_container.add_child(Rc::clone(&loader) as ComponentRef);
        }
        self.ui.set_focus(Some(Rc::clone(&loader) as ComponentRef));
        self.ui.request_render();

        // `loader.onAbort` kills the child; the cancellation token reaches the
        let result = tokio::select! {
            result = crate::core::share::create_secret_gist(&temp_file, &runner) => Some(result),
            () = signal.cancelled() => None,
        };

        loader.borrow_mut().dispose();
        {
            let mut editor_container = self.editor_container.borrow_mut();
            editor_container.clear();
            editor_container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        let _ = std::fs::remove_file(&temp_file);

        match result {
            None => self.show_status("Share cancelled"),
            Some(Ok(gist)) => self.show_status(&format!(
                "Share URL: {}\nGist: {}",
                gist.viewer_url, gist.gist_url
            )),
            Some(Err(error)) => self.show_error(&error.to_string()),
        }
    }

    fn handle_copy_command(&mut self, flash_confirmation: bool) {
        let Some(text) = self.session().get_last_assistant_text() else {
            self.show_error("No agent messages to copy yet.");
            return;
        };
        match crate::utils::clipboard::copy_to_clipboard(&text) {
            Ok(()) => {
                // The alternate screen flashes instead of writing a status line.
                if !(flash_confirmation && self.cell.flash("Copied!")) {
                    self.show_status("Copied last agent message to clipboard");
                }
            }
            Err(message) => self.show_error(&message),
        }
    }

    fn handle_name_command(&mut self, text: &str) {
        let name = text
            .strip_prefix("/name")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let session = self.session();
        if name.is_empty() {
            let current = session.with_session_manager(|manager| manager.get_session_name());
            match current {
                Some(current) => {
                    let mut chat = self.chat_container.borrow_mut();
                    chat.add_child(component_ref(Spacer::new(1)));
                    chat.add_child(component_ref(Text::new(
                        theme().fg(ThemeColor::Dim, &format!("Session name: {current}")),
                        1,
                        0,
                    )));
                }
                None => self.show_warning("Usage: /name <name>"),
            }
            self.ui.request_render();
            return;
        }

        session.set_session_name(&name);
        let session_name = session.with_session_manager(|manager| manager.get_session_name());
        if session_name.as_deref() != Some(name.as_str()) {
            self.show_warning(&format!(
                "Session name was normalized from {} to {}",
                json_string(&name),
                match session_name.as_deref() {
                    Some(normalized) => json_string(normalized),
                    None => "undefined".to_owned(),
                }
            ));
        }
        let shown = session_name.unwrap_or(name);
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(Text::new(
            theme().fg(ThemeColor::Dim, &format!("Session name set: {shown}")),
            1,
            0,
        )));
        drop(chat);
        self.ui.request_render();
    }

    fn handle_session_command(&mut self) {
        let session = self.session();
        let stats = session.get_session_stats();
        let session_name = session.with_session_manager(|manager| manager.get_session_name());
        let entries = session.with_session_manager(|manager| manager.get_entries());
        let model_runtime = self.runtime.services().model_runtime.clone();
        let cache_waste = crate::core::cache_stats::compute_cache_waste(&entries, &*model_runtime);
        let usage_breakdown = crate::core::usage_totals::get_usage_cost_breakdown(&entries);
        let paint = theme();
        let dim = |text: &str| paint.fg(ThemeColor::Dim, text);

        let mut info = format!("{}\n\n", paint.bold("Session Info"));
        if let Some(name) = session_name {
            info.push_str(&format!("{} {name}\n", dim("Name:")));
        }
        info.push_str(&format!(
            "{} {}\n",
            dim("File:"),
            stats.session_file.as_deref().unwrap_or("In-memory")
        ));
        info.push_str(&format!("{} {}\n\n", dim("ID:"), stats.session_id));
        info.push_str(&format!("{}\n", paint.bold("Messages")));
        info.push_str(&format!("{} {}\n", dim("Total:"), stats.total_messages));
        info.push_str(&format!("{} {}\n", dim("User:"), stats.user_messages));
        info.push_str(&format!(
            "{} {}\n",
            dim("Assistant:"),
            stats.assistant_messages
        ));
        info.push_str(&format!(
            "{} {} calls, {} results\n\n",
            dim("Tools:"),
            stats.tool_calls,
            stats.tool_results
        ));
        info.push_str(&format!("{}\n", paint.bold("Tokens")));
        let input = stats.tokens.input;
        let cache_read = stats.tokens.cache_read;
        let cache_write = stats.tokens.cache_write;
        let prompt_tokens = input + cache_read + cache_write;
        info.push_str(&format!(
            "{} {}\n",
            dim("Input:"),
            to_locale_string(prompt_tokens)
        ));
        if prompt_tokens > 0 && (cache_read > 0 || cache_write > 0) {
            let hit_rate = dim(&format!(
                "({:.1}%)",
                (cache_read as f64 / prompt_tokens as f64) * 100.0
            ));
            info.push_str(&format!(
                "  {} {} {hit_rate}\n",
                dim("Cached:"),
                to_locale_string(cache_read)
            ));
            let written = if cache_write > 0 {
                format!(
                    " {}",
                    dim(&format!(
                        "({} written to cache)",
                        to_locale_string(cache_write)
                    ))
                )
            } else {
                String::new()
            };
            info.push_str(&format!(
                "  {} {}{written}\n",
                dim("Uncached:"),
                to_locale_string(input + cache_write)
            ));
        }
        info.push_str(&format!(
            "{} {}\n",
            dim("Output:"),
            to_locale_string(stats.tokens.output)
        ));
        info.push_str(&format!(
            "{} {}\n",
            dim("Total:"),
            to_locale_string(stats.tokens.total)
        ));

        if stats.cost > 0.0 || cache_waste.missed_tokens > 0 {
            info.push_str(&format!("\n{}\n", paint.bold("Cost")));
            info.push_str(&format!("{} ${:.3}", dim("Total:"), stats.cost));
            if usage_breakdown.len() > 1 {
                for entry in &usage_breakdown {
                    info.push_str(&format!(
                        "\n  {} ${:.3} {}",
                        dim(&format!("{}:", entry.key)),
                        entry.cost,
                        dim(&format!("({} tokens)", format_tokens(entry.tokens)))
                    ));
                }
            }
            if cache_waste.missed_tokens > 0 {
                let miss_label = if cache_waste.miss_count == 1 {
                    "1 miss".to_owned()
                } else {
                    format!("{} misses", cache_waste.miss_count)
                };
                let detail = format!(
                    "{} tokens, {miss_label}",
                    to_locale_string(cache_waste.missed_tokens)
                );
                if cache_waste.missed_cost >= 0.0001 {
                    info.push_str(&format!(
                        "\n{} ${:.3} {}",
                        dim("Cache Re-billed:"),
                        cache_waste.missed_cost,
                        dim(&format!("({detail})"))
                    ));
                } else {
                    info.push_str(&format!("\n{} {detail}", dim("Cache Re-billed:")));
                }
            }
        }

        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(Text::new(info, 1, 0)));
        drop(chat);
        self.ui.request_render();
    }

    fn handle_changelog_command(&mut self) {
        let entries =
            crate::utils::changelog::parse_changelog(&crate::config::get_changelog_path());
        let changelog_markdown = if entries.is_empty() {
            "No changelog entries found.".to_owned()
        } else {
            entries
                .iter()
                .rev()
                .map(|entry| {
                    crate::utils::changelog::normalize_changelog_links_for_entry(
                        &entry.content,
                        entry,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        self.add_bordered_block("What's New", &changelog_markdown, 1);
    }

    fn handle_hotkeys_command(&mut self) {
        let key = key_display_text;
        let hotkeys = format!(
            "**Navigation**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{}` / `{}` / `{}` / `{}` | Move cursor / browse history |\n\
             | `{}` / `{}` | Move by word |\n\
             | `{}` | Start of line |\n\
             | `{}` | End of line |\n\
             | `{}` | Jump forward to character |\n\
             | `{}` | Jump backward to character |\n\
             | `{}` / `{}` | Scroll by page |\n\
             \n**Editing**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{}` | Send message |\n\
             | `{}` | New line{} |\n\
             | `{}` | Delete word backwards |\n\
             | `{}` | Delete word forwards |\n\
             | `{}` | Delete to start of line |\n\
             | `{}` | Delete to end of line |\n\
             | `{}` | Paste the most-recently-deleted text |\n\
             | `{}` | Cycle through the deleted text after pasting |\n\
             | `{}` | Undo |\n\
             \n**Other**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{}` | Path completion / accept autocomplete |\n\
             | `{}` | Cancel autocomplete / abort streaming |\n\
             | `{}` | Clear editor (first) / exit (second) |\n\
             | `{}` | Exit (when editor is empty) |\n\
             | `{}` | Suspend to background |\n\
             | `{}` | Cycle thinking level |\n\
             | `{}` / `{}` | Cycle models |\n\
             | `{}` | Open model selector |\n\
             | `{}` | Toggle tool output expansion |\n\
             | `{}` | Toggle thinking block visibility |\n\
             | `{}` | Edit message in external editor |\n\
             | `{}` | Copy last assistant message |\n\
             | `{}` | Queue follow-up message |\n\
             | `{}` | Restore queued messages |\n\
             | `{}` | Paste image or text from clipboard |\n\
             | `/` | Slash commands |\n\
             | `!` | Run bash command |\n\
             | `!!` | Run bash command (excluded from context) |",
            key("tui.editor.cursorUp"),
            key("tui.editor.cursorDown"),
            key("tui.editor.cursorLeft"),
            key("tui.editor.cursorRight"),
            key("tui.editor.cursorWordLeft"),
            key("tui.editor.cursorWordRight"),
            key("tui.editor.cursorLineStart"),
            key("tui.editor.cursorLineEnd"),
            key("tui.editor.jumpForward"),
            key("tui.editor.jumpBackward"),
            key("tui.editor.pageUp"),
            key("tui.editor.pageDown"),
            key("tui.input.submit"),
            key("tui.input.newLine"),
            if cfg!(target_os = "windows") {
                " (Ctrl+Enter on Windows Terminal)"
            } else {
                ""
            },
            key("tui.editor.deleteWordBackward"),
            key("tui.editor.deleteWordForward"),
            key("tui.editor.deleteToLineStart"),
            key("tui.editor.deleteToLineEnd"),
            key("tui.editor.yank"),
            key("tui.editor.yankPop"),
            key("tui.editor.undo"),
            key("tui.input.tab"),
            key("app.interrupt"),
            key("app.clear"),
            key("app.exit"),
            key("app.suspend"),
            key("app.thinking.cycle"),
            key("app.model.cycleForward"),
            key("app.model.cycleBackward"),
            key("app.model.select"),
            key("app.tools.expand"),
            key("app.thinking.toggle"),
            key("app.editor.external"),
            key("app.message.copy"),
            key("app.message.followUp"),
            key("app.message.dequeue"),
            key("app.clipboard.pasteImage"),
        );
        self.add_bordered_block("Keyboard Shortcuts", &hotkeys, 1);
    }

    /// The `DynamicBorder`/title/`Markdown`/`DynamicBorder` block `/changelog`
    /// and `/hotkeys` both draw.
    fn add_bordered_block(&mut self, title: &str, markdown: &str, markdown_padding_y: usize) {
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(DynamicBorder::new(None)));
        chat.add_child(component_ref(Text::new(
            theme().bold(&theme().fg(ThemeColor::Accent, title)),
            1,
            0,
        )));
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(Markdown::new(
            markdown,
            1,
            markdown_padding_y,
            get_markdown_theme(),
            None,
            None,
        )));
        chat.add_child(component_ref(DynamicBorder::new(None)));
        drop(chat);
        self.ui.request_render();
    }

    async fn handle_clear_command(&mut self) {
        self.clear_status_indicator(None);
        match self.runtime.new_session(None).await {
            Ok(()) => {
                self.rebind_current_session();
                let mut chat = self.chat_container.borrow_mut();
                chat.add_child(component_ref(Spacer::new(1)));
                chat.add_child(component_ref(Text::new(
                    theme().fg(ThemeColor::Accent, "✓ New session started"),
                    1,
                    1,
                )));
                drop(chat);
                self.ui.request_render();
            }
            Err(message) => self.show_error(&format!("Failed to create session: {message}")),
        }
    }

    fn handle_compact_command(&mut self, custom_instructions: Option<String>) {
        // The session reports start and completion through its event channel.
        // Keeping the work in the main select lets those events render while
        // the provider is still producing the summary.
        self.pending_compactions.push_back(custom_instructions);
    }

    /// `/btw <question>` — opens the side-question panel and asks the first
    /// question.
    /// A second invocation replaces the open panel rather than stacking one:
    /// two side channels would compete for the same editor and the same
    /// attention.
    fn handle_side_question_command(&mut self, question: Option<&str>) {
        // The question is optional: without one the panel opens empty and waits,
        // which is the same thing a follow-up does once it is open.
        let question = question.map(str::trim).filter(|text| !text.is_empty());
        // The open one goes first, and goes even when the new one cannot be
        // started: a panel left standing over a child that is gone answers
        // nothing and invites a follow-up that cannot arrive.
        self.close_side_question_panel();
        if let Err(error) = self.session().start_side_question() {
            self.show_error(&error);
            return;
        }

        let markdown_theme = get_markdown_theme();
        let editor = Rc::clone(&self.editor);
        let ui = self.ui.clone();
        let panel = Rc::new(RefCell::new(SideQuestionPanel::new(
            SideQuestionPanelOptions {
                markdown_theme,
                // Not while the user is typing: the caret needs the arrows more
                // than the panel does.
                can_use_scroll_keys: Rc::new(move || {
                    editor.borrow().editor().get_text().is_empty()
                }),
                terminal_rows: Rc::new(move || ui.with_terminal(|terminal| terminal.rows())),
            },
        )));
        {
            let mut container = self.widget_container_above.borrow_mut();
            container.clear();
            container.add_child(component_ref(Spacer::new(1)));
            container.add_child(Rc::clone(&panel) as ComponentRef);
        }
        self.side_question_panel = Some(panel);
        self.ui.request_render();
        if let Some(question) = question {
            self.ask_side_question(question);
        }
    }

    /// Puts a question to the open child and streams the answer into the panel.
    fn ask_side_question(&mut self, question: &str) {
        let Some(panel) = self.side_question_panel.clone() else {
            return;
        };
        if !panel.borrow_mut().submit(question) {
            return;
        }
        let Some(question) = panel.borrow_mut().take_pending_prompt() else {
            return;
        };
        let session = self.session();
        let tx = self.ui_tx.clone();
        self.side_futures.push(Box::pin(async move {
            let sink = tx.clone();
            let outcome = session
                .ask_side_question(
                    &question,
                    Arc::new(move |event| {
                        let _ = sink.send(UiMessage::SideQuestionEvent {
                            event: Box::new(event),
                        });
                    }),
                )
                .await;
            UiMessage::SideQuestionFinished {
                error: outcome.err(),
            }
        }));
    }

    /// The answer text and the reasoning text of an assistant message, each
    /// joined from the blocks that carry it.
    fn side_question_texts(message: &notagent_ai::types::AssistantMessage) -> (String, String) {
        let mut answer = String::new();
        let mut thinking = String::new();
        for block in &message.content {
            match block {
                notagent_ai::types::AssistantContent::Text(text) => answer.push_str(&text.text),
                notagent_ai::types::AssistantContent::Thinking(block) => {
                    thinking.push_str(&block.thinking)
                }
                _ => {}
            }
        }
        (answer, thinking)
    }

    /// The child's events, as the panel reads them.
    fn handle_side_question_event(&mut self, event: AgentEvent) {
        let Some(panel) = self.side_question_panel.clone() else {
            return;
        };
        match event {
            AgentEvent::MessageUpdate { message, .. } => {
                let AgentMessage::Assistant(message) = message else {
                    return;
                };
                // The panel keeps the whole text rather than deltas, so the
                // update replaces what is there instead of appending to it.
                let (text, thinking) = Self::side_question_texts(&message);
                let mut panel = panel.borrow_mut();
                panel.set_answer(&text);
                panel.set_thinking(&thinking);
            }
            AgentEvent::MessageEnd { message } => {
                if let AgentMessage::Assistant(message) = message {
                    let (text, thinking) = Self::side_question_texts(&message);
                    let mut panel = panel.borrow_mut();
                    panel.set_answer(&text);
                    panel.set_thinking(&thinking);
                }
            }
            _ => return,
        }
        self.ui.request_render();
    }

    /// The run settled, one way or the other.
    fn handle_side_question_finished(&mut self, error: Option<String>) {
        let Some(panel) = self.side_question_panel.clone() else {
            return;
        };
        match error {
            Some(error) => panel.borrow_mut().mark_failed(error),
            None => panel.borrow_mut().mark_done(None),
        }
        self.ui.request_render();
    }

    /// Stops an answer that is still coming, leaving the panel and everything
    /// already in it standing. Returns whether there was one.
    fn stop_side_question_answer(&mut self) -> bool {
        let Some(panel) = self.side_question_panel.clone() else {
            return false;
        };
        if !panel.borrow().is_running() {
            return false;
        }
        // The child survives, so a follow-up still reaches the same
        // conversation; only the run in flight ends.
        self.session().stop_side_question_answer();
        panel.borrow_mut().mark_failed("Stopped.");
        self.ui.request_render();
        true
    }

    /// Takes the panel down, and the child with it — nothing more will be asked
    /// of a child whose panel is gone.
    fn close_side_question_panel(&mut self) -> bool {
        if self.side_question_panel.take().is_none() {
            return false;
        }
        self.session().cancel_side_question();
        self.widget_container_above.borrow_mut().clear();
        self.ui.request_render();
        true
    }

    /// Whether the editor's text belongs to the panel rather than to the main
    /// agent.
    fn side_question_takes_input(&self) -> bool {
        self.side_question_panel.is_some()
    }

    /// A submitted line while the panel is open.
    fn submit_to_side_question(&mut self, text: &str) {
        let Some(panel) = self.side_question_panel.clone() else {
            return;
        };
        if panel.borrow().is_running() {
            // Not queued: two questions in flight at once is a state the panel
            // cannot show and the child cannot answer.
            panel
                .borrow_mut()
                .add_transient_notice("Wait for the answer before asking again.");
            self.editor.borrow_mut().editor_mut().set_text(text);
            self.ui.request_render();
            return;
        }
        self.clear_editor_text();
        self.ask_side_question(text);
    }

    /// Starts the `/init` child and lets the loop carry on.
    /// Awaiting the run here would freeze the UI for as long as the child
    /// explores, which is exactly when the user wants to watch the subagent
    /// panel — so the run goes into the side futures and reports back.
    async fn handle_init_command(&mut self) {
        if self.init_running {
            self.show_warning("An init run is already going.");
            return;
        }
        if self.session().is_streaming() {
            self.show_warning("Wait for the current response to finish before running init.");
            return;
        }
        self.init_running = true;
        self.show_status("Exploring the project to write AGENTS.md...");
        let session = self.session();
        self.side_futures.push(Box::pin(async move {
            UiMessage::InitFinished {
                error: session.run_init().await.err(),
            }
        }));
    }

    /// Remaining: `showLoadedResources`, which belongs to the resources slice,
    /// and `maybeSaveImplicitProjectTrustAfterReload`, which needs the trust
    /// selector of the selector slice.
    async fn handle_reload_command(&mut self) {
        if self.session().is_streaming() {
            self.show_warning("Wait for the current response to finish before reloading.");
            return;
        }
        if self.session().is_compacting() {
            self.show_warning("Wait for compaction to finish before reloading.");
            return;
        }

        let reload_box = Rc::new(RefCell::new(Container::new()));
        {
            let mut container = reload_box.borrow_mut();
            container.add_child(component_ref(DynamicBorder::new(Some(Rc::new(
                |text: &str| theme().fg(ThemeColor::Border, text),
            )))));
            container.add_child(component_ref(Spacer::new(1)));
            container.add_child(component_ref(Text::new(
                theme().fg(
                    ThemeColor::Muted,
                    "Reloading keybindings, extensions, skills, prompts, themes, and context files...",
                ),
                1,
                0,
            )));
            container.add_child(component_ref(Spacer::new(1)));
            container.add_child(component_ref(DynamicBorder::new(Some(Rc::new(
                |text: &str| theme().fg(ThemeColor::Border, text),
            )))));
        }
        {
            let mut editor_container = self.editor_container.borrow_mut();
            editor_container.clear();
            editor_container.add_child(Rc::clone(&reload_box) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&reload_box) as ComponentRef));
        self.cell.request_render(true);

        self.session().reload().await;
        self.hide_thinking_block = self.settings().get_hide_thinking_block();
        self.output_pad = self.settings().get_output_pad() as usize;
        self.rebuild_chat_from_messages();
        self.keybindings.borrow_mut().reload();
        self.setup_autocomplete_provider();
        set_keybindings(self.keybindings.borrow().to_tui());
        let _ = set_registered_themes(self.session().resource_loader().get_themes().0);
        self.theme_controller.apply_from_settings().await;
        self.apply_runtime_settings();
        self.show_loaded_resources(false, true);
        let saved_implicit_project_trust = self.maybe_save_implicit_project_trust_after_reload();
        if let Some(error) = self.runtime.services().model_runtime.get_error() {
            self.show_error(&format!("models.json error: {error}"));
        }
        self.show_status(if saved_implicit_project_trust {
            "Reloaded keybindings, extensions, skills, prompts, themes, and context files; saved project trust"
        } else {
            "Reloaded keybindings, extensions, skills, prompts, themes, and context files"
        });

        {
            let mut editor_container = self.editor_container.borrow_mut();
            editor_container.clear();
            editor_container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.ui.request_render();
    }

    fn apply_runtime_settings(&mut self) {
        let settings = self.settings();
        self.ui.set_clear_on_shrink(settings.get_clear_on_shrink());
        self.ui
            .set_show_hardware_cursor(settings.get_show_hardware_cursor());
        {
            let mut editor = self.editor.borrow_mut();
            editor.set_padding_x(settings.get_editor_padding_x() as usize);
            let editor = editor.editor_mut();
            editor.set_autocomplete_max_visible(settings.get_autocomplete_max_visible() as usize);
        }
        {
            let mut footer = self.footer.borrow_mut();
            footer.set_session(Arc::clone(&self.session())
                as Arc<dyn crate::modes::interactive::components::footer::FooterSession>);
        }
        self.footer_data.set_cwd(&self.cwd());
        self.hide_thinking_block = settings.get_hide_thinking_block();
        self.output_pad = settings.get_output_pad() as usize;
        self.update_editor_border_color();
    }

    fn rebuild_chat_from_messages(&mut self) {
        self.chat_container.borrow_mut().clear();
        // The transcript is the durable record the user resumes. Compaction
        // only changes the context sent to the model, never this projection.
        let entries = self.session().with_session_manager(|manager| {
            manager
                .get_branch(None)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        });
        self.render_session_entries(&entries, false);
        self.restore_compaction_chat_indicator();
    }

    /// because the renderer keeps that pass private to the frame it writes.
    fn handle_debug_command(&mut self) {
        let width = self.ui.columns();
        let height = self.ui.rows();
        let lines: Vec<Line> = self
            .ui
            .children()
            .iter()
            .flat_map(|child| child.borrow_mut().render(width))
            .collect();

        let debug_log_path = crate::config::get_debug_log_path();
        let mut debug_data = vec![
            format!("Debug output at {}", chrono::Utc::now().to_rfc3339()),
            format!("Terminal: {width}x{height}"),
            format!("Total lines: {}", lines.len()),
            String::new(),
            "=== All rendered lines with visible widths ===".to_owned(),
        ];
        for (index, line) in lines.iter().enumerate() {
            debug_data.push(format!(
                "[{index}] (w={}) {}",
                notagent_tui::utils::visible_width(line),
                json_string(line)
            ));
        }
        debug_data.push(String::new());
        debug_data.push("=== Agent messages (JSONL) ===".to_owned());
        for message in self.session().messages() {
            debug_data.push(serde_json::to_string(&message).unwrap_or_default());
        }
        debug_data.push(String::new());

        if let Some(parent) = debug_log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&debug_log_path, debug_data.join("\n"));

        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(Text::new(
            format!(
                "{}\n{}",
                theme().fg(ThemeColor::Accent, "✓ Debug log written"),
                theme().fg(ThemeColor::Muted, &debug_log_path.to_string_lossy())
            ),
            1,
            1,
        )));
        drop(chat);
        self.ui.request_render();
    }

    fn handle_armin_says_hi(&mut self) {
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(ArminComponent::new()));
        drop(chat);
        self.ui.request_render();
    }

    fn handle_demented_delves(&mut self) {
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(EarendilAnnouncementComponent::new()));
        drop(chat);
        self.ui.request_render();
    }

    /// over the list selector `showExtensionSelector` uses.
    /// Deviation (class 1): the dialog blocks this loop until it answers, so the
    /// events of a run that is still going are handled after it closes. The
    /// caller's render loop keeps drawing and keeps feeding the dialog, which is
    /// what the dialog itself needs.
    async fn confirm(&mut self, title: &str, message: &str) -> bool {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Option<String>>();
        let done_tx = Rc::new(RefCell::new(Some(done_tx)));
        let select_tx = Rc::clone(&done_tx);
        let cancel_tx = Rc::clone(&done_tx);
        let selector = component_ref(ListSelectorComponent::new(
            format!("{title}\n{message}"),
            vec!["Yes".to_owned(), "No".to_owned()],
            Box::new(move |option| {
                if let Some(sender) = select_tx.borrow_mut().take() {
                    let _ = sender.send(Some(option));
                }
            }),
            Box::new(move || {
                if let Some(sender) = cancel_tx.borrow_mut().take() {
                    let _ = sender.send(None);
                }
            }),
            None,
        ));
        {
            let mut editor_container = self.editor_container.borrow_mut();
            editor_container.clear();
            editor_container.add_child(Rc::clone(&selector));
        }
        self.ui.set_focus(Some(Rc::clone(&selector)));
        self.ui.request_render();

        let answer = done_rx.await.unwrap_or(None);

        {
            let mut editor_container = self.editor_container.borrow_mut();
            editor_container.clear();
            editor_container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.ui.request_render();
        answer.as_deref() == Some("Yes")
    }

    fn switch_tui_mode(&mut self, mode: TuiMode) {
        if !self.cell.switch(mode, get_agent_dir()) {
            self.show_status("Close active overlays before changing TUI mode");
            return;
        }
        self.ui = self.cell.core();
        self.cell
            .set_layout_root(self.fullscreen_layout_root.clone());
        self.options.tui_mode = Some(mode);
        self.settings().set_tui_mode(mode);
        self.cell.start();
        self.theme_controller.rebind_tui();
        if self.settings().get_show_terminal_progress()
            && (self.session().is_streaming() || self.session().is_compacting())
        {
            self.ui
                .with_terminal(|terminal| terminal.set_progress(true));
        }
        if self.active_status_indicator.is_none() {
            self.status_container.borrow_mut().clear();
        }
        self.show_status(&format!(
            "TUI mode: {}",
            match mode {
                TuiMode::Regular => "regular",
                TuiMode::Fullscreen => "fullscreen",
            }
        ));
    }

    /// the TUI from a `SIGCONT` handler; in Rust the signal stops the thread
    /// inside `kill`, so the code after it *is* the resume path.
    fn handle_ctrl_z(&mut self) {
        #[cfg(windows)]
        {
            self.show_status("Suspend to background is not supported on Windows");
        }
        #[cfg(unix)]
        {
            self.cell.stop(TuiStopOptions::default());
            unsafe {
                libc::kill(0, libc::SIGTSTP);
            }
            self.cell.start();
            self.cell.request_render(true);
        }
    }

    /// An image is inserted as an `[Image #n]` marker and its bytes are held
    /// until submit, when they ride along as an image part. The path used to go
    /// into the editor instead — sixty characters of temporary directory the
    /// user had to type around and could read nothing from.
    /// No temporary file is written any more. It only ever existed to give the
    /// editor a path to show, and nothing read it back — the bytes now travel
    /// with the message instead.
    fn handle_clipboard_paste(&mut self) {
        if let Some(image) = crate::utils::clipboard_image::read_clipboard_image() {
            let marker = self.register_pasted_image(image);
            self.editor
                .borrow_mut()
                .editor_mut()
                .insert_text_at_cursor(&marker);
            self.ui.request_render();
            return;
        }
        if let Some(text) = crate::utils::clipboard::read_clipboard_text() {
            self.editor
                .borrow_mut()
                .editor_mut()
                .insert_text_at_cursor(&text);
            self.ui.request_render();
        }
    }

    /// Records a pasted image and returns the marker to insert for it.
    fn register_pasted_image(
        &mut self,
        image: crate::utils::clipboard_image::ClipboardImage,
    ) -> String {
        let id = self.next_image_paste_id;
        self.next_image_paste_id = id.saturating_add(1);
        self.pasted_images.insert(
            id,
            ImageContent {
                data: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &image.bytes,
                ),
                mime_type: image.mime_type,
            },
        );
        self.sync_image_markers();
        format_image_marker(id)
    }

    /// Tells the editor which markers to treat as single units, so the cursor
    /// steps over one and a backspace removes the whole thing. Half a marker
    /// resolves to nothing, and a marker the user can only delete by taking it
    /// apart is a marker they will leave broken in the text.
    fn sync_image_markers(&self) {
        let markers = self
            .pasted_images
            .keys()
            .map(|id| format_image_marker(*id))
            .collect();
        self.editor
            .borrow_mut()
            .editor_mut()
            .set_atomic_markers(markers);
    }

    /// The images a submitted line refers to, in order and without repeats.
    /// The marker stays in the text. It is what the model reads as "the image
    /// you were given here", which is the only thing that tells two attachments
    /// apart when a message carries several.
    fn images_for_text(&self, text: &str) -> Vec<ImageContent> {
        let mut seen = std::collections::HashSet::new();
        let mut images = Vec::new();
        for id in parse_image_markers(text) {
            if !seen.insert(id) {
                continue;
            }
            if let Some(image) = self.pasted_images.get(&id) {
                images.push(image.clone());
            }
        }
        images
    }

    /// Drops the pasted images once their prompt is on its way.
    fn clear_pasted_images(&mut self) {
        if self.pasted_images.is_empty() {
            return;
        }
        self.pasted_images.clear();
        self.next_image_paste_id = 1;
        self.sync_image_markers();
    }

    async fn handle_ctrl_c(&mut self) {
        // The panel stacks above the transcript, so the key reaches it first —
        // in two steps, like everything else here: stop the answer that is
        // still coming, and only on a second press take the panel away.
        if self.stop_side_question_answer() || self.close_side_question_panel() {
            self.last_sigint_time = None;
            return;
        }
        let now = Instant::now();
        if self
            .last_sigint_time
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(500))
        {
            self.shutdown().await;
        } else {
            self.clear_editor();
            self.last_sigint_time = Some(now);
        }
    }

    /// empty editor, which `CustomEditor` enforces.
    async fn handle_ctrl_d(&mut self) {
        self.shutdown().await;
    }

    /// The double-escape action (`/tree` or `/fork`) needs the selectors and
    /// arrives with their slice.
    fn handle_escape(&mut self) {
        // The panel stacks above everything else and is what the key most
        // obviously points at. Closing it must not also stop the main turn —
        // the two are unrelated, and a user dismissing a side question is not
        // asking the agent to stop working.
        if self.close_side_question_panel() {
            return;
        }
        match self.escape_target {
            // While a compaction or a retry runs, Escape aborts that instead.
            EscapeTarget::Compaction => {
                self.session().abort_compaction();
                return;
            }
            EscapeTarget::Retry => {
                self.restore_queued_messages_to_editor(true);
                return;
            }
            EscapeTarget::BranchSummary => {
                self.session().abort_branch_summary();
                return;
            }
            EscapeTarget::Default => {}
        }
        // Everything running stops, not whichever the first branch happened to
        // name. A `!` command and an agent turn can be going at once, and a key
        // that means "stop" leaving one of them running is a key the user has
        // to press again with no way to know why the first press did nothing.
        let streaming = self.session().is_streaming();
        let bash_running = self.session().is_bash_running();
        if bash_running {
            self.session().abort_bash();
        }
        if streaming {
            self.restore_queued_messages_to_editor(true);
        }
        if streaming || bash_running {
            return;
        }
        if self.is_bash_mode {
            self.clear_editor_text();
            self.is_bash_mode = false;
            self.update_editor_border_color();
        } else if self.editor.borrow().editor().get_text().trim().is_empty() {
            self.handle_double_escape();
        }
    }

    /// The session teardown runs before the terminal is touched: removing
    /// sockets and writing the session file must not be skipped because a
    /// restore write to a dead terminal failed. No resume hint is printed —
    async fn shutdown_from_signal(&mut self) {
        if self.is_shutting_down {
            return;
        }
        self.is_shutting_down = true;
        self.runtime.dispose().await;
        self.theme_controller.disable_auto_sync();
        self.stop();
        self.exit_code = Some(0);
    }

    /// Deviation (class 1): instead of `process.exit(0)` the mode records the
    /// exit code and lets [`Self::run`] return it, because the render loop
    /// belongs to the caller and has to unwind.
    async fn shutdown(&mut self) {
        if self.is_shutting_down {
            return;
        }
        self.is_shutting_down = true;
        self.theme_controller.disable_auto_sync();
        self.stop();
        self.runtime.dispose().await;
        if let Some(command) = self.format_resume_command() {
            println!(
                "{} {command}",
                theme().fg(ThemeColor::Dim, "To resume this session:")
            );
        }
        self.exit_code = Some(0);
    }

    fn format_resume_command(&self) -> Option<String> {
        if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            return None;
        }
        let session = self.session();
        session.with_session_manager(|manager| {
            if !manager.is_persisted() {
                return None;
            }
            let file = manager.get_session_file()?;
            if !std::path::Path::new(&file).exists() {
                return None;
            }
            let mut args = vec![APP_NAME.to_owned()];
            if !manager.uses_default_session_dir() {
                args.push("--session-dir".to_owned());
                args.push(quote_if_needed(manager.get_session_dir()));
            }
            args.push("--session".to_owned());
            args.push(manager.get_session_id().to_owned());
            Some(args.join(" "))
        })
    }

    pub fn stop(&mut self) {
        self.dispose_active_selector();
        if self.settings().get_show_terminal_progress() {
            self.ui
                .with_terminal(|terminal| terminal.set_progress(false));
        }
        self.clear_status_indicator(None);
        self.theme_controller.disable_auto_sync();
        self.footer.borrow_mut().dispose();
        self.agent_subscription = None;
        if self.is_initialized {
            // `stopInteractiveTui(fullscreenExitOutput)`: leaving fullscreen
            // with `transcript` writes the document into the scrollback by
            // switching back to the main screen for one last frame.
            if self.cell.mode() == TuiMode::Fullscreen
                && self.settings().get_fullscreen_exit_output() == FullscreenExitOutput::Transcript
            {
                while self.ui.has_overlay_entries() {
                    self.ui.hide_overlay();
                }
                if self.cell.switch(TuiMode::Regular, get_agent_dir()) {
                    self.ui = self.cell.core();
                    self.cell.render_now(false);
                }
            }
            self.cell.stop(TuiStopOptions {
                preserve_screen: self.cell.mode() == TuiMode::Fullscreen,
            });
            self.is_initialized = false;
        }
    }

    // ------------------------------------------------------------------
    // Session events
    // ------------------------------------------------------------------

    async fn handle_event(&mut self, event: AgentSessionEvent) {
        self.footer.borrow_mut().invalidate();

        match event {
            AgentSessionEvent::Agent(AgentEvent::AgentStart) => {
                self.pending_tools.clear();
                // The retry's success event arrives later, but Escape has to
                // abort the run again from here on.
                if self.escape_target == EscapeTarget::Retry {
                    self.escape_target = EscapeTarget::Default;
                }
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(true));
                }
                if self.working_visible {
                    let message = self
                        .working_message
                        .clone()
                        .unwrap_or_else(|| DEFAULT_WORKING_MESSAGE.to_owned());
                    self.show_status_indicator(StatusIndicator::working(message, None));
                } else {
                    self.clear_status_indicator(None);
                }
                self.ui.request_render();
            }
            AgentSessionEvent::Agent(AgentEvent::MessageStart { message }) => match message {
                AgentMessage::Assistant(message) => {
                    let component = Rc::new(RefCell::new(AssistantMessageComponent::new(
                        None,
                        self.hide_thinking_block,
                        Some(get_markdown_theme()),
                        Some(self.hidden_thinking_label.clone()),
                        Some(self.output_pad),
                        Vec::new(),
                    )));
                    self.chat_container
                        .borrow_mut()
                        .add_child(Rc::clone(&component) as ComponentRef);
                    component
                        .borrow_mut()
                        .update_content(message.clone(), Some(true));
                    component
                        .borrow_mut()
                        .set_expanded(self.tool_output_expanded);
                    self.chat_expandables
                        .push(Rc::clone(&component) as Rc<RefCell<dyn Expandable>>);
                    self.streaming_component = Some(component);
                    // Providers may deliver the first visible content with
                    // MessageStart rather than a later delta.
                    self.streaming_visible_chars = streamed_visible_chars(&message);
                    if self.streaming_visible_chars > 0 {
                        self.close_explore_block();
                    }
                    self.streaming_message = Some(message);
                    self.ui.request_render();
                }
                message => {
                    self.add_message_to_chat(&message, false);
                    self.ui.request_render();
                }
            },
            AgentSessionEvent::Agent(AgentEvent::MessageUpdate { message, .. }) => {
                if let (Some(component), AgentMessage::Assistant(message)) =
                    (self.streaming_component.clone(), message)
                {
                    component
                        .borrow_mut()
                        .update_content(message.clone(), Some(true));
                    // Settle the preceding block before any subsequent calls
                    // open their own exploration below this text or thought.
                    let visible = streamed_visible_chars(&message);
                    if visible > self.streaming_visible_chars {
                        self.close_explore_block();
                    }
                    self.streaming_visible_chars = visible;
                    for content in message.content.iter() {
                        if let notagent_ai::types::AssistantContent::ToolCall(call) = content {
                            let args = serde_json::Value::Object(call.arguments.clone());
                            // A streamed bash call does not reveal whether it
                            // is background work until its later arguments
                            // arrive. Wait for ToolExecutionStart, where the
                            // complete arguments let us omit background bash
                            // without ever painting a transient tool row.
                            if call.name == "bash" {
                                self.remove_tool_component(&call.id);
                                continue;
                            }
                            match self.tool_component(&call.id) {
                                Some(component) => component.borrow_mut().update_args(args),
                                None => self.add_tool_component(&call.name, &call.id, args),
                            }
                        }
                    }
                    self.streaming_message = Some(message);
                    self.ui.request_render();
                }
            }
            AgentSessionEvent::Agent(AgentEvent::MessageEnd { message }) => {
                if let AgentMessage::Assistant(mut message) = message
                    && let Some(component) = self.streaming_component.clone()
                {
                    let mut error_message: Option<String> = None;
                    if message.stop_reason == StopReason::Aborted {
                        let attempt = self.session().retry_attempt();
                        let text = if attempt > 0 {
                            format!(
                                "Aborted after {attempt} retry attempt{}",
                                if attempt > 1 { "s" } else { "" }
                            )
                        } else {
                            "Operation aborted".to_owned()
                        };
                        message.error_message = Some(text.clone());
                        error_message = Some(text);
                    }
                    component
                        .borrow_mut()
                        .update_content(message.clone(), Some(false));

                    if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                        let error_message = error_message.unwrap_or_else(|| {
                            message
                                .error_message
                                .clone()
                                .unwrap_or_else(|| "Error".to_owned())
                        });
                        for (_, component) in std::mem::take(&mut self.pending_tools) {
                            component
                                .borrow_mut()
                                .update_result(error_result(&error_message), false);
                        }
                    } else {
                        // Args are complete: the edit tools compute their diff.
                        for (_, component) in self.pending_tools.iter() {
                            component.borrow_mut().set_args_complete();
                        }
                        self.maybe_show_cache_miss_notice(&message);
                    }
                    // An aborted or failed turn settles the block red: the
                    // exploration was cut off, so it must not read as one that
                    // went fine. Visible assistant text ends the run too —
                    // unless this very message announced the exploration its
                    // text introduces.
                    if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                        self.abort_explore_block();
                    } else if assistant_message_ends_search_run(&message) {
                        self.close_explore_block();
                    }
                    self.streaming_component = None;
                    self.streaming_message = None;
                    self.streaming_visible_chars = 0;
                    self.footer.borrow_mut().invalidate();
                }
                self.ui.request_render();
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            }) => {
                if is_background_bash_call(&tool_name, &args) {
                    self.remove_tool_component(&tool_call_id);
                    self.ui.request_render();
                    return;
                }
                if self.tool_component(&tool_call_id).is_none() {
                    self.add_tool_component(&tool_name, &tool_call_id, args);
                }
                if let Some(component) = self.tool_component(&tool_call_id) {
                    component.borrow_mut().mark_execution_started();
                }
                self.ui.request_render();
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            }) => {
                if let Some(component) = self.tool_component(&tool_call_id) {
                    component
                        .borrow_mut()
                        .update_result(tool_result(partial_result, false), true);
                    self.ui.request_render();
                }
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            }) => {
                let todo_details = (tool_name == "todo_write")
                    .then(|| result.details.clone())
                    .flatten();
                if let Some(component) = self.tool_component(&tool_call_id) {
                    component
                        .borrow_mut()
                        .update_result(tool_result(result, is_error), false);
                    self.pending_tools.retain(|(id, _)| id != &tool_call_id);
                    self.ui.request_render();
                } else if is_explore_tool(&tool_name) {
                    // Search calls live in a block, not in a tool row; the
                    // block may already be closed, so route by call id.
                    for block in self.chat_explore_blocks.iter().rev() {
                        if block.borrow().has_call(&tool_call_id) {
                            block.borrow_mut().complete_call(&tool_call_id, is_error);
                            self.ui.request_render();
                            break;
                        }
                    }
                }
                // Fed from the result rather than from the store: a list whose
                // items are all completed is gone from the store by now.
                if tool_name == "todo_write" {
                    let todos = todo_details
                        .as_ref()
                        .and_then(|details| details.get("after"))
                        .and_then(|after| serde_json::from_value::<Vec<Todo>>(after.clone()).ok())
                        .unwrap_or_default();
                    self.sync_todo_panel(todos, true);
                    self.ui.request_render();
                }
            }
            AgentSessionEvent::AgentEnd { .. } => {
                // The run is over, so the exploration is too — the reference
                // closes on `TaskComplete` for the same reason. Without this a
                // later turn would hang its first calls on the stale block.
                self.close_explore_block();
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(false));
                }
                self.settle_status_indicator(StatusIndicatorKind::Working);
                if let Some(component) = self.streaming_component.take() {
                    self.chat_container
                        .borrow_mut()
                        .remove_child(&(component as ComponentRef));
                    self.streaming_message = None;
                }
                self.pending_tools.clear();
                self.ui.request_render();
            }
            AgentSessionEvent::QueueUpdate { .. } => {
                self.update_pending_messages_display();
                self.ui.request_render();
            }
            AgentSessionEvent::CompactionStart { reason } => {
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(true));
                }
                // The editor stays live; submissions are queued while it runs.
                self.escape_target = EscapeTarget::Compaction;
                self.show_compaction_chat_indicator(compaction_status_reason(reason));
                self.ui.request_render();
            }
            AgentSessionEvent::CompactionEnd {
                reason,
                result,
                aborted,
                will_retry,
                error_message,
            } => {
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(false));
                }
                if self.escape_target == EscapeTarget::Compaction {
                    self.escape_target = EscapeTarget::Default;
                }
                self.clear_compaction_chat_indicator();
                if aborted {
                    if reason == CompactionReason::Manual {
                        self.show_error("Compaction cancelled");
                    } else {
                        self.show_status("Auto-compaction cancelled");
                    }
                } else if let Some(result) = result {
                    self.add_message_to_chat(
                        &AgentMessage::CompactionSummary(create_compaction_summary_message(
                            &result.summary,
                            result.tokens_before,
                            result.estimated_tokens_after,
                            now_millis(),
                        )),
                        false,
                    );
                    self.footer.borrow_mut().invalidate();
                } else if let Some(error_message) = error_message {
                    if reason == CompactionReason::Manual {
                        self.show_error(&error_message);
                    } else {
                        let mut chat = self.chat_container.borrow_mut();
                        chat.add_child(component_ref(Spacer::new(1)));
                        chat.add_child(component_ref(Text::new(
                            theme().fg(ThemeColor::Error, &error_message),
                            1,
                            0,
                        )));
                    }
                }
                self.flush_compaction_queue(will_retry).await;
                self.ui.request_render();
            }
            AgentSessionEvent::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                ..
            } => {
                self.escape_target = EscapeTarget::Retry;
                self.show_status_indicator(StatusIndicator::retry(
                    attempt as u32,
                    max_attempts as u32,
                    delay_ms,
                ));
                self.ui.request_render();
            }
            AgentSessionEvent::AutoRetryEnd {
                attempt,
                success,
                final_error,
                ..
            } => {
                if self.escape_target == EscapeTarget::Retry {
                    self.escape_target = EscapeTarget::Default;
                }
                self.clear_status_indicator(Some(StatusIndicatorKind::Retry));
                if !success {
                    self.show_error(&format!(
                        "Retry failed after {attempt} attempts: {}",
                        final_error.unwrap_or_else(|| "Unknown error".to_owned())
                    ));
                }
                self.ui.request_render();
            }
            AgentSessionEvent::PersistenceError { error_message } => {
                self.show_error(&error_message);
                self.ui.request_render();
            }
            AgentSessionEvent::SessionInfoChanged { .. } => {
                self.update_terminal_title();
                self.footer.borrow_mut().invalidate();
                self.ui.request_render();
            }
            AgentSessionEvent::ThinkingLevelChanged { .. } => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
            }
            AgentSessionEvent::EntryAppended { entry } => {
                if let Some(record) = TaskLifecycleRecord::from_session_entry(&entry) {
                    self.append_task_lifecycle(&record);
                }
            }
            // Other session bookkeeping arrives with the slices that own it.
            _ => {}
        }
    }

    fn tool_component(&self, tool_call_id: &str) -> Option<Rc<RefCell<ToolExecutionComponent>>> {
        self.pending_tools
            .iter()
            .find(|(id, _)| id == tool_call_id)
            .map(|(_, component)| Rc::clone(component))
    }

    fn remove_tool_component(&mut self, tool_call_id: &str) {
        let Some(index) = self
            .pending_tools
            .iter()
            .position(|(id, _)| id == tool_call_id)
        else {
            return;
        };
        let (_, component) = self.pending_tools.remove(index);
        let child = Rc::clone(&component) as ComponentRef;
        self.chat_container.borrow_mut().remove_child(&child);
        self.chat_tool_rows
            .retain(|candidate| !Rc::ptr_eq(candidate, &component));
        let expandable = component as Rc<RefCell<dyn Expandable>>;
        self.chat_expandables
            .retain(|candidate| !Rc::ptr_eq(candidate, &expandable));
    }

    fn add_tool_component(&mut self, tool_name: &str, tool_call_id: &str, args: serde_json::Value) {
        // `todo_write` already has the dock panel. A background bash and every
        // `task` call are represented by immutable lifecycle rows; keeping the
        // ordinary tool component would stream a second copy into the
        // transcript (subagents: user decision 2026-08-31).
        if tool_name == "todo_write"
            || tool_name == "task"
            || is_background_bash_call(tool_name, &args)
        {
            return;
        }
        if is_explore_tool(tool_name) {
            // Streaming repeats earlier calls even after another tool closes
            // their block. Keep each call in its original block so its result
            // cannot leave a duplicate permanently pending.
            let existing = self
                .chat_explore_blocks
                .iter()
                .rev()
                .find(|block| block.borrow().has_call(tool_call_id))
                .cloned();
            let block = existing.unwrap_or_else(|| self.open_explore_block(false));
            block
                .borrow_mut()
                .push_call(tool_name, tool_call_id.to_owned(), &args);
            return;
        }
        // Any other tool row ends the run of searches.
        self.close_explore_block();
        let component = Rc::new(RefCell::new(self.create_tool_component(
            tool_name,
            tool_call_id,
            args,
        )));
        component
            .borrow_mut()
            .set_expanded(self.tool_output_expanded);
        self.chat_container
            .borrow_mut()
            .add_child(Rc::clone(&component) as ComponentRef);
        self.chat_tool_rows.push(Rc::clone(&component));
        self.chat_expandables
            .push(Rc::clone(&component) as Rc<RefCell<dyn Expandable>>);
        self.pending_tools
            .push((tool_call_id.to_owned(), component));
    }

    /// Ends the run of consecutive searches: the block freezes with its final
    /// success or error background.
    fn close_explore_block(&mut self) {
        if let Some(block) = self.explore_block.take() {
            block.borrow_mut().close();
        }
    }

    /// Ends the run on an aborted or failed turn, the reference way
    /// (`pending_explore_tools.drain()` → `complete_call(true)`, then
    /// `close_active_explore()`): every call that never reported back is
    /// failed, calls that already completed keep their result — a block whose
    /// calls all succeeded settles green even on an aborted turn.
    fn abort_explore_block(&mut self) {
        for block in &self.chat_explore_blocks {
            block.borrow_mut().fail_running_calls();
        }
        self.close_explore_block();
    }

    /// The open search block, or a fresh one appended to the chat.
    fn open_explore_block(&mut self, replayed: bool) -> Rc<RefCell<ExploreBlockComponent>> {
        if let Some(block) = self
            .explore_block
            .as_ref()
            .filter(|block| block.borrow().is_open())
        {
            return Rc::clone(block);
        }
        let block = Rc::new(RefCell::new(ExploreBlockComponent::new()));
        if replayed {
            block.borrow_mut().mark_replayed();
        }
        block.borrow_mut().set_expanded(self.tool_output_expanded);
        self.chat_container
            .borrow_mut()
            .add_child(Rc::clone(&block) as ComponentRef);
        self.chat_expandables
            .push(Rc::clone(&block) as Rc<RefCell<dyn Expandable>>);
        self.chat_explore_blocks.push(Rc::clone(&block));
        self.explore_block = Some(Rc::clone(&block));
        block
    }

    fn create_tool_component(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        args: serde_json::Value,
    ) -> ToolExecutionComponent {
        let settings = self.settings();
        let core = self.ui.clone();
        ToolExecutionComponent::new(
            tool_name,
            tool_call_id,
            args,
            ToolExecutionOptions {
                show_images: Some(settings.get_show_images()),
                image_width_cells: Some(settings.get_image_width_cells() as usize),
            },
            None,
            Rc::new(move || core.request_render()),
            self.cwd(),
        )
    }

    // ------------------------------------------------------------------
    // Transcript
    // ------------------------------------------------------------------

    fn render_initial_messages(&mut self) {
        let entries = self.session().with_session_manager(|manager| {
            manager
                .get_branch(None)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        });
        self.render_session_entries(&entries, true);
        self.render_project_trust_warning_if_needed();

        let compaction_count = self
            .session()
            .with_session_manager(|manager| manager.get_entries())
            .iter()
            .filter(|entry| matches!(entry, SessionEntry::Compaction { .. }))
            .count();
        if compaction_count > 0 {
            let times = if compaction_count == 1 {
                "1 time".to_owned()
            } else {
                format!("{compaction_count} times")
            };
            self.show_status(&format!("Session compacted {times}"));
        }
    }

    fn render_session_entries(&mut self, entries: &[SessionEntry], populate_history: bool) {
        let mut items: Vec<AgentMessage> = Vec::new();
        for entry in entries {
            if let Some(record) = TaskLifecycleRecord::from_session_entry(entry) {
                items.push(AgentMessage::Custom(CustomMessage {
                    custom_type: TASK_LIFECYCLE_ENTRY_TYPE.to_owned(),
                    content: UserContent::Text(String::new()),
                    display: true,
                    details: serde_json::to_value(&record).ok(),
                    timestamp: record.task.base().started_at,
                }));
                continue;
            }
            items.extend(session_entry_to_context_messages(entry));
        }
        self.render_session_items(&items, populate_history);
    }

    /// The cache-miss notices and the custom session entries belong to the
    /// slices that own them.
    fn render_session_items(&mut self, items: &[AgentMessage], populate_history: bool) {
        self.pending_tools.clear();
        self.chat_tool_rows.clear();
        self.chat_expandables.clear();
        let mut rendered_pending: Vec<(String, Rc<RefCell<ToolExecutionComponent>>)> = Vec::new();

        for item in items {
            match item {
                AgentMessage::Assistant(message) => {
                    self.add_message_to_chat(item, populate_history);
                    // Reproduce the live text/thinking boundary before this
                    // restored message's calls open their own block.
                    if streamed_visible_chars(message) > 0 {
                        self.close_explore_block();
                    }
                    for content in message.content.iter() {
                        let notagent_ai::types::AssistantContent::ToolCall(call) = content else {
                            continue;
                        };
                        // No transcript row for `todo_write` or `task` on
                        // restore either — same deviation as
                        // `add_tool_component`; subagents replay through their
                        // lifecycle rows.
                        if call.name == "todo_write" || call.name == "task" {
                            continue;
                        }
                        let args = serde_json::Value::Object(call.arguments.clone());
                        if is_background_bash_call(&call.name, &args) {
                            continue;
                        }
                        // Restored searches group like live ones; the block is
                        // replayed history and closes at the end of the items.
                        if is_explore_tool(&call.name) {
                            let block = self.open_explore_block(true);
                            block.borrow_mut().push_call(
                                &call.name,
                                call.id.clone(),
                                &serde_json::Value::Object(call.arguments.clone()),
                            );
                            if matches!(
                                message.stop_reason,
                                StopReason::Aborted | StopReason::Error
                            ) {
                                block.borrow_mut().complete_call(&call.id, true);
                            }
                            continue;
                        }
                        self.close_explore_block();
                        let component = Rc::new(RefCell::new(
                            self.create_tool_component(&call.name, &call.id, args),
                        ));
                        component
                            .borrow_mut()
                            .set_expanded(self.tool_output_expanded);
                        self.chat_container
                            .borrow_mut()
                            .add_child(Rc::clone(&component) as ComponentRef);
                        self.chat_tool_rows.push(Rc::clone(&component));
                        self.chat_expandables
                            .push(Rc::clone(&component) as Rc<RefCell<dyn Expandable>>);

                        if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                            let error_message = if message.stop_reason == StopReason::Aborted {
                                let attempt = self.session().retry_attempt();
                                if attempt > 0 {
                                    format!(
                                        "Aborted after {attempt} retry attempt{}",
                                        if attempt > 1 { "s" } else { "" }
                                    )
                                } else {
                                    "Operation aborted".to_owned()
                                }
                            } else {
                                message
                                    .error_message
                                    .clone()
                                    .unwrap_or_else(|| "Error".to_owned())
                            };
                            component
                                .borrow_mut()
                                .update_result(error_result(&error_message), false);
                        } else {
                            rendered_pending.push((call.id.clone(), component));
                        }
                    }
                    // After the message's own calls have joined the block, so
                    // that a restored abort settles the same single block the
                    // live path settles — not a fresh one behind it.
                    if matches!(message.stop_reason, StopReason::Aborted | StopReason::Error) {
                        self.abort_explore_block();
                    }
                }
                AgentMessage::ToolResult(result) => {
                    if let Some(index) = rendered_pending
                        .iter()
                        .position(|(id, _)| id == &result.tool_call_id)
                    {
                        let (_, component) = rendered_pending.remove(index);
                        component
                            .borrow_mut()
                            .update_result(result_from_message(result), false);
                    } else {
                        // Search results have no row; route them to the block
                        // carrying the call.
                        for block in self.chat_explore_blocks.iter().rev() {
                            if block.borrow().has_call(&result.tool_call_id) {
                                block
                                    .borrow_mut()
                                    .complete_call(&result.tool_call_id, result.is_error);
                                break;
                            }
                        }
                    }
                }
                message => self.add_message_to_chat(message, populate_history),
            }
        }

        // Restored history is over; whatever block is still open freezes.
        self.close_explore_block();
        self.pending_tools.extend(rendered_pending);
        self.ui.request_render();
    }

    fn add_message_to_chat(&mut self, message: &AgentMessage, populate_history: bool) {
        // A new message row between searches ends the block (the streamed
        // assistant path closes it at MessageEnd instead).
        // A tool result belongs to a call the block already carries, so it
        // must not end the run — closing here gave every call its own block.
        // Everything else that lands in the transcript does end it, matching
        // the events the reference closes on (user message, compaction,
        // interrupt).
        if !matches!(
            message,
            AgentMessage::Assistant(_) | AgentMessage::ToolResult(_)
        ) {
            self.close_explore_block();
        }
        match message {
            AgentMessage::User(message) => {
                let text = user_message_text(message);
                if text.is_empty() {
                    return;
                }
                if !self.chat_container.borrow().children.is_empty() {
                    self.chat_container
                        .borrow_mut()
                        .add_child(component_ref(Spacer::new(1)));
                }
                match parse_skill_block(&text) {
                    Some(block) => {
                        let component =
                            Rc::new(RefCell::new(SkillInvocationMessageComponent::new(
                                block.clone(),
                                Some(get_markdown_theme()),
                            )));
                        component
                            .borrow_mut()
                            .set_expanded(self.tool_output_expanded);
                        self.chat_container
                            .borrow_mut()
                            .add_child(component as ComponentRef);
                        if let Some(user_message) = block.user_message {
                            let mut chat = self.chat_container.borrow_mut();
                            chat.add_child(component_ref(Spacer::new(1)));
                            chat.add_child(component_ref(UserMessageComponent::new(
                                user_message,
                                Some(get_markdown_theme()),
                                Some(self.output_pad),
                                Vec::new(),
                            )));
                        }
                    }
                    None => {
                        self.chat_container.borrow_mut().add_child(component_ref(
                            UserMessageComponent::new(
                                text.clone(),
                                Some(get_markdown_theme()),
                                Some(self.output_pad),
                                Vec::new(),
                            ),
                        ));
                    }
                }
                if populate_history {
                    self.editor.borrow_mut().editor_mut().add_to_history(&text);
                }
            }
            AgentMessage::Assistant(message) => {
                let component = Rc::new(RefCell::new(AssistantMessageComponent::new(
                    Some(message.clone()),
                    self.hide_thinking_block,
                    Some(get_markdown_theme()),
                    Some(self.hidden_thinking_label.clone()),
                    Some(self.output_pad),
                    Vec::new(),
                )));
                component
                    .borrow_mut()
                    .set_expanded(self.tool_output_expanded);
                self.chat_container
                    .borrow_mut()
                    .add_child(Rc::clone(&component) as ComponentRef);
                self.chat_expandables
                    .push(component as Rc<RefCell<dyn Expandable>>);
            }
            AgentMessage::CompactionSummary(message) => {
                let component = Rc::new(RefCell::new(CompactionSummaryMessageComponent::new(
                    message.clone(),
                    Some(get_markdown_theme()),
                )));
                component
                    .borrow_mut()
                    .set_expanded(self.tool_output_expanded);
                let mut chat = self.chat_container.borrow_mut();
                chat.add_child(component_ref(Spacer::new(1)));
                chat.add_child(component as ComponentRef);
            }
            AgentMessage::BranchSummary(message) => {
                let component = Rc::new(RefCell::new(BranchSummaryMessageComponent::new(
                    message.clone(),
                    Some(get_markdown_theme()),
                )));
                component
                    .borrow_mut()
                    .set_expanded(self.tool_output_expanded);
                let mut chat = self.chat_container.borrow_mut();
                chat.add_child(component_ref(Spacer::new(1)));
                chat.add_child(component as ComponentRef);
            }
            AgentMessage::Custom(message) => {
                if message.custom_type == TASK_LIFECYCLE_ENTRY_TYPE {
                    if let Some(record) = message
                        .details
                        .clone()
                        .and_then(|details| serde_json::from_value(details).ok())
                    {
                        self.append_task_lifecycle(&record);
                    }
                } else if message.display {
                    let component = Rc::new(RefCell::new(CustomMessageComponent::new(
                        message.clone(),
                        Some(get_markdown_theme()),
                        Some(self.output_pad),
                    )));
                    component
                        .borrow_mut()
                        .set_expanded(self.tool_output_expanded);
                    self.chat_container
                        .borrow_mut()
                        .add_child(component as ComponentRef);
                }
            }
            // `bashExecution` belongs to the bash slice; tool results are drawn
            // inline with their call.
            AgentMessage::BashExecution(_) | AgentMessage::ToolResult(_) => {}
        }
    }

    fn render_project_trust_warning_if_needed(&mut self) {
        if self.settings().is_project_trusted()
            || !has_trust_requiring_project_resources(&self.cwd())
        {
            return;
        }
        let mut chat = self.chat_container.borrow_mut();
        if !chat.children.is_empty() {
            chat.add_child(component_ref(Spacer::new(1)));
        }
        chat.add_child(component_ref(Text::new(
            theme().fg(
                ThemeColor::Warning,
                &format!(
                    "This project is not trusted. Project {CONFIG_DIR_NAME} resources and packages are ignored. Use /trust to save a trust decision, then restart notagent."
                ),
            ),
            1,
            0,
        )));
    }

    // ------------------------------------------------------------------
    // UI helpers
    // ------------------------------------------------------------------

    fn show_status(&mut self, message: &str) {
        let styled = theme().fg(ThemeColor::Dim, message);
        {
            let chat = self.chat_container.borrow();
            let len = chat.children.len();
            if len >= 2 {
                let last = &chat.children[len - 1];
                let second_last = &chat.children[len - 2];
                let is_last_status = self
                    .last_status_text
                    .as_ref()
                    .is_some_and(|text| Rc::ptr_eq(last, &(Rc::clone(text) as ComponentRef)));
                let is_second_last_spacer = self
                    .last_status_spacer
                    .as_ref()
                    .is_some_and(|spacer| Rc::ptr_eq(second_last, spacer));
                if is_last_status && is_second_last_spacer {
                    drop(chat);
                    if let Some(text) = self.last_status_text.as_ref() {
                        text.borrow_mut().set_text(styled);
                    }
                    self.ui.request_render();
                    return;
                }
            }
        }

        let spacer: ComponentRef = component_ref(Spacer::new(1));
        let text = Rc::new(RefCell::new(Text::new(styled, 1, 0)));
        {
            let mut chat = self.chat_container.borrow_mut();
            chat.add_child(Rc::clone(&spacer));
            chat.add_child(Rc::clone(&text) as ComponentRef);
        }
        self.last_status_spacer = Some(spacer);
        self.last_status_text = Some(text);
        self.ui.request_render();
    }

    pub fn show_error(&mut self, message: &str) {
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(Text::new(
            theme().fg(ThemeColor::Error, &format!("Error: {message}")),
            self.output_pad,
            0,
        )));
        drop(chat);
        self.ui.request_render();
    }

    pub fn show_warning(&mut self, message: &str) {
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(Text::new(
            theme().fg(ThemeColor::Warning, &format!("Warning: {message}")),
            1,
            0,
        )));
        drop(chat);
        self.ui.request_render();
    }

    fn clear_editor(&mut self) {
        self.editor.borrow_mut().editor_mut().set_text("");
        self.ui.request_render();
    }

    fn update_editor_border_color(&mut self) {
        let color = if self.is_bash_mode {
            theme().get_bash_mode_border_color()
        } else {
            get_editor_theme().border_color
        };
        self.editor.borrow_mut().editor_mut().border_color = color;
        self.ui.request_render();
    }

    fn show_status_indicator(&mut self, indicator: StatusIndicator) {
        self.clear_status_indicator(None);
        let indicator = Rc::new(RefCell::new(indicator));
        indicator.borrow_mut().loader_mut().start();
        let mut container = self.status_container.borrow_mut();
        container.clear();
        container.add_child(Rc::clone(&indicator) as ComponentRef);
        drop(container);
        self.active_status_indicator = Some(indicator);
    }

    fn show_compaction_chat_indicator(&mut self, reason: CompactionStatusReason) {
        self.clear_compaction_chat_indicator();
        self.clear_status_indicator(None);

        let indicator = Rc::new(RefCell::new(StatusIndicator::compaction(reason)));
        indicator.borrow_mut().loader_mut().start();
        let spacer = component_ref(Spacer::new(1));
        let component = Rc::clone(&indicator) as ComponentRef;
        {
            let mut chat = self.chat_container.borrow_mut();
            chat.add_child(Rc::clone(&spacer));
            chat.add_child(Rc::clone(&component));
        }
        self.compaction_chat_indicator = Some(ActiveCompactionChatIndicator {
            spacer,
            indicator: component,
        });
        self.active_status_indicator = Some(indicator);
    }

    fn restore_compaction_chat_indicator(&self) {
        let Some(active) = self.compaction_chat_indicator.as_ref() else {
            return;
        };
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(Rc::clone(&active.spacer));
        chat.add_child(Rc::clone(&active.indicator));
    }

    fn clear_compaction_chat_indicator(&mut self) {
        self.clear_status_indicator(Some(StatusIndicatorKind::Compaction));
        let Some(active) = self.compaction_chat_indicator.take() else {
            return;
        };
        let mut chat = self.chat_container.borrow_mut();
        chat.remove_child(&active.indicator);
        chat.remove_child(&active.spacer);
    }

    /// Stop the indicator but leave it on screen, reporting what the work took.
    /// Used where an activity finished on its own. Somewhere the session is
    /// being torn down or switched, `clear_status_indicator` is still the right
    /// call: a runtime for work the user is no longer looking at is clutter.
    fn settle_status_indicator(&mut self, kind: StatusIndicatorKind) {
        let Some(indicator) = self.active_status_indicator.clone() else {
            return;
        };
        if indicator.borrow().kind != kind {
            return;
        }
        indicator.borrow_mut().settle();
        self.ui.request_render();
    }

    fn clear_status_indicator(&mut self, kind: Option<StatusIndicatorKind>) {
        let Some(indicator) = self.active_status_indicator.clone() else {
            return;
        };
        if let Some(kind) = kind
            && indicator.borrow().kind != kind
        {
            return;
        }
        indicator.borrow_mut().dispose();
        self.active_status_indicator = None;
        self.show_idle_status();
    }

    fn show_idle_status(&self) {
        let mut container = self.status_container.borrow_mut();
        container.clear();
        container.add_child(component_ref(IdleStatus));
    }
}

fn path_command_argument(text: &str, command: &str) -> Option<String> {
    if text == command {
        return None;
    }
    let rest = text.strip_prefix(&format!("{command} "))?;
    let arguments = rest.trim_start();
    if arguments.is_empty() {
        return None;
    }
    let first = arguments.chars().next()?;
    if first == '"' || first == '\'' {
        let closing = arguments[first.len_utf8()..].find(first)?;
        return Some(arguments[first.len_utf8()..first.len_utf8() + closing].to_owned());
    }
    match arguments.find(char::is_whitespace) {
        Some(index) => Some(arguments[..index].to_owned()),
        None => Some(arguments.to_owned()),
    }
}

/// The reason a compaction reports, as the status line names it.
fn compaction_status_reason(reason: CompactionReason) -> CompactionStatusReason {
    match reason {
        CompactionReason::Manual => CompactionStatusReason::Manual,
        CompactionReason::Threshold => CompactionStatusReason::Threshold,
        CompactionReason::Overflow => CompactionStatusReason::Overflow,
    }
}

/// Whether an api-key method offers an interactive login (`method.login`).
fn has_api_key_login(api_key: &dyn notagent_ai::auth::types::ApiKeyAuth) -> bool {
    let interaction = notagent_ai::auth::types::ProviderAuthInteraction {
        interaction: &NoAuthInteraction,
        signal: tokio_util::sync::CancellationToken::new(),
    };
    api_key.login(&interaction).is_some()
}

/// A never-used interaction, only to ask a method whether it has a login.
struct NoAuthInteraction;

impl notagent_ai::auth::types::AuthInteraction for NoAuthInteraction {
    fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        None
    }

    fn prompt(
        &self,
        _prompt: AuthPrompt,
    ) -> futures::future::BoxFuture<'_, Result<String, notagent_ai::auth::types::AuthError>> {
        Box::pin(async {
            Err(notagent_ai::auth::types::AuthError(
                "Login cancelled".to_owned(),
            ))
        })
    }

    fn notify(&self, _event: AuthEvent) {}
}

fn is_unknown_model(model: Option<&Model>) -> bool {
    model.is_some_and(|model| {
        model.provider == "unknown" && model.id == "unknown" && model.api == "unknown"
    })
}

/// `configuredEnabledIds(models)` of the models selector.
fn configured_enabled_ids(
    configured_patterns: Option<&[String]>,
    models: &[Model],
) -> Option<Vec<String>> {
    let patterns = configured_patterns.filter(|patterns| !patterns.is_empty())?;
    let resolved = resolve_model_scope_from_models(patterns, models);
    let mut ids: Vec<String> = resolved
        .scoped_models
        .iter()
        .map(|scoped| format!("{}/{}", scoped.model.provider, scoped.model.id))
        .collect();
    for diagnostic in resolved.diagnostics {
        if diagnostic.code == ModelScopeDiagnosticCode::NoMatch
            && !ids.contains(&diagnostic.pattern)
        {
            ids.push(diagnostic.pattern);
        }
    }
    Some(ids)
}

/// The wire name of a thinking level, as the status line prints it.
fn thinking_level_name(level: notagent_agent::types::ThinkingLevel) -> &'static str {
    use notagent_agent::types::ThinkingLevel;
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

/// The two spellings of the tree filter mode.
fn tree_filter_mode(
    mode: crate::core::settings_manager::TreeFilterMode,
) -> crate::modes::interactive::components::tree_selector::FilterMode {
    use crate::core::settings_manager::TreeFilterMode as Setting;
    use crate::modes::interactive::components::tree_selector::FilterMode as Mode;
    match mode {
        Setting::Default => Mode::Default,
        Setting::NoTools => Mode::NoTools,
        Setting::UserOnly => Mode::UserOnly,
        Setting::LabeledOnly => Mode::LabeledOnly,
        Setting::All => Mode::All,
    }
}

/// The settings spelling of a queue mode (`settingsQueueMode` of the session).
fn settings_queue_mode(
    mode: notagent_agent::QueueMode,
) -> crate::core::settings_manager::QueueMode {
    match mode {
        notagent_agent::QueueMode::All => crate::core::settings_manager::QueueMode::All,
        notagent_agent::QueueMode::OneAtATime => {
            crate::core::settings_manager::QueueMode::OneAtATime
        }
    }
}

/// The agent spelling of a queue mode.
fn agent_queue_mode(mode: crate::core::settings_manager::QueueMode) -> notagent_agent::QueueMode {
    match mode {
        crate::core::settings_manager::QueueMode::All => notagent_agent::QueueMode::All,
        crate::core::settings_manager::QueueMode::OneAtATime => {
            notagent_agent::QueueMode::OneAtATime
        }
    }
}

/// The `transport` setting, as `settings.json` spells it.
fn parse_transport(value: &str) -> notagent_ai::types::Transport {
    match value {
        "sse" => notagent_ai::types::Transport::Sse,
        "websocket" => notagent_ai::types::Transport::Websocket,
        "websocket-cached" => notagent_ai::types::Transport::WebsocketCached,
        _ => notagent_ai::types::Transport::Auto,
    }
}

fn transport_wire_name(transport: notagent_ai::types::Transport) -> &'static str {
    match transport {
        notagent_ai::types::Transport::Sse => "sse",
        notagent_ai::types::Transport::Websocket => "websocket",
        notagent_ai::types::Transport::WebsocketCached => "websocket-cached",
        notagent_ai::types::Transport::Auto => "auto",
    }
}

/// The two spellings of the scrollbar setting.
fn scroll_view_scrollbar(
    mode: crate::core::settings_manager::ScrollViewScrollbar,
) -> notagent_tui::components::scroll_view::ScrollViewScrollbar {
    match mode {
        crate::core::settings_manager::ScrollViewScrollbar::Auto => {
            notagent_tui::components::scroll_view::ScrollViewScrollbar::Auto
        }
        crate::core::settings_manager::ScrollViewScrollbar::Always => {
            notagent_tui::components::scroll_view::ScrollViewScrollbar::Always
        }
        crate::core::settings_manager::ScrollViewScrollbar::Hidden => {
            notagent_tui::components::scroll_view::ScrollViewScrollbar::Hidden
        }
    }
}

fn settings_scrollbar(
    mode: notagent_tui::components::scroll_view::ScrollViewScrollbar,
) -> crate::core::settings_manager::ScrollViewScrollbar {
    match mode {
        notagent_tui::components::scroll_view::ScrollViewScrollbar::Auto => {
            crate::core::settings_manager::ScrollViewScrollbar::Auto
        }
        notagent_tui::components::scroll_view::ScrollViewScrollbar::Always => {
            crate::core::settings_manager::ScrollViewScrollbar::Always
        }
        notagent_tui::components::scroll_view::ScrollViewScrollbar::Hidden => {
            crate::core::settings_manager::ScrollViewScrollbar::Hidden
        }
    }
}

/// `Date.now()` in milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// `JSON.stringify(value)` for a string — the quoting `/name` and `/debug` show.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

const DEFAULT_WORKING_MESSAGE: &str = "Working...";
const DEFAULT_HIDDEN_THINKING_LABEL: &str = "Thinking...";

/// `formatCompactList(items, options)` of `showLoadedResources`.
fn compact_list(items: &[String], sort: bool) -> String {
    let mut labels: Vec<String> = items
        .iter()
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect();
    if sort {
        labels.sort();
    }
    theme().fg(ThemeColor::Dim, &format!("  {}", labels.join(", ")))
}

/// on an already shortened path.
fn compact_path_label(short_path: &str) -> String {
    short_path
        .replace('\\', "/")
        .split('/')
        .rfind(|segment| !segment.is_empty() && *segment != "~")
        .map(str::to_owned)
        .unwrap_or_else(|| short_path.to_owned())
}

/// label and the optional scope note.
fn display_source_label(source_info: &SourceInfo) -> (&'static str, Option<&'static str>) {
    let source = source_info.source.as_str();
    let scope = source_info.scope;
    if source == "local" {
        return match scope {
            SourceScope::User => ("user", None),
            SourceScope::Project => ("project", None),
            SourceScope::Temporary => ("path", Some("temp")),
        };
    }
    if source == "cli" {
        return ("path", Some("cli"));
    }
    if source.starts_with("npm:") || source.starts_with("git:") {
        return ("package", None);
    }
    ("path", None)
}

fn quote_if_needed(value: &str) -> String {
    let needs_quotes = value
        .chars()
        .any(|character| !(character.is_ascii_alphanumeric() || "_-./~:@".contains(character)));
    if !value.is_empty() && !needs_quotes {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn user_message_text(message: &UserMessage) -> String {
    match &message.content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|content| match content {
                TextOrImageContent::Text(text) => Some(text.text.as_str()),
                TextOrImageContent::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join(""),
    }
}

/// The result of a finished tool call, as the row shows it.
fn tool_result(result: AgentToolResult, is_error: bool) -> ToolExecutionResult {
    ToolExecutionResult {
        content: result.content,
        details: result.details,
        is_error,
    }
}

/// The same for a persisted `toolResult` message.
fn result_from_message(message: &ToolResultMessage) -> ToolExecutionResult {
    ToolExecutionResult {
        content: message.content.clone(),
        details: message.details.clone(),
        is_error: message.is_error,
    }
}

const ANTHROPIC_SUBSCRIPTION_AUTH_WARNING: &str = "Anthropic subscription auth is active. Third-party harness usage draws from extra usage and is billed per token, not your Claude plan limits. Manage extra usage at https://claude.ai/settings/usage. Disable this warning in /settings.";

/// The one-line error result the aborted and failed paths write into every
/// still-pending tool row.
fn error_result(message: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![TextOrImageContent::Text(TextContent {
            text: message.to_owned(),
            ..Default::default()
        })],
        details: None,
        is_error: true,
    }
}

/// Splits an optional leading `user` off an `/mcp` argument.
/// The project file is the default because that is the one a repository shares;
/// reaching the user's own file is the deliberate act and is spelled out.
fn split_scope(argument: &str) -> (crate::core::mcp::McpConfigScope, &str) {
    use crate::core::mcp::McpConfigScope;
    match argument.strip_prefix("user") {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => {
            (McpConfigScope::User, rest.trim())
        }
        _ => (McpConfigScope::Project, argument.trim()),
    }
}

/// The marker shown in the editor for pasted image `id`.
pub fn format_image_marker(id: u32) -> String {
    format!("[Image #{id}]")
}

/// The ids of the `[Image #n]` markers in `text`, in the order they appear.
/// `#0` is not a marker this ever writes, so a text containing one is the
/// user's own and resolves to nothing.
pub fn parse_image_markers(text: &str) -> Vec<u32> {
    static MARKER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[Image #(\d+)\]").expect("a literal pattern")
    });
    MARKER
        .captures_iter(text)
        .filter_map(|capture| capture.get(1)?.as_str().parse::<u32>().ok())
        .filter(|id| *id > 0)
        .collect()
}
