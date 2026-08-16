//! Port of `packages/coding-agent/src/modes/interactive/interactive-mode.ts`
//! (6 688 LOC) — the wiring between the TUI components and the agent session.
//!
//! The file is ported in vertical slices; every slice carries its own section
//! below and its own entry in `crates/notagent/PARITY.md`. What is here now:
//! the entry point with the terminal seam (interface request A-23), the main
//! loop, the editor submit path and the transcript half of the session events.
//!
//! # How this differs in shape from the TypeScript original
//!
//! TypeScript keeps one long-lived object whose callbacks (`onSubmit`,
//! `onAction`, the agent subscription) mutate its fields while the Node event
//! loop renders in between. Rust has neither an ambient event loop nor
//! re-entrant `&mut self`, so two things change (deviation class 1, no
//! behavioural difference):
//!
//! 1. **The loop is written out.** [`InteractiveMode::run`] is a `select!` over
//!    the editor's submissions, the session's events, the in-flight prompt and
//!    the animation deadlines. The callbacks the components take only *post*
//!    to that loop; every field mutation happens inside it, where `&mut self`
//!    is available. `Editor::take_submitted`/`take_changes` (workstream A) are
//!    the queue the submit and change handlers of TypeScript drain from.
//! 2. **Rendering is driven from outside.** `createInteractiveTui` hands
//!    TypeScript a renderer that schedules its own frames through
//!    `setTimeout`; here the caller owns the render loop
//!    (`notagent_tui::tui::run_until`, interface request A-20) and drives the
//!    renderer this module hands back. That is the same seam the startup
//!    dialogs use (`cli/startup_ui.rs`) and the seam the G3 end-to-end
//!    scenarios need, because it lets a test pass its own terminal — exactly
//!    what `InteractiveTuiOptions.terminal` does in TypeScript
//!    (`interactive-mode.ts:344-354`).
//!
//! The third consequence is the exit code. `shutdown()` ends the TypeScript
//! process with `process.exit(0)` from inside the class; a future that owns the
//! terminal cannot do that without stranding the render loop, so the mode
//! resolves with the exit code and `main_app` returns it (deviation class 1).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notagent_agent::types::{AgentEvent, AgentMessage, AgentToolResult};
use notagent_ai::types::{
    AssistantMessage, ImageContent, StopReason, TextContent, TextOrImageContent, ToolResultMessage,
    UserContent, UserMessage,
};
use notagent_tui::components::markdown::Markdown;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::components::truncated_text::TruncatedText;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::terminal::{ProcessTerminal, Terminal, TerminalPump};
use notagent_tui::tui::{
    Component, ComponentRef, Container, RenderLoop, TuiCore, TuiStopOptions, component_ref,
};
use notagent_tui::tui_main_screen::TuiMainScreen;

use crate::config::{APP_NAME, APP_TITLE, CONFIG_DIR_NAME, VERSION, get_agent_dir};
use crate::core::agent_session::{
    AgentSession, AgentSessionEvent, CompactionReason, ExecuteBashOptions, PromptOptions,
    QueueBehavior, parse_skill_block,
};
use crate::core::agent_session_runtime::AgentSessionRuntime;
use crate::core::bash_executor::BashResult;
use crate::core::keybindings::KeybindingsManager;
use crate::core::messages::create_compaction_summary_message;
use crate::core::session_manager::{SessionEntry, session_entry_to_context_messages};
use crate::core::settings_manager::TuiMode;
use crate::core::tools::truncate::TruncationResult;
use crate::core::trust_manager::has_trust_requiring_project_resources;
use crate::modes::interactive::components::armin::ArminComponent;
use crate::modes::interactive::components::assistant_message::AssistantMessageComponent;
use crate::modes::interactive::components::bash_execution::BashExecutionComponent;
use crate::modes::interactive::components::bordered_loader::BorderedLoader;
use crate::modes::interactive::components::branch_summary_message::BranchSummaryMessageComponent;
use crate::modes::interactive::components::compaction_summary_message::CompactionSummaryMessageComponent;
use crate::modes::interactive::components::custom_editor::CustomEditor;
use crate::modes::interactive::components::custom_message::CustomMessageComponent;
use crate::modes::interactive::components::dynamic_border::DynamicBorder;
use crate::modes::interactive::components::earendil_announcement::EarendilAnnouncementComponent;
use crate::modes::interactive::components::footer::{FooterComponent, format_tokens};
use crate::modes::interactive::components::keybinding_hints::{
    key_display_text, key_hint, key_text, raw_key_hint,
};
use crate::modes::interactive::components::list_selector::ListSelectorComponent;
use crate::modes::interactive::components::skill_invocation_message::SkillInvocationMessageComponent;
use crate::modes::interactive::components::status_indicator::{
    CompactionStatusReason, IdleStatus, StatusIndicator, StatusIndicatorKind,
};
use crate::modes::interactive::components::to_locale_string;
use crate::modes::interactive::components::tool_execution::{
    ToolExecutionComponent, ToolExecutionOptions, ToolExecutionResult,
};
use crate::modes::interactive::components::user_message::UserMessageComponent;
use crate::modes::interactive::theme::theme::{
    ThemeColor, get_editor_theme, get_markdown_theme, on_theme_change, set_registered_themes, theme,
};
use crate::modes::interactive::theme::theme_controller::InteractiveThemeController;

// ============================================================================
// The terminal seam (interface request A-23)
// ============================================================================

/// A terminal handed to the interactive mode from outside, together with the
/// pump that feeds it.
///
/// TypeScript passes a single `terminal` (`InteractiveTuiOptions.terminal`);
/// in the port `ProcessTerminal::new().into_shared()` splits the terminal into
/// the handle the TUI writes to and the pump the render loop drives
/// (interface request A-20), so both halves travel together.
pub struct InteractiveTerminal {
    pub terminal: Box<dyn Terminal>,
    pub pump: Box<dyn TerminalPump>,
}

/// Options for the interactive mode — `InteractiveModeOptions`
/// (`interactive-mode.ts:325-342`) plus the terminal seam.
#[derive(Default)]
pub struct InteractiveModeOptions {
    /// Providers that were migrated to `auth.json` (shows a warning).
    pub migrated_providers: Vec<String>,
    /// Warning shown when the session model could not be restored.
    pub model_fallback_message: Option<String>,
    /// Cwd to trust after a reload if it gained a `.notagent` directory during
    /// this implicitly trusted session.
    pub auto_trust_on_reload_cwd: Option<String>,
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
/// (`CompactionQueuedMessage` in `interactive-mode.ts:212-215`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionQueueMode {
    Steer,
    FollowUp,
}

struct CompactionQueuedMessage {
    text: String,
    mode: CompactionQueueMode,
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
}

/// The prompt the main loop currently drives, if any.
type PromptFuture = Pin<Box<dyn Future<Output = Result<(), String>>>>;

/// What [`create_interactive_mode`] hands back: the three pieces a caller needs
/// to run the mode on its own render loop.
pub struct InteractiveModeHandle {
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
///
/// The composition root of `createInteractiveTui` (`interactive-mode.ts:353-366`)
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
    InteractiveModeHandle {
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
///
/// `TuiMainScreen` today; the alternate screen joins it with the fullscreen
/// slice, which is why the renderer already lives behind a cell that can be
/// swapped underneath a caller holding [`InteractiveRenderer`].
enum ActiveRenderer {
    Main(TuiMainScreen),
}

struct RendererState {
    renderer: ActiveRenderer,
    /// Bumped whenever the renderer is replaced, so a handle notices.
    generation: u64,
}

/// Shared handle on the renderer; the mode holds one, the caller another.
#[derive(Clone)]
struct RendererCell(Rc<RefCell<RendererState>>);

impl RendererCell {
    fn new(renderer: ActiveRenderer) -> Self {
        Self(Rc::new(RefCell::new(RendererState {
            renderer,
            generation: 0,
        })))
    }

    fn generation(&self) -> u64 {
        self.0.borrow().generation
    }

    fn core(&self) -> TuiCore {
        let state = self.0.borrow();
        match &state.renderer {
            ActiveRenderer::Main(screen) => screen.core().clone(),
        }
    }

    fn start(&self) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.start(),
        }
    }

    fn stop(&self, options: TuiStopOptions) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.stop(options),
        }
    }

    /// `ui.requestRender(force)` — the forcing variant resets the differential
    /// render state, which `/reload` and the external editor need.
    fn request_render(&self, force: bool) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.request_render(force),
        }
    }

    fn render_pending_frame(&self) {
        let mut state = self.0.borrow_mut();
        match &mut state.renderer {
            ActiveRenderer::Main(screen) => screen.render_pending_frame(),
        }
    }

    fn mode(&self) -> TuiMode {
        let state = self.0.borrow();
        match &state.renderer {
            ActiveRenderer::Main(_) => TuiMode::Regular,
        }
    }
}

/// The renderer handed to the caller's render loop.
///
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
/// `setupKeyHandlers`, `interactive-mode.ts:3035-3057`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppAction {
    /// `app.clear` — Ctrl+C.
    Clear,
    /// `app.message.followUp` — Alt+Enter.
    FollowUp,
    /// `app.message.dequeue`.
    Dequeue,
    /// `app.exit` — Ctrl+D on an empty editor.
    Exit,
    /// `app.interrupt` — Escape.
    Escape,
}

/// What the component callbacks post into [`InteractiveMode::run`].
enum UiMessage {
    /// Terminal input was dispatched; the editor's queues may have grown.
    TerminalInput,
    Action(AppAction),
    /// A theme file changed on disk (`onThemeChange`).
    ThemeChanged,
}

// ============================================================================
// A text component with a collapsed and an expanded form
// ============================================================================

/// `class ExpandableText extends Text` (`interactive-mode.ts:190-216`).
struct ExpandableText {
    text: Text,
    collapsed: String,
    expanded: String,
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
    fn render(&mut self, width: usize) -> Vec<String> {
        self.text.render(width)
    }

    fn invalidate(&mut self) {
        self.text.invalidate();
    }
}

// ============================================================================
// The mode
// ============================================================================

/// `class InteractiveMode` (`interactive-mode.ts:398-6687`).
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
    tasks_panel_container: Rc<RefCell<Container>>,
    widget_container_above: Rc<RefCell<Container>>,
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
    built_in_header: Option<Rc<RefCell<ExpandableText>>>,

    version: String,
    is_initialized: bool,

    /// The status line: the idle placeholder or one indicator.
    active_status_indicator: Option<Rc<RefCell<StatusIndicator>>>,
    working_message: Option<String>,
    working_visible: bool,
    hidden_thinking_label: String,

    last_sigint_time: Option<Instant>,

    /// `lastStatusSpacer`/`lastStatusText` — the mutation target of two status
    /// lines emitted back to back.
    last_status_spacer: Option<ComponentRef>,
    last_status_text: Option<Rc<RefCell<Text>>>,

    streaming_component: Option<Rc<RefCell<AssistantMessageComponent>>>,
    streaming_message: Option<AssistantMessage>,
    /// `pendingTools` — insertion-ordered like the JavaScript `Map`.
    pending_tools: Vec<(String, Rc<RefCell<ToolExecutionComponent>>)>,

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

    tool_output_expanded: bool,
    hide_thinking_block: bool,
    output_pad: usize,
    is_bash_mode: bool,

    /// Prompts waiting for the main loop (`pendingUserInputs`/`getUserInput`).
    pending_user_inputs: VecDeque<String>,

    is_shutting_down: bool,
    exit_code: Option<i32>,

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

        let cell = RendererCell::new(ActiveRenderer::Main(TuiMainScreen::with_options(
            terminal,
            Some(settings_manager.get_show_hardware_cursor()),
            Some(get_agent_dir()),
        )));
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
            .set_auto_compact_enabled(session.auto_compaction_enabled());
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

        let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (bash_tx, bash_rx) = tokio::sync::mpsc::unbounded_channel();

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
            todo_panel_container: Rc::new(RefCell::new(Container::new())),
            tasks_panel_container: Rc::new(RefCell::new(Container::new())),
            widget_container_above: Rc::new(RefCell::new(Container::new())),
            editor_container,
            widget_container_below: Rc::new(RefCell::new(Container::new())),
            footer_container,
            subagent_panel_container: Rc::new(RefCell::new(Container::new())),
            editor,
            footer,
            footer_data,
            keybindings,
            theme_controller,
            built_in_header: None,
            version: VERSION.to_owned(),
            is_initialized: false,
            active_status_indicator: None,
            working_message: None,
            working_visible: true,
            hidden_thinking_label: DEFAULT_HIDDEN_THINKING_LABEL.to_owned(),
            last_sigint_time: None,
            last_status_spacer: None,
            last_status_text: None,
            streaming_component: None,
            streaming_message: None,
            pending_tools: Vec::new(),
            bash_component: None,
            pending_bash: None,
            pending_bash_components: Vec::new(),
            bash_tx,
            bash_rx: Some(bash_rx),
            compaction_queued_messages: Vec::new(),
            escape_target: EscapeTarget::Default,
            tool_output_expanded: false,
            hide_thinking_block,
            output_pad,
            is_bash_mode: false,
            pending_user_inputs: VecDeque::new(),
            is_shutting_down: false,
            exit_code: None,
            ui_tx,
            ui_rx: Some(ui_rx),
            agent_tx,
            agent_rx: Some(agent_rx),
            agent_subscription: None,
        }
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
    // Convenience accessors (the TS getters)
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

    /// `async init()` (`interactive-mode.ts:848-1010`).
    ///
    /// Not ported yet and tracked with the later slices: `ensureTool` for fd/rg
    /// (autocomplete slice), the scoped-model startup line, the fullscreen
    /// layout root and the extension-free remainder of `rebindCurrentSession`.
    async fn init(&mut self) {
        if self.is_initialized {
            return;
        }

        self.mount();
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.setup_key_handlers();
        self.subscribe_to_agent();

        self.cell.start();
        self.is_initialized = true;

        self.theme_controller.apply_from_settings().await;
        self.build_header();
        self.ui.request_render();

        self.render_initial_messages();

        // Deviation (class 1): the theme watcher fires from its own thread, so
        // the callback posts a repaint into the loop instead of touching the
        // `!Send` core the way the TypeScript closure does.
        let tx = self.ui_tx.clone();
        on_theme_change(Arc::new(move || {
            let _ = tx.send(UiMessage::ThemeChanged);
        }));

        self.update_terminal_title();
    }

    /// `mountInteractiveTui` (`interactive-mode.ts:780-786`) with the component
    /// list `init` builds.
    fn mount(&mut self) {
        let children: [ComponentRef; 10] = [
            Rc::clone(&self.document_container) as ComponentRef,
            Rc::clone(&self.pending_messages_container) as ComponentRef,
            Rc::clone(&self.status_container) as ComponentRef,
            Rc::clone(&self.todo_panel_container) as ComponentRef,
            Rc::clone(&self.tasks_panel_container) as ComponentRef,
            Rc::clone(&self.widget_container_above) as ComponentRef,
            Rc::clone(&self.editor_container) as ComponentRef,
            Rc::clone(&self.widget_container_below) as ComponentRef,
            Rc::clone(&self.footer_container) as ComponentRef,
            Rc::clone(&self.subagent_panel_container) as ComponentRef,
        ];
        for child in children {
            self.ui.add_child(child);
        }
        self.show_idle_status();
    }

    /// The header block of `init` (`interactive-mode.ts:905-1009`).
    fn build_header(&mut self) {
        let settings = self.settings();
        if self.options.verbose || !settings.get_quiet_startup() {
            let logo = format!(
                "{}{}",
                theme().bold(&theme().fg(ThemeColor::Accent, APP_NAME)),
                theme().fg(ThemeColor::Dim, &format!(" v{}", self.version))
            );
            let expanded_instructions = [
                key_hint("app.interrupt", "to interrupt"),
                key_hint("app.clear", "to clear"),
                raw_key_hint(&format!("{} twice", key_text("app.clear")), "to exit"),
                key_hint("app.exit", "to exit (empty)"),
                key_hint("app.suspend", "to suspend"),
                key_hint("tui.editor.deleteToLineEnd", "to delete to end"),
                key_hint("app.mode.cycle", "to cycle mode"),
                key_hint("app.thinking.cycle", "to cycle thinking level"),
                raw_key_hint(
                    &format!(
                        "{}/{}",
                        key_text("app.model.cycleForward"),
                        key_text("app.model.cycleBackward")
                    ),
                    "to cycle models",
                ),
                key_hint("app.model.select", "to select model"),
                key_hint("app.tools.expand", "to expand tools"),
                key_hint("app.thinking.toggle", "to expand thinking"),
                key_hint("app.editor.external", "for external editor"),
                raw_key_hint("/", "for commands"),
                raw_key_hint("!", "to run bash"),
                raw_key_hint("!!", "to run bash (no context)"),
                key_hint("app.message.followUp", "to queue follow-up"),
                key_hint("app.message.dequeue", "to edit all queued messages"),
                key_hint(
                    "app.clipboard.pasteImage",
                    "to paste image (with text fallback)",
                ),
                raw_key_hint("drop files", "to attach"),
            ]
            .join("\n");
            let compact_instructions = [
                key_hint("app.interrupt", "interrupt"),
                raw_key_hint(
                    &format!("{}/{}", key_text("app.clear"), key_text("app.exit")),
                    "clear/exit",
                ),
                raw_key_hint("/", "commands"),
                raw_key_hint("!", "bash"),
                key_hint("app.tools.expand", "more"),
            ]
            .join(&theme().fg(ThemeColor::Muted, " · "));
            let compact_onboarding = theme().fg(
                ThemeColor::Dim,
                &format!(
                    "Press {} to show full startup help and loaded resources.",
                    key_text("app.tools.expand")
                ),
            );
            let onboarding = theme().fg(
                ThemeColor::Dim,
                "Notagent can explain its own features and look up its docs. Ask it how to use or extend Notagent.",
            );
            let header = Rc::new(RefCell::new(ExpandableText::new(
                format!("{logo}\n{compact_instructions}\n{compact_onboarding}\n\n{onboarding}"),
                format!("{logo}\n{expanded_instructions}\n\n{onboarding}"),
                self.tool_output_expanded,
                1,
            )));
            let mut container = self.header_container.borrow_mut();
            container.add_child(component_ref(Spacer::new(1)));
            container.add_child(Rc::clone(&header) as ComponentRef);
            container.add_child(component_ref(Spacer::new(1)));
            drop(container);
            self.built_in_header = Some(header);
        } else {
            self.header_container
                .borrow_mut()
                .add_child(component_ref(Text::new("", 0, 0)));
        }
    }

    /// `updateTerminalTitle` (`interactive-mode.ts:1011-1024`).
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

    /// `setupKeyHandlers` (`interactive-mode.ts:3003-3072`).
    ///
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
        for (action, message) in [
            ("app.clear", AppAction::Clear),
            ("app.message.followUp", AppAction::FollowUp),
            ("app.message.dequeue", AppAction::Dequeue),
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

    /// `subscribeToAgent` (`interactive-mode.ts:3307-3312`).
    ///
    /// The session emits from wherever the run happens to be; the events cross
    /// into the single-threaded UI over a channel and are handled in the loop.
    fn subscribe_to_agent(&mut self) {
        let tx = self.agent_tx.clone();
        self.agent_subscription = Some(self.session().subscribe(Arc::new(move |event| {
            let _ = tx.send(event);
        })));
    }

    /// `rebindCurrentSession` (`interactive-mode.ts:1935-1960`).
    ///
    /// Deviation (class 1): TypeScript hands the runtime a callback
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
        self.update_editor_border_color();
        self.update_terminal_title();
    }

    /// `renderCurrentSessionState` (`interactive-mode.ts:1970-1985`).
    fn render_current_session_state(&mut self) {
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

    /// `async run()` (`interactive-mode.ts:1025-1117`).
    ///
    /// The startup checks TypeScript fires off in the background (version
    /// check, package updates, tmux keyboard setup, the model-runtime refresh)
    /// belong to later slices and are noted in `PARITY.md`.
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

        if let Some(initial) = self.options.initial_message.take() {
            self.pending_user_inputs.push_back(initial);
        }
        for message in std::mem::take(&mut self.options.initial_messages) {
            self.pending_user_inputs.push_back(message);
        }

        let mut ui_rx = self.ui_rx.take().expect("run called once");
        let mut agent_rx = self.agent_rx.take().expect("init subscribed");
        let mut bash_rx = self.bash_rx.take().expect("run called once");
        let mut prompt: Option<PromptFuture> = None;

        loop {
            if let Some(code) = self.exit_code {
                return code;
            }
            if prompt.is_none()
                && let Some(text) = self.pending_user_inputs.pop_front()
            {
                let session = self.session();
                let images = std::mem::take(&mut self.options.initial_images);
                prompt = Some(Box::pin(async move {
                    session
                        .prompt(
                            &text,
                            PromptOptions {
                                images,
                                ..PromptOptions::default()
                            },
                        )
                        .await
                }));
            }

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
                    if let Err(message) = result {
                        self.show_error(&message);
                    }
                    None
                }
                result = async {
                    match bash.as_mut() {
                        Some(bash) => bash.run.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => bash.take().map(|bash| (bash, result)),
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
    ///
    /// TypeScript lets `setInterval` inside the loader drive the spinner; the
    /// port drives every time seam from the loop (interface request A-23).
    fn next_deadline(&self) -> Option<Instant> {
        let mut deadline: Option<Instant> = None;
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
        }
        if let Some(candidate) = self.editor.borrow().editor().autocomplete_deadline() {
            deadline = Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
        }
        deadline
    }

    /// One pass over everything a deadline was due for.
    fn tick(&mut self) {
        let mut needs_render = false;
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
            UiMessage::Action(AppAction::Escape) => self.handle_escape(),
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
            // `onChange` (`interactive-mode.ts:3060-3066`).
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

    /// `onSubmit` (`interactive-mode.ts:3113-3306`).
    ///
    /// The slash-command table, the bash mode and the compaction queue arrive
    /// with the slices that own them; what is wired here is the path a plain
    /// prompt takes.
    async fn handle_submit(&mut self, text: String) {
        let text = text.trim().to_owned();
        if text.is_empty() {
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
            let session = self.session();
            if let Err(message) = session
                .prompt(
                    &text,
                    PromptOptions {
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

    // ------------------------------------------------------------------
    // Bash mode
    // ------------------------------------------------------------------

    /// `handleBashCommand(command, excludeFromContext)`
    /// (`interactive-mode.ts:6571-6656`), minus the `user_bash` extension event
    /// and the result it could hand back (class 2).
    ///
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
                // TypeScript casts an incomplete literal
                // (`{ truncated: true, content } as TruncationResult`); the row
                // reads exactly those two fields, the rest is zero here
                // (deviation class 1).
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
        // `executeBash` records the result itself in the port; TypeScript only
        // records it on the extension path.
        let _ = (command, exclude_from_context);
        // `onSubmit` resets the bash border once the command is done.
        self.is_bash_mode = false;
        self.update_editor_border_color();
        self.ui.request_render();
    }

    /// `flushPendingBashComponents` (`interactive-mode.ts:6600-6610`).
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

    /// `getAllQueuedMessages` (`interactive-mode.ts:4430-4445`).
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

    /// `clearAllQueues` (`interactive-mode.ts:4447-4460`).
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

    /// `updatePendingMessagesDisplay` (`interactive-mode.ts:4462-4479`).
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

    /// `restoreQueuedMessagesToEditor` (`interactive-mode.ts:4481-4500`).
    fn restore_queued_messages_to_editor(&mut self, abort: bool) -> usize {
        let (steering, follow_up) = self.clear_all_queues();
        let mut all_queued = steering;
        all_queued.extend(follow_up);
        if all_queued.is_empty() {
            self.update_pending_messages_display();
            if abort {
                self.session().agent().abort();
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
            self.session().agent().abort();
        }
        all_queued.len()
    }

    /// `handleDequeue` (`interactive-mode.ts:4236-4243`).
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

    /// `handleFollowUp` (`interactive-mode.ts:4204-4234`).
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

    /// `queueCompactionMessage` (`interactive-mode.ts:4502-4508`).
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

    /// `flushCompactionQueue` (`interactive-mode.ts:4520-4598`).
    ///
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

    /// The command table of `onSubmit` (`interactive-mode.ts:3116-3225`), in
    /// the order TypeScript tests it. `true` means the text was a command and
    /// must not reach the model.
    ///
    /// The commands that open a selector (`/settings`, `/model`,
    /// `/scoped-models`, `/tasks`, `/fork`, `/clone`, `/tree`, `/trust`,
    /// `/login`, `/logout`, `/resume`) are recognised here and answered with a
    /// notice until the selector slice wires them; they must never fall through
    /// to the model, which is what the fall-through of an unknown `/word` does
    /// in TypeScript too.
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
            "/settings" => {
                self.selector_slice_notice("/settings");
                self.clear_editor_text();
            }
            "/scoped-models" => {
                self.clear_editor_text();
                self.selector_slice_notice("/scoped-models");
            }
            _ if text == "/model" || text.starts_with("/model ") => {
                self.clear_editor_text();
                self.selector_slice_notice("/model");
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
                self.selector_slice_notice("/tasks");
                self.clear_editor_text();
            }
            "/hotkeys" => {
                self.handle_hotkeys_command();
                self.clear_editor_text();
            }
            "/fork" => {
                self.selector_slice_notice("/fork");
                self.clear_editor_text();
            }
            "/clone" => {
                self.clear_editor_text();
                self.selector_slice_notice("/clone");
            }
            "/tree" => {
                self.selector_slice_notice("/tree");
                self.clear_editor_text();
            }
            "/trust" => {
                self.selector_slice_notice("/trust");
                self.clear_editor_text();
            }
            _ if text == "/login" || text.starts_with("/login ") => {
                self.clear_editor_text();
                self.selector_slice_notice("/login");
            }
            "/logout" => {
                self.selector_slice_notice("/logout");
                self.clear_editor_text();
            }
            "/new" => {
                self.clear_editor_text();
                self.handle_clear_command().await;
            }
            _ if text == "/compact" || text.starts_with("/compact ") => {
                let instructions = argument("/compact ");
                self.clear_editor_text();
                self.handle_compact_command(instructions.as_deref()).await;
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
                self.selector_slice_notice("/resume");
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

    /// The interim answer of a command whose selector is not wired yet.
    fn selector_slice_notice(&mut self, command: &str) {
        self.show_error(&format!(
            "{command} opens a selector, which lands with the next slice of plan task 13"
        ));
    }

    fn clear_editor_text(&mut self) {
        self.editor.borrow_mut().editor_mut().set_text("");
    }

    /// `handleExportCommand` (`interactive-mode.ts:6058-6072`).
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
    /// `core::export_html` (workstream B, plan task 16); the binding is here,
    /// where the TypeScript session method binds it.
    fn export_to_html(&self, output_path: Option<&str>) -> Result<String, String> {
        let session = self.session();
        session.with_session_manager(|manager| {
            crate::core::export_html::export_session_to_html(
                manager,
                None,
                crate::core::export_html::ExportOptions {
                    output_path: output_path.map(str::to_owned),
                    ..crate::core::export_html::ExportOptions::default()
                },
            )
            .map_err(|error| error.to_string())
        })
    }

    /// `handleImportCommand` (`interactive-mode.ts:6103-6145`).
    ///
    /// Remaining: the `MissingSessionCwdError` branch, which offers the
    /// fallback cwd and retries. `AgentSessionRuntime::import_from_jsonl`
    /// flattens its errors into a string, so the branch needs a typed error
    /// first; noted in `PARITY.md`.
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
            Err(message) => self.show_error(&format!("Failed to import session: {message}")),
        }
    }

    /// `handleShareCommand` (`interactive-mode.ts:6147-6239`) — the two `gh`
    /// calls live in `core::share` (interface request B-9), the loader, the
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
        // runner through the same select the TypeScript gets from `proc.kill()`.
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

    /// `handleCopyCommand` (`interactive-mode.ts:6241-6258`).
    fn handle_copy_command(&mut self, _flash_confirmation: bool) {
        let Some(text) = self.session().get_last_assistant_text() else {
            self.show_error("No agent messages to copy yet.");
            return;
        };
        match crate::utils::clipboard::copy_to_clipboard(&text) {
            Ok(()) => self.show_status("Copied last agent message to clipboard"),
            Err(message) => self.show_error(&message),
        }
    }

    /// `handleNameCommand` (`interactive-mode.ts:6260-6282`).
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

    /// `handleSessionCommand` (`interactive-mode.ts:6284-6345`).
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

    /// `handleChangelogCommand` (`interactive-mode.ts:6347-6369`).
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

    /// `handleHotkeysCommand` (`interactive-mode.ts:6382-6497`).
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

    /// `handleClearCommand` (`interactive-mode.ts:6499-6512`).
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

    /// `handleCompactCommand` (`interactive-mode.ts:6658-6664`).
    async fn handle_compact_command(&mut self, custom_instructions: Option<&str>) {
        // The result is reported through `compaction_end`; a failure here is the
        // same event with an error message.
        let _ = self.session().compact(custom_instructions).await;
    }

    /// `handleReloadCommand` (`interactive-mode.ts:5968-6056`).
    ///
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
        set_keybindings(self.keybindings.borrow().to_tui());
        if let Some(header) = self.built_in_header.clone() {
            header.borrow_mut().set_expanded(self.tool_output_expanded);
        }
        let _ = set_registered_themes(self.session().resource_loader().get_themes().0);
        self.theme_controller.apply_from_settings().await;
        self.apply_runtime_settings();
        if let Some(error) = self.runtime.services().model_runtime.get_error() {
            self.show_error(&format!("models.json error: {error}"));
        }
        self.show_status(
            "Reloaded keybindings, extensions, skills, prompts, themes, and context files",
        );

        {
            let mut editor_container = self.editor_container.borrow_mut();
            editor_container.clear();
            editor_container.add_child(Rc::clone(&self.editor) as ComponentRef);
        }
        self.ui
            .set_focus(Some(Rc::clone(&self.editor) as ComponentRef));
        self.ui.request_render();
    }

    /// `applyRuntimeSettings` (`interactive-mode.ts:1911-1933`).
    fn apply_runtime_settings(&mut self) {
        let settings = self.settings();
        self.ui.set_clear_on_shrink(settings.get_clear_on_shrink());
        self.ui
            .set_show_hardware_cursor(settings.get_show_hardware_cursor());
        {
            let mut editor = self.editor.borrow_mut();
            let editor = editor.editor_mut();
            editor.set_padding_x(settings.get_editor_padding_x() as usize);
            editor.set_autocomplete_max_visible(settings.get_autocomplete_max_visible() as usize);
        }
        {
            let mut footer = self.footer.borrow_mut();
            footer.set_session(Arc::clone(&self.session())
                as Arc<dyn crate::modes::interactive::components::footer::FooterSession>);
            footer.set_auto_compact_enabled(self.session().auto_compaction_enabled());
        }
        self.footer_data.set_cwd(&self.cwd());
        self.hide_thinking_block = settings.get_hide_thinking_block();
        self.output_pad = settings.get_output_pad() as usize;
        self.update_editor_border_color();
    }

    /// `rebuildChatFromMessages` (`interactive-mode.ts:4001-4004`).
    fn rebuild_chat_from_messages(&mut self) {
        self.chat_container.borrow_mut().clear();
        let entries = self
            .session()
            .with_session_manager(|manager| manager.build_context_entries());
        self.render_session_entries(&entries, false);
    }

    /// `handleDebugCommand` (`interactive-mode.ts:6514-6545`).
    ///
    /// Deviation (class 1): TypeScript asks the TUI for the rendered document
    /// (`this.ui.render(width)`); the port renders the mounted children itself,
    /// because the renderer keeps that pass private to the frame it writes.
    fn handle_debug_command(&mut self) {
        let width = self.ui.columns();
        let height = self.ui.rows();
        let lines: Vec<String> = self
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

    /// `handleArminSaysHi` (`interactive-mode.ts:6547-6551`).
    fn handle_armin_says_hi(&mut self) {
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(ArminComponent::new()));
        drop(chat);
        self.ui.request_render();
    }

    /// `handleDementedDelves` (`interactive-mode.ts:6553-6557`).
    fn handle_demented_delves(&mut self) {
        let mut chat = self.chat_container.borrow_mut();
        chat.add_child(component_ref(Spacer::new(1)));
        chat.add_child(component_ref(EarendilAnnouncementComponent::new()));
        drop(chat);
        self.ui.request_render();
    }

    /// `showExtensionConfirm(title, message)` (`interactive-mode.ts:2473-2480`)
    /// over the list selector `showExtensionSelector` uses.
    ///
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

    /// `handleCtrlC` (`interactive-mode.ts:4010-4018`).
    async fn handle_ctrl_c(&mut self) {
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

    /// `handleCtrlD` (`interactive-mode.ts:4020-4023`) — only reached on an
    /// empty editor, which `CustomEditor` enforces.
    async fn handle_ctrl_d(&mut self) {
        self.shutdown().await;
    }

    /// The escape branch of `setupKeyHandlers` (`interactive-mode.ts:3006-3033`).
    ///
    /// The double-escape action (`/tree` or `/fork`) needs the selectors and
    /// arrives with their slice.
    fn handle_escape(&mut self) {
        match self.escape_target {
            // While a compaction or a retry runs, Escape aborts that instead.
            EscapeTarget::Compaction => {
                self.session().abort_compaction();
                return;
            }
            EscapeTarget::Retry => {
                self.session().abort_retry();
                return;
            }
            EscapeTarget::Default => {}
        }
        if self.session().is_streaming() {
            self.restore_queued_messages_to_editor(true);
        } else if self.session().is_bash_running() {
            self.session().abort_bash();
        } else if self.is_bash_mode {
            self.clear_editor_text();
            self.is_bash_mode = false;
            self.update_editor_border_color();
        }
    }

    /// `shutdown()` (`interactive-mode.ts:4032-4071`) for the interactive quit.
    ///
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

    /// `formatResumeCommand` (`interactive-mode.ts:250-263`).
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

    /// `stop()` (`interactive-mode.ts:6668-6687`).
    pub fn stop(&mut self) {
        if self.settings().get_show_terminal_progress() {
            self.ui
                .with_terminal(|terminal| terminal.set_progress(false));
        }
        self.clear_status_indicator(None);
        self.theme_controller.disable_auto_sync();
        self.footer.borrow_mut().dispose();
        self.agent_subscription = None;
        if self.is_initialized {
            self.cell.stop(TuiStopOptions {
                preserve_screen: self.cell.mode() != TuiMode::Regular,
            });
            self.is_initialized = false;
        }
    }

    // ------------------------------------------------------------------
    // Session events
    // ------------------------------------------------------------------

    /// `handleEvent` (`interactive-mode.ts:3313-3652`).
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
                    self.streaming_component = Some(component);
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
                    for content in message.content.iter() {
                        if let notagent_ai::types::AssistantContent::ToolCall(call) = content {
                            match self.tool_component(&call.id) {
                                Some(component) => component
                                    .borrow_mut()
                                    .update_args(serde_json::Value::Object(call.arguments.clone())),
                                None => self.add_tool_component(
                                    &call.name,
                                    &call.id,
                                    serde_json::Value::Object(call.arguments.clone()),
                                ),
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
                        for (_, component) in self.pending_tools.iter() {
                            component.borrow_mut().set_args_complete();
                        }
                    }
                    self.streaming_component = None;
                    self.streaming_message = None;
                    self.footer.borrow_mut().invalidate();
                }
                self.ui.request_render();
            }
            AgentSessionEvent::Agent(AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            }) => {
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
                result,
                is_error,
                ..
            }) => {
                if let Some(component) = self.tool_component(&tool_call_id) {
                    component
                        .borrow_mut()
                        .update_result(tool_result(result, is_error), false);
                    self.pending_tools.retain(|(id, _)| id != &tool_call_id);
                    self.ui.request_render();
                }
            }
            AgentSessionEvent::AgentEnd { .. } => {
                if self.settings().get_show_terminal_progress() {
                    self.ui
                        .with_terminal(|terminal| terminal.set_progress(false));
                }
                self.clear_status_indicator(Some(StatusIndicatorKind::Working));
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
                self.show_status_indicator(StatusIndicator::compaction(compaction_status_reason(
                    reason,
                )));
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
                self.clear_status_indicator(Some(StatusIndicatorKind::Compaction));
                if aborted {
                    if reason == CompactionReason::Manual {
                        self.show_error("Compaction cancelled");
                    } else {
                        self.show_status("Auto-compaction cancelled");
                    }
                } else if let Some(result) = result {
                    self.chat_container.borrow_mut().clear();
                    self.rebuild_chat_from_messages();
                    self.add_message_to_chat(
                        &AgentMessage::CompactionSummary(create_compaction_summary_message(
                            &result.summary,
                            result.tokens_before,
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
            AgentSessionEvent::SessionInfoChanged { .. } => {
                self.update_terminal_title();
                self.footer.borrow_mut().invalidate();
                self.ui.request_render();
            }
            AgentSessionEvent::ThinkingLevelChanged { .. } => {
                self.footer.borrow_mut().invalidate();
                self.update_editor_border_color();
            }
            // Queueing, compaction, retries and the custom entries arrive with
            // the slices that own them.
            _ => {}
        }
    }

    fn tool_component(&self, tool_call_id: &str) -> Option<Rc<RefCell<ToolExecutionComponent>>> {
        self.pending_tools
            .iter()
            .find(|(id, _)| id == tool_call_id)
            .map(|(_, component)| Rc::clone(component))
    }

    fn add_tool_component(&mut self, tool_name: &str, tool_call_id: &str, args: serde_json::Value) {
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
        self.pending_tools
            .push((tool_call_id.to_owned(), component));
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

    /// `renderInitialMessages` (`interactive-mode.ts:3950-3966`).
    fn render_initial_messages(&mut self) {
        let entries = self
            .session()
            .with_session_manager(|manager| manager.build_context_entries());
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

    /// `renderSessionEntries` (`interactive-mode.ts:3908-3925`).
    fn render_session_entries(&mut self, entries: &[SessionEntry], populate_history: bool) {
        let mut items: Vec<AgentMessage> = Vec::new();
        for entry in entries {
            items.extend(session_entry_to_context_messages(entry));
        }
        self.render_session_items(&items, populate_history);
    }

    /// `renderSessionItems` (`interactive-mode.ts:3817-3906`).
    ///
    /// The cache-miss notices and the custom session entries belong to the
    /// slices that own them.
    fn render_session_items(&mut self, items: &[AgentMessage], populate_history: bool) {
        self.pending_tools.clear();
        let mut rendered_pending: Vec<(String, Rc<RefCell<ToolExecutionComponent>>)> = Vec::new();

        for item in items {
            match item {
                AgentMessage::Assistant(message) => {
                    self.add_message_to_chat(item, populate_history);
                    for content in message.content.iter() {
                        let notagent_ai::types::AssistantContent::ToolCall(call) = content else {
                            continue;
                        };
                        let component = Rc::new(RefCell::new(self.create_tool_component(
                            &call.name,
                            &call.id,
                            serde_json::Value::Object(call.arguments.clone()),
                        )));
                        component
                            .borrow_mut()
                            .set_expanded(self.tool_output_expanded);
                        self.chat_container
                            .borrow_mut()
                            .add_child(Rc::clone(&component) as ComponentRef);

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
                    }
                }
                message => self.add_message_to_chat(message, populate_history),
            }
        }

        self.pending_tools.extend(rendered_pending);
        self.ui.request_render();
    }

    /// `addMessageToChat` (`interactive-mode.ts:3710-3815`).
    fn add_message_to_chat(&mut self, message: &AgentMessage, populate_history: bool) {
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
                self.chat_container.borrow_mut().add_child(component_ref(
                    AssistantMessageComponent::new(
                        Some(message.clone()),
                        self.hide_thinking_block,
                        Some(get_markdown_theme()),
                        Some(self.hidden_thinking_label.clone()),
                        Some(self.output_pad),
                        Vec::new(),
                    ),
                ));
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
                if message.display {
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

    /// `renderProjectTrustWarningIfNeeded` (`interactive-mode.ts:3967-3986`).
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

    /// `showStatus` (`interactive-mode.ts:3668-3686`).
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

    /// `showError` (`interactive-mode.ts:4367-4371`).
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

    /// `showWarning` (`interactive-mode.ts:4373-4377`).
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

    /// `clearEditor` (`interactive-mode.ts:4362-4365`).
    fn clear_editor(&mut self) {
        self.editor.borrow_mut().editor_mut().set_text("");
        self.ui.request_render();
    }

    /// `updateEditorBorderColor` (`interactive-mode.ts:4245-4258`).
    fn update_editor_border_color(&mut self) {
        let color = if self.is_bash_mode {
            theme().get_bash_mode_border_color()
        } else {
            theme().get_thinking_border_color(self.session().thinking_level())
        };
        self.editor.borrow_mut().editor_mut().border_color = color;
        self.ui.request_render();
    }

    /// `showStatusIndicator` (`interactive-mode.ts:2062-2067`).
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

    /// `clearStatusIndicator` (`interactive-mode.ts:2069-2080`).
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

/// `getPathCommandArgument(text, command)` (`interactive-mode.ts:6074-6101`).
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

/// `quoteIfNeeded` (`interactive-mode.ts:243-249`).
fn quote_if_needed(value: &str) -> String {
    let needs_quotes = value
        .chars()
        .any(|character| !(character.is_ascii_alphanumeric() || "_-./~:@".contains(character)));
    if !value.is_empty() && !needs_quotes {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `getUserMessageText` (`interactive-mode.ts:3653-3659`).
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
