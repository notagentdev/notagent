use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use notagent_ai::models::{ModelsRefreshOptions, models_are_equal};
use notagent_ai::types::Model;
use notagent_tui::components::input::Input;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::fuzzy::fuzzy_filter;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, Focusable, Line, component_ref};
use tokio_util::sync::CancellationToken;

use crate::core::model_resolver::ScopedModel;
use crate::core::model_runtime::ModelRuntime;
use crate::core::settings_manager::SettingsManager;
use crate::modes::interactive::model_search::{ModelSearchItem, get_model_selector_search_text};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_hint;

/// `timeoutMs` of `refreshModels`.
const REFRESH_TIMEOUT: Duration = Duration::from_millis(15_000);

/// `maxVisible` of `updateList`.
const MAX_VISIBLE: usize = 10;

/// `ModelItem`
#[derive(Clone)]
struct ModelItem {
    provider: String,
    id: String,
    model: Model,
}

/// `ModelScope`
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelScope {
    /// `"all"`
    All,
    /// `"scoped"`
    Scoped,
}

/// `String.prototype.localeCompare` for the ASCII provider ids sorted here:
/// case-insensitive, with lowercase winning a tie.
fn locale_compare(a: &str, b: &str) -> Ordering {
    let folded = a.to_lowercase().cmp(&b.to_lowercase());
    if folded != Ordering::Equal {
        return folded;
    }
    b.cmp(a)
}

/// What one `refreshModels` pass learned from the runtime.
/// from the constructor with `void`. A Rust constructor cannot own a task that
/// later mutates `&mut self`, so the pass is split: [`ModelSelectorComponent::refresh_models`]
/// returns the future that talks to the runtime (`Send`, so it can be spawned),
/// and [`ModelSelectorComponent::apply_refresh`] performs the half that touches
/// its `await`.
#[derive(Debug, Clone)]
pub struct ModelRefreshOutcome {
    /// `result.aborted`
    pub aborted: bool,
    /// The `timedOut` flag the timer sets before it aborts.
    pub timed_out: bool,
    /// `[...result.errors.keys()]`
    /// preserved the order the providers failed in.
    pub failed_providers: Vec<String>,
}

/// Component that renders a model selector with search
pub struct ModelSelectorComponent {
    container: Container,
    search_input: Rc<RefCell<Input>>,
    /// `this.searchInput.onSubmit` — see [`ModelSelectorComponent::handle_input`].
    submitted: Rc<Cell<bool>>,

    // Focusable implementation - propagate to searchInput for IME cursor positioning
    focused: bool,
    list_container: Rc<RefCell<Container>>,
    all_models: Vec<ModelItem>,
    scoped_model_items: Vec<ModelItem>,
    active_models: Vec<ModelItem>,
    filtered_models: Vec<ModelItem>,
    selected_index: usize,
    current_model: Option<Model>,
    settings_manager: Arc<SettingsManager>,
    model_runtime: Arc<ModelRuntime>,
    on_select_callback: Box<dyn FnMut(Model)>,
    on_cancel_callback: Box<dyn FnMut()>,
    error_message: Option<String>,
    refresh_status_message: String,
    refresh_status_success: bool,
    /// `this.tui.requestRender()` — the TUI is reached through the callback the
    /// caller injects (class 1, as in `session_selector`).
    request_render: Rc<dyn Fn()>,
    scoped_models: Vec<ScopedModel>,
    scope: ModelScope,
    scope_text: Option<Rc<RefCell<Text>>>,
    scope_hint_text: Option<Rc<RefCell<Text>>>,
    refresh_abort: CancellationToken,
    closed: bool,
}

impl ModelSelectorComponent {
    /// see [`ModelRefreshOutcome`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_render: Rc<dyn Fn()>,
        current_model: Option<Model>,
        settings_manager: Arc<SettingsManager>,
        model_runtime: Arc<ModelRuntime>,
        scoped_models: Vec<ScopedModel>,
        on_select: Box<dyn FnMut(Model)>,
        on_cancel: Box<dyn FnMut()>,
        initial_search_input: Option<&str>,
    ) -> Self {
        let theme_instance = theme();
        let scope = if scoped_models.is_empty() {
            ModelScope::All
        } else {
            ModelScope::Scoped
        };

        let mut container = Container::new();

        // Add top border
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));

        // Add hint about model filtering
        let (scope_text, scope_hint_text) = if scoped_models.is_empty() {
            let hint_text =
                "Only showing models from configured providers. Use /login to add providers.";
            container.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Text, hint_text),
                0,
                0,
            )));
            (None, None)
        } else {
            let scope_text = Rc::new(RefCell::new(Text::new(scope_line(scope), 0, 0)));
            container.add_child(Rc::clone(&scope_text) as ComponentRef);
            let scope_hint_text = Rc::new(RefCell::new(Text::new(scope_hint_line(), 0, 0)));
            container.add_child(Rc::clone(&scope_hint_text) as ComponentRef);
            (Some(scope_text), Some(scope_hint_text))
        };
        container.add_child(component_ref(Spacer::new(1)));

        // Create search input
        let search_input = Rc::new(RefCell::new(Input::new()));
        if let Some(initial) = initial_search_input.filter(|initial| !initial.is_empty()) {
            search_input.borrow_mut().set_value(initial);
        }
        let submitted = Rc::new(Cell::new(false));
        {
            let flag = Rc::clone(&submitted);
            search_input.borrow_mut().on_submit = Some(Box::new(move |_value| flag.set(true)));
        }
        container.add_child(Rc::clone(&search_input) as ComponentRef);

        container.add_child(component_ref(Spacer::new(1)));

        // Create list container
        let list_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&list_container) as ComponentRef);

        container.add_child(component_ref(Spacer::new(1)));

        // Add bottom border
        container.add_child(component_ref(DynamicBorder::new(None)));

        let mut selector = Self {
            container,
            search_input,
            submitted,
            focused: false,
            list_container,
            all_models: Vec::new(),
            scoped_model_items: Vec::new(),
            active_models: Vec::new(),
            filtered_models: Vec::new(),
            selected_index: 0,
            current_model,
            settings_manager,
            model_runtime,
            on_select_callback: on_select,
            on_cancel_callback: on_cancel,
            error_message: None,
            refresh_status_message: "Refreshing model catalogs…".to_owned(),
            refresh_status_success: false,
            request_render,
            scoped_models,
            scope,
            scope_text,
            scope_hint_text,
            refresh_abort: CancellationToken::new(),
            closed: false,
        };

        // Render the current snapshot immediately, then refresh in the background.
        selector.load_models_from_snapshot();
        match initial_search_input.filter(|initial| !initial.is_empty()) {
            Some(initial) => selector.filter_models(initial),
            None => selector.update_list(),
        }
        (selector.request_render)();
        selector
    }

    fn load_models_from_snapshot(&mut self) {
        let models: Vec<ModelItem> = self
            .model_runtime
            .get_available_snapshot()
            .into_iter()
            .map(|model| ModelItem {
                provider: model.provider.clone(),
                id: model.id.clone(),
                model,
            })
            .collect();
        self.all_models = self.sort_models(models);
        self.scoped_models = self
            .scoped_models
            .iter()
            .map(|scoped| {
                match self
                    .model_runtime
                    .get_model(&scoped.model.provider, &scoped.model.id)
                {
                    Some(refreshed) => ScopedModel {
                        model: refreshed,
                        thinking_level: scoped.thinking_level,
                    },
                    None => scoped.clone(),
                }
            })
            .collect();
        self.scoped_model_items = self
            .scoped_models
            .iter()
            .map(|scoped| ModelItem {
                provider: scoped.model.provider.clone(),
                id: scoped.model.id.clone(),
                model: scoped.model.clone(),
            })
            .collect();
        self.active_models = match self.scope {
            ModelScope::Scoped => self.scoped_model_items.clone(),
            ModelScope::All => self.all_models.clone(),
        };
        self.filtered_models = self.active_models.clone();
        let current_index = self
            .filtered_models
            .iter()
            .position(|item| models_are_equal(self.current_model.as_ref(), Some(&item.model)));
        self.selected_index = match current_index {
            Some(index) => index,
            None => self
                .selected_index
                .min(self.filtered_models.len().saturating_sub(1)),
        };
    }

    /// The half of `refreshModels` that talks to the runtime.
    /// hand its result to [`ModelSelectorComponent::apply_refresh`]. The 15 s
    /// timer that aborts the shared signal lives inside it, so a `dispose()`
    pub fn refresh_models(&self) -> impl Future<Output = ModelRefreshOutcome> + Send + 'static {
        let runtime = Arc::clone(&self.model_runtime);
        let signal = self.refresh_abort.clone();
        async move {
            let mut timed_out = false;
            let options = ModelsRefreshOptions {
                signal: Some(signal.clone()),
                ..ModelsRefreshOptions::default()
            };
            let refresh = runtime.refresh(options);
            tokio::pin!(refresh);
            let result = tokio::select! {
                result = &mut refresh => result,
                () = tokio::time::sleep(REFRESH_TIMEOUT) => {
                    timed_out = true;
                    signal.cancel();
                    refresh.await
                }
            };
            ModelRefreshOutcome {
                aborted: result.aborted,
                timed_out,
                failed_providers: result.errors.into_keys().collect(),
            }
        }
    }

    /// The half of `refreshModels` that runs after its `await`.
    pub fn apply_refresh(&mut self, outcome: ModelRefreshOutcome) {
        if self.closed {
            return;
        }
        self.refresh_status_message = String::new();
        if outcome.aborted && outcome.timed_out {
            self.error_message = Some("Model refresh timed out; showing cached models.".to_owned());
        } else if outcome.failed_providers.len() == 1 {
            self.error_message = Some(format!(
                "Could not refresh {}; showing cached models.",
                outcome.failed_providers[0]
            ));
        } else if outcome.failed_providers.len() > 1 {
            self.error_message = Some(format!(
                "Could not refresh {} model catalogs ({}); showing cached models.",
                outcome.failed_providers.len(),
                outcome.failed_providers.join(", ")
            ));
        } else {
            self.error_message = self.model_runtime.get_error();
            if self.error_message.is_none() {
                self.refresh_status_message = "Model catalogs refreshed.".to_owned();
                self.refresh_status_success = true;
            }
        }
        self.load_models_from_snapshot();
        let query = self.search_input.borrow().get_value().to_owned();
        self.filter_models(&query);
        (self.request_render)();
    }

    /// `dispose()`
    pub fn dispose(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.refresh_abort.cancel();
    }

    fn sort_models(&self, models: Vec<ModelItem>) -> Vec<ModelItem> {
        let mut sorted = models;
        // Sort: current model first, then by provider
        sorted.sort_by(|a, b| {
            let a_is_current = models_are_equal(self.current_model.as_ref(), Some(&a.model));
            let b_is_current = models_are_equal(self.current_model.as_ref(), Some(&b.model));
            if a_is_current && !b_is_current {
                return Ordering::Less;
            }
            if !a_is_current && b_is_current {
                return Ordering::Greater;
            }
            locale_compare(&a.provider, &b.provider)
        });
        sorted
    }

    fn set_scope(&mut self, scope: ModelScope) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        self.active_models = match self.scope {
            ModelScope::Scoped => self.scoped_model_items.clone(),
            ModelScope::All => self.all_models.clone(),
        };
        let current_index = self
            .active_models
            .iter()
            .position(|item| models_are_equal(self.current_model.as_ref(), Some(&item.model)));
        self.selected_index = current_index.unwrap_or(0);
        let query = self.search_input.borrow().get_value().to_owned();
        self.filter_models(&query);
        if let Some(text) = self.scope_text.as_ref() {
            text.borrow_mut().set_text(scope_line(self.scope));
        }
    }

    fn filter_models(&mut self, query: &str) {
        self.filtered_models = if query.is_empty() {
            self.active_models.clone()
        } else {
            fuzzy_filter(&self.active_models, query, |item| {
                get_model_selector_search_text(&ModelSearchItem::new(
                    item.id.clone(),
                    item.provider.clone(),
                    Some(item.model.name.clone()),
                ))
            })
        };
        // When filtering by a query, move the selector to the top row so the best
        // match is highlighted. When the query is cleared, keep the current position
        // clamped to the (restored) list length.
        self.selected_index = if query.is_empty() {
            self.selected_index
                .min(self.filtered_models.len().saturating_sub(1))
        } else {
            0
        };
        self.update_list();
    }

    fn update_list(&mut self) {
        let theme_instance = theme();
        let mut list_container = self.list_container.borrow_mut();
        list_container.clear();

        let start_index = (self.selected_index as isize - (MAX_VISIBLE / 2) as isize)
            .min(self.filtered_models.len() as isize - MAX_VISIBLE as isize)
            .max(0) as usize;
        let end_index = (start_index + MAX_VISIBLE).min(self.filtered_models.len());

        // Show visible slice of filtered models
        for index in start_index..end_index {
            let Some(item) = self.filtered_models.get(index) else {
                continue;
            };

            let is_selected = index == self.selected_index;
            let is_current = models_are_equal(self.current_model.as_ref(), Some(&item.model));

            let provider_badge =
                theme_instance.fg(ThemeColor::Muted, &format!("[{}]", item.provider));
            let checkmark = if is_current {
                theme_instance.fg(ThemeColor::Success, " ✓")
            } else {
                String::new()
            };
            let line = if is_selected {
                let prefix = theme_instance.fg(ThemeColor::Accent, "→ ");
                format!(
                    "{} {provider_badge}{checkmark}",
                    prefix + &theme_instance.fg(ThemeColor::Accent, &item.id)
                )
            } else {
                format!("  {} {provider_badge}{checkmark}", item.id)
            };

            list_container.add_child(component_ref(Text::new(line, 0, 0)));
        }

        // Add scroll indicator if needed
        if start_index > 0 || end_index < self.filtered_models.len() {
            let scroll_info = theme_instance.fg(
                ThemeColor::Muted,
                &format!(
                    "  ({}/{})",
                    self.selected_index + 1,
                    self.filtered_models.len()
                ),
            );
            list_container.add_child(component_ref(Text::new(scroll_info, 0, 0)));
        }

        // Show error message or "no results" if empty
        if let Some(error_message) = self.error_message.as_ref() {
            // Show error in red
            for line in error_message.split('\n') {
                list_container.add_child(component_ref(Text::new(
                    theme_instance.fg(ThemeColor::Error, line),
                    0,
                    0,
                )));
            }
        } else if self.filtered_models.is_empty() {
            list_container.add_child(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Muted, "  No matching models"),
                0,
                0,
            )));
        } else {
            let selected = &self.filtered_models[self.selected_index];
            list_container.add_child(component_ref(Spacer::new(1)));
            list_container.add_child(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Muted,
                    &format!("  Model Name: {}", selected.model.name),
                ),
                0,
                0,
            )));
        }
        if !self.refresh_status_message.is_empty() {
            list_container.add_child(component_ref(Spacer::new(1)));
            list_container.add_child(component_ref(Text::new(
                theme_instance.fg(
                    if self.refresh_status_success {
                        ThemeColor::Success
                    } else {
                        ThemeColor::Muted
                    },
                    &format!("  {}", self.refresh_status_message),
                ),
                0,
                0,
            )));
        }
    }

    fn handle_select(&mut self, model: Model) {
        self.dispose();
        // Save as new default
        self.settings_manager
            .set_default_model_and_provider(&model.provider, &model.id);
        (self.on_select_callback)(model);
    }

    /// `getSearchInput()`
    pub fn search_input(&self) -> Rc<RefCell<Input>> {
        Rc::clone(&self.search_input)
    }
}

/// `getScopeText()`
fn scope_line(scope: ModelScope) -> String {
    let theme_instance = theme();
    let all_text = theme_instance.fg(
        if scope == ModelScope::All {
            ThemeColor::Accent
        } else {
            ThemeColor::Muted
        },
        "all",
    );
    let scoped_text = theme_instance.fg(
        if scope == ModelScope::Scoped {
            ThemeColor::Accent
        } else {
            ThemeColor::Muted
        },
        "scoped",
    );
    format!(
        "{}{all_text}{}{scoped_text}",
        theme_instance.fg(ThemeColor::Muted, "Scope: "),
        theme_instance.fg(ThemeColor::Muted, " | ")
    )
}

/// `getScopeHintText()`
fn scope_hint_line() -> String {
    key_hint("tui.input.tab", "scope") + &theme().fg(ThemeColor::Muted, " (all/scoped)")
}

impl Component for ModelSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }

    fn handle_input(&mut self, data: &str) {
        if keybindings_match(data, "tui.input.tab") {
            if !self.scoped_model_items.is_empty() {
                let next_scope = match self.scope {
                    ModelScope::All => ModelScope::Scoped,
                    ModelScope::Scoped => ModelScope::All,
                };
                self.set_scope(next_scope);
                if let Some(text) = self.scope_hint_text.as_ref() {
                    text.borrow_mut().set_text(scope_hint_line());
                }
            }
            return;
        }
        // Up arrow - wrap to bottom when at top
        if keybindings_match(data, "tui.select.up") {
            if self.filtered_models.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == 0 {
                self.filtered_models.len() - 1
            } else {
                self.selected_index - 1
            };
            self.update_list();
        }
        // Down arrow - wrap to top when at bottom
        else if keybindings_match(data, "tui.select.down") {
            if self.filtered_models.is_empty() {
                return;
            }
            self.selected_index = if self.selected_index == self.filtered_models.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
            self.update_list();
        }
        // Enter
        else if keybindings_match(data, "tui.select.confirm") {
            if let Some(selected) = self.filtered_models.get(self.selected_index) {
                let model = selected.model.clone();
                self.handle_select(model);
            }
        }
        // Escape or Ctrl+C
        else if keybindings_match(data, "tui.select.cancel") {
            self.dispose();
            (self.on_cancel_callback)();
        }
        // Pass everything else to search input
        else {
            self.submitted.set(false);
            self.search_input.borrow_mut().handle_input(data);
            // `this.searchInput.onSubmit` — Enter on the search input selects the
            // first filtered item. The callback cannot reach back into the
            // component, so it raises a flag the caller reads (class 1, as in
            // `filterModels` below.
            if self.submitted.replace(false)
                && let Some(selected) = self.filtered_models.get(self.selected_index)
            {
                let model = selected.model.clone();
                self.handle_select(model);
            }
            let query = self.search_input.borrow().get_value().to_owned();
            self.filter_models(&query);
        }
    }
}

impl Focusable for ModelSelectorComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.borrow_mut().set_focused(focused);
    }
}
