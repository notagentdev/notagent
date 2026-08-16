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
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::terminal::{ProcessTerminal, Terminal, TerminalPump};
use notagent_tui::tui::{
    Component, ComponentRef, Container, RenderLoop, TuiCore, TuiStopOptions, component_ref,
};
use notagent_tui::tui_main_screen::TuiMainScreen;

use crate::config::{APP_NAME, APP_TITLE, CONFIG_DIR_NAME, VERSION, get_agent_dir};
use crate::core::agent_session::{
    AgentSession, AgentSessionEvent, PromptOptions, parse_skill_block,
};
use crate::core::agent_session_runtime::AgentSessionRuntime;
use crate::core::keybindings::KeybindingsManager;
use crate::core::session_manager::{SessionEntry, session_entry_to_context_messages};
use crate::core::settings_manager::TuiMode;
use crate::core::trust_manager::has_trust_requiring_project_resources;
use crate::modes::interactive::components::assistant_message::AssistantMessageComponent;
use crate::modes::interactive::components::branch_summary_message::BranchSummaryMessageComponent;
use crate::modes::interactive::components::compaction_summary_message::CompactionSummaryMessageComponent;
use crate::modes::interactive::components::custom_editor::CustomEditor;
use crate::modes::interactive::components::custom_message::CustomMessageComponent;
use crate::modes::interactive::components::footer::FooterComponent;
use crate::modes::interactive::components::keybinding_hints::{key_hint, key_text, raw_key_hint};
use crate::modes::interactive::components::skill_invocation_message::SkillInvocationMessageComponent;
use crate::modes::interactive::components::status_indicator::{
    IdleStatus, StatusIndicator, StatusIndicatorKind,
};
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
            tool_output_expanded: false,
            hide_thinking_block,
            output_pad,
            is_bash_mode: false,
            pending_user_inputs: VecDeque::new(),
            is_shutting_down: false,
            exit_code: None,
            ui_tx,
            ui_rx: Some(ui_rx),
            agent_rx: None,
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
        let tx = self.ui_tx.clone();
        editor.on_action(
            "app.clear",
            Box::new(move || {
                let _ = tx.send(UiMessage::Action(AppAction::Clear));
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

    /// `subscribeToAgent` (`interactive-mode.ts:3307-3312`).
    ///
    /// The session emits from wherever the run happens to be; the events cross
    /// into the single-threaded UI over a channel and are handled in the loop.
    fn subscribe_to_agent(&mut self) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = self.session().subscribe(Arc::new(move |event| {
            let _ = tx.send(event);
        }));
        self.agent_rx = Some(rx);
        self.agent_subscription = Some(handle);
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

            let deadline = self.next_deadline();
            tokio::select! {
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
                }
                message = ui_rx.recv() => {
                    match message {
                        Some(message) => self.handle_ui_message(message).await,
                        None => return 0,
                    }
                }
                event = agent_rx.recv() => {
                    match event {
                        Some(event) => self.handle_event(event).await,
                        None => return 0,
                    }
                }
                () = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                        None => std::future::pending().await,
                    }
                } => self.tick(),
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

        // If streaming, `prompt()` with steering behaviour queues the message.
        if self.session().is_streaming() {
            self.editor.borrow_mut().editor_mut().add_to_history(&text);
            self.editor.borrow_mut().editor_mut().set_text("");
            let session = self.session();
            if let Err(message) = session
                .prompt(
                    &text,
                    PromptOptions {
                        streaming_behavior: Some(crate::core::agent_session::QueueBehavior::Steer),
                        ..PromptOptions::default()
                    },
                )
                .await
            {
                self.show_error(&message);
            }
            self.ui.request_render();
            return;
        }

        self.pending_user_inputs.push_back(text.clone());
        self.editor.borrow_mut().editor_mut().add_to_history(&text);
        self.editor.borrow_mut().editor_mut().set_text("");
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

    /// The escape branch of `setupKeyHandlers` that this slice covers: abort a
    /// running turn. The bash and double-escape branches follow with their
    /// slices.
    fn handle_escape(&mut self) {
        if self.session().is_streaming() {
            self.session().agent().abort();
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
