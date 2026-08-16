//! 1:1 port of `packages/coding-agent/src/extensions/llama/ui.ts` (542 LOC).
//!
//! The `/llama` model manager: the model list of a llama.cpp router, the
//! confirmation and choice dialogs it opens, the Hugging Face search and the
//! progress view of a load or a download.
//!
//! Deviation (class 1, structural): TypeScript hands every dialog back as a
//! promise the extension's flow awaits. A component here cannot resolve a
//! promise from inside `handle_input`, so every dialog gets a sequence number
//! and reports its answer over a channel; the flow ([`super::super::llama_command`])
//! awaits the answer whose sequence number it just installed. Answers of a
//! dialog that has already been replaced are therefore ignored instead of
//! resolving a promise nobody holds any more — the same effect TypeScript gets
//! by dropping the promise in `setContent`.
//!
//! Deviation (class 1): `tui`, `theme` and `keybindings` are not constructor
//! arguments — the port reads the process-wide theme and keybindings the same
//! way every other component here does, and renders through the
//! `request_render` callback of the mode.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use notagent_tui::components::input::Input;
use notagent_tui::components::select_list::{
    SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::fuzzy::fuzzy_filter;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, ComponentRef, Container, Focusable, component_ref};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::core::llama::client::{
    LLAMA_STATUS_LOADED, LLAMA_STATUS_SLEEPING, LLAMA_STATUS_UNLOADED, LlamaModelInfo,
    LlamaProgress,
};
use crate::core::llama::huggingface::HuggingFaceModel;
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_hint;

/// `DOWNLOAD_VALUE`
const DOWNLOAD_VALUE: &str = "\0download";

/// `maxVisible` of `HuggingFaceSearch.updateResults`.
const MAX_VISIBLE: usize = 10;

/// The 500 ms debounce of `scheduleSearch`.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(500);

/// `status` before the first two characters are typed.
const STATUS_TYPE_MORE: &str = "Type at least 2 characters";
/// `status` while a request is in flight.
const STATUS_SEARCHING: &str = "Searching Hugging Face…";
/// `status` of an empty result.
const STATUS_NO_MODELS: &str = "No GGUF models found";

/// `LlamaManagerAction`
#[derive(Debug, Clone, PartialEq)]
pub enum LlamaManagerAction {
    /// `{ type: "model"; model }`
    Model(Box<LlamaModelInfo>),
    /// `{ type: "download" }`
    Download,
    /// `{ type: "close" }`
    Close,
}

/// One answer of the manager UI, tagged with the dialog that produced it.
#[derive(Debug, Clone, PartialEq)]
pub enum LlamaAnswer {
    /// `showModels` resolved.
    Models(LlamaManagerAction),
    /// `select` resolved — `None` is the cancelled promise.
    Select(Option<String>),
    /// `searchModels` resolved — `None` is the cancelled promise.
    Search(Option<String>),
    /// The promise of `progress` resolved: the user asked to stop.
    ProgressStop,
}

/// The channel every dialog answers on: `(sequence number, answer)`.
pub type LlamaAnswers = UnboundedSender<(u64, LlamaAnswer)>;

/// `interface ProgressState extends LlamaProgress`
#[derive(Debug, Clone, Default)]
pub struct ProgressState {
    pub title: String,
    pub model: String,
    pub message: String,
    pub ratio: Option<f64>,
    pub detail: Option<String>,
}

impl ProgressState {
    /// `Object.assign(state, progress)`.
    ///
    /// Deviation (class 1): `Object.assign` distinguishes an absent key from an
    /// explicit `undefined`, a Rust struct cannot. All three fields are copied,
    /// which matches every sequence the client produces: `parseLoadProgress`
    /// and `parseDownloadProgress` always carry the keys they may clear, and
    /// the two key-less emissions (`{ message: "Loading model" }` and
    /// `{ message: "Downloading model" }`) are the first report of their run,
    /// where there is no ratio or detail to keep.
    pub fn apply(&mut self, progress: LlamaProgress) {
        self.message = progress.message;
        self.ratio = progress.ratio;
        self.detail = progress.detail;
    }
}

/// `String.prototype.localeCompare` for the ASCII model ids sorted here:
/// case-insensitive, with lowercase winning a tie.
fn locale_compare(a: &str, b: &str) -> Ordering {
    let folded = a.to_lowercase().cmp(&b.to_lowercase());
    if folded != Ordering::Equal {
        return folded;
    }
    b.cmp(a)
}

/// `Number(context)` of a `--ctx-size` argument: JavaScript's `Number()` on a
/// trimmed string, which accepts an empty string as `0` and hex/exponent forms.
fn js_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return u64::from_str_radix(hex, 16).ok().map(|value| value as f64);
    }
    trimmed.parse::<f64>().ok()
}

/// `contextLabel(model)`
fn context_label(model: &LlamaModelInfo) -> Option<String> {
    let format = |value: f64| -> String {
        if value >= 1000.0 {
            format!(
                "{}k",
                notagent_ai::utils::js_number::to_js_string((value / 1000.0).round())
            )
        } else {
            notagent_ai::utils::js_number::to_js_string(value)
        }
    };
    let context = model
        .meta
        .as_ref()
        .and_then(|meta| meta.n_ctx.or(meta.n_ctx_train));
    // `if (context)` — zero is falsy and falls through to the arguments.
    if let Some(context) = context.filter(|context| *context != 0) {
        return Some(format(context as f64));
    }
    let args = model.status.args.clone().unwrap_or_default();
    for index in 0..args.len().saturating_sub(1) {
        let flag = args[index].as_str();
        if flag != "--ctx-size" && flag != "-c" && flag != "-ctx" {
            continue;
        }
        let value = js_number(&args[index + 1])?;
        if value.is_finite() && value > 0.0 {
            return Some(format(value));
        }
        return None;
    }
    None
}

/// `modelDescription(model)`
fn model_description(model: &LlamaModelInfo) -> String {
    let mut details: Vec<String> = Vec::new();
    let loaded =
        model.status.value == LLAMA_STATUS_LOADED || model.status.value == LLAMA_STATUS_SLEEPING;
    if loaded {
        details.push("loaded".to_owned());
    } else if model.status.value != LLAMA_STATUS_UNLOADED {
        details.push(model.status.value.clone());
    }
    if loaded && let Some(context) = context_label(model) {
        details.push(format!("{context} context"));
    }
    details.join(" · ")
}

/// `selectTheme(theme)`
fn select_theme() -> SelectListTheme {
    SelectListTheme {
        selected_prefix: Rc::new(|text: &str| theme().fg(ThemeColor::Accent, text)),
        selected_text: Rc::new(|text: &str| theme().fg(ThemeColor::Accent, text)),
        description: Rc::new(|text: &str| theme().fg(ThemeColor::Muted, text)),
        scroll_info: Rc::new(|text: &str| theme().fg(ThemeColor::Dim, text)),
        no_match: Rc::new(|text: &str| theme().fg(ThemeColor::Warning, text)),
    }
}

/// `frame(theme, title, body, footer)`
fn frame(title: &str, body: Vec<ComponentRef>, footer: Option<&str>) -> Container {
    let accent = || {
        component_ref(DynamicBorder::new(Some(Rc::new(|text: &str| {
            theme().fg(ThemeColor::Accent, text)
        }))))
    };
    let mut container = Container::new();
    container.add_child(accent());
    let theme_instance = theme();
    container.add_child(component_ref(Text::new(
        theme_instance.fg(ThemeColor::Accent, &theme_instance.bold(title)),
        1,
        0,
    )));
    for child in body {
        container.add_child(child);
    }
    if let Some(footer) = footer {
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(Text::new(
            theme_instance.fg(ThemeColor::Dim, footer),
            1,
            0,
        )));
    }
    container.add_child(accent());
    container
}

/// `compactCount(value)`
fn compact_count(value: f64) -> String {
    if value >= 1_000_000.0 {
        return format!(
            "{}M",
            crate::core::llama::client::to_fixed(
                value / 1_000_000.0,
                if value < 10_000_000.0 { 1 } else { 0 }
            )
        );
    }
    if value >= 1_000.0 {
        return format!(
            "{}k",
            crate::core::llama::client::to_fixed(
                value / 1_000.0,
                if value < 100_000.0 { 1 } else { 0 }
            )
        );
    }
    notagent_ai::utils::js_number::to_js_string(value)
}

/// `/^[^/\s]+\/[^:\s]+(?::[^\s:]+)?$/u.test(query)`
fn is_exact_repository(query: &str) -> bool {
    let Some(slash) = query.find('/') else {
        return false;
    };
    let owner = &query[..slash];
    let rest = &query[slash + 1..];
    if owner.is_empty() || owner.chars().any(char::is_whitespace) {
        return false;
    }
    // `[^:\s]+` followed by an optional `:[^\s:]+`; the regex is anchored, so
    // at most one colon may appear and neither part may be empty.
    let (repository, quantization) = match rest.split_once(':') {
        Some((repository, quantization)) => (repository, Some(quantization)),
        None => (rest, None),
    };
    if repository.is_empty() || repository.chars().any(char::is_whitespace) {
        return false;
    }
    match quantization {
        None => true,
        Some(quantization) => {
            !quantization.is_empty()
                && !quantization.contains(':')
                && !quantization.chars().any(char::is_whitespace)
        }
    }
}

// ============================================================================
// Hugging Face search
// ============================================================================

/// The search cache of one manager session (`LlamaView.searchCache`).
pub type SearchCache = Rc<RefCell<BTreeMap<String, Vec<HuggingFaceModel>>>>;

/// `class HuggingFaceSearch extends Container implements Focusable`
///
/// Deviation (class 1): TypeScript runs the debounce with `setTimeout` and the
/// request with an `AbortController` the component owns. The port keeps the
/// same state but lets the caller drive both time seams (interface request
/// A-23): [`HuggingFaceSearch::take_due_search`] hands out the query whose
/// debounce has elapsed together with its cancellation token, and
/// [`HuggingFaceSearch::apply_search_result`] delivers the answer.
pub struct HuggingFaceSearch {
    container: Container,
    input: Rc<RefCell<Input>>,
    results_container: Rc<RefCell<Container>>,
    results: Vec<HuggingFaceModel>,
    filtered_results: Vec<HuggingFaceModel>,
    selected_index: usize,
    query: String,
    status: String,
    cache: SearchCache,
    /// `debounce` — the query and when its request is due.
    debounce: Option<(String, Instant)>,
    /// `request` — the controller of the request in flight.
    request: Option<CancellationToken>,
    closed: bool,
    focused: bool,
    seq: u64,
    answers: LlamaAnswers,
    request_render: Rc<dyn Fn()>,
}

impl HuggingFaceSearch {
    /// `new HuggingFaceSearch(tui, theme, keybindings, search, cache, onSelectModel)`
    pub fn new(
        seq: u64,
        answers: LlamaAnswers,
        cache: SearchCache,
        request_render: Rc<dyn Fn()>,
    ) -> Self {
        let mut container = Container::new();
        container.add_child(component_ref(Text::new(
            theme().fg(ThemeColor::Dim, "Model name or owner/repository[:quant]"),
            1,
            0,
        )));
        let input = Rc::new(RefCell::new(Input::new()));
        container.add_child(Rc::clone(&input) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));
        let results_container = Rc::new(RefCell::new(Container::new()));
        container.add_child(Rc::clone(&results_container) as ComponentRef);

        let mut search = Self {
            container,
            input,
            results_container,
            results: Vec::new(),
            filtered_results: Vec::new(),
            selected_index: 0,
            query: String::new(),
            status: STATUS_TYPE_MORE.to_owned(),
            cache,
            debounce: None,
            request: None,
            closed: false,
            focused: false,
            seq,
            answers,
            request_render,
        };
        search.update_results();
        search
    }

    /// `updateResults()`
    fn update_results(&mut self) {
        let theme_instance = theme();
        {
            let mut results = self.results_container.borrow_mut();
            results.clear();
            let start = self
                .selected_index
                .saturating_sub(MAX_VISIBLE / 2)
                .min(self.filtered_results.len().saturating_sub(MAX_VISIBLE));
            let end = (start + MAX_VISIBLE).min(self.filtered_results.len());
            for index in start..end {
                let Some(model) = self.filtered_results.get(index) else {
                    continue;
                };
                let prefix = if index == self.selected_index {
                    "→ "
                } else {
                    "  "
                };
                let details = format!("{} downloads", compact_count(model.downloads));
                let text = if index == self.selected_index {
                    theme_instance.fg(
                        ThemeColor::Accent,
                        &format!("{prefix}{}  {details}", model.id),
                    )
                } else {
                    format!(
                        "{prefix}{}{}",
                        model.id,
                        theme_instance.fg(ThemeColor::Muted, &format!("  {details}"))
                    )
                };
                results.add_child(component_ref(Text::new(text, 0, 0)));
            }
            if start > 0 || end < self.filtered_results.len() {
                results.add_child(component_ref(Text::new(
                    theme_instance.fg(
                        ThemeColor::Dim,
                        &format!(
                            "  ({}/{})",
                            self.selected_index + 1,
                            self.filtered_results.len()
                        ),
                    ),
                    0,
                    0,
                )));
            }
            if self.filtered_results.is_empty() || self.status == STATUS_SEARCHING {
                results.add_child(component_ref(Text::new(
                    theme_instance.fg(ThemeColor::Dim, &format!("  {}", self.status)),
                    0,
                    0,
                )));
            }
        }
        (self.request_render)();
    }

    /// `filterResults()`
    fn filter_results(&mut self) {
        if self.query.is_empty() {
            self.filtered_results = self.results.clone();
        } else {
            let matches = fuzzy_filter(&self.results, &self.query, |model: &HuggingFaceModel| {
                model.id.clone()
            });
            let ids: std::collections::BTreeSet<String> =
                matches.into_iter().map(|model| model.id).collect();
            self.filtered_results = self
                .results
                .iter()
                .filter(|model| ids.contains(&model.id))
                .cloned()
                .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered_results.len().saturating_sub(1));
        self.update_results();
    }

    /// `scheduleSearch()`
    fn schedule_search(&mut self) {
        self.debounce = None;
        if let Some(request) = self.request.take() {
            request.cancel();
        }
        if self.query.chars().count() < 2 {
            self.status = STATUS_TYPE_MORE.to_owned();
            self.filter_results();
            return;
        }
        let cached = self.cache.borrow().get(&self.query.to_lowercase()).cloned();
        if let Some(cached) = cached {
            self.status = if cached.is_empty() {
                STATUS_NO_MODELS.to_owned()
            } else {
                String::new()
            };
            self.results = cached;
            self.filter_results();
            return;
        }
        self.status = STATUS_SEARCHING.to_owned();
        self.filter_results();
        self.debounce = Some((self.query.clone(), Instant::now() + SEARCH_DEBOUNCE));
    }

    /// When the debounced request is due, if one is scheduled.
    pub fn search_deadline(&self) -> Option<Instant> {
        self.debounce.as_ref().map(|(_, due)| *due)
    }

    /// The query whose debounce has elapsed, with the token that aborts it.
    pub fn take_due_search(&mut self) -> Option<(String, CancellationToken)> {
        let (query, due) = self.debounce.as_ref()?;
        if *due > Instant::now() {
            return None;
        }
        let query = query.clone();
        self.debounce = None;
        let token = CancellationToken::new();
        self.request = Some(token.clone());
        Some((query, token))
    }

    /// `runSearch(query)` after the awaited call returned.
    pub fn apply_search_result(
        &mut self,
        query: &str,
        result: Result<Vec<HuggingFaceModel>, String>,
    ) {
        let aborted = self
            .request
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled);
        match result {
            Ok(results) => {
                // TypeScript caches before it checks the guards.
                self.cache
                    .borrow_mut()
                    .insert(query.to_lowercase(), results.clone());
                if self.closed || aborted || self.query != query {
                    self.clear_request();
                    return;
                }
                self.status = if results.is_empty() {
                    STATUS_NO_MODELS.to_owned()
                } else {
                    String::new()
                };
                self.results = results;
                self.selected_index = 0;
            }
            Err(message) => {
                if self.closed || aborted || self.query != query {
                    self.clear_request();
                    return;
                }
                self.results = Vec::new();
                self.status = message;
            }
        }
        self.clear_request();
        self.filter_results();
    }

    /// `finally { if (this.request === request) this.request = undefined; }`
    fn clear_request(&mut self) {
        self.request = None;
    }

    /// `close(model)`
    fn close(&mut self, model: Option<String>) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.debounce = None;
        if let Some(request) = self.request.take() {
            request.cancel();
        }
        let _ = self.answers.send((self.seq, LlamaAnswer::Search(model)));
    }

    /// `dispose()` of the frame that holds it: stop the timer and the request.
    pub fn dispose(&mut self) {
        self.closed = true;
        self.debounce = None;
        if let Some(request) = self.request.take() {
            request.cancel();
        }
    }
}

impl Component for HuggingFaceSearch {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        if keybindings_match(data, "tui.select.up") {
            if !self.filtered_results.is_empty() {
                self.selected_index = if self.selected_index == 0 {
                    self.filtered_results.len() - 1
                } else {
                    self.selected_index - 1
                };
                self.update_results();
            }
            return;
        }
        if keybindings_match(data, "tui.select.down") {
            if !self.filtered_results.is_empty() {
                self.selected_index = if self.selected_index == self.filtered_results.len() - 1 {
                    0
                } else {
                    self.selected_index + 1
                };
                self.update_results();
            }
            return;
        }
        if keybindings_match(data, "tui.select.confirm") {
            let exact = is_exact_repository(&self.query).then(|| self.query.clone());
            let selected = exact.or_else(|| {
                self.filtered_results
                    .get(self.selected_index)
                    .map(|model| model.id.clone())
            });
            if let Some(selected) = selected {
                self.close(Some(selected));
            }
            return;
        }
        if keybindings_match(data, "tui.select.cancel") {
            self.close(None);
            return;
        }
        self.input.borrow_mut().handle_input(data);
        let query = self.input.borrow().get_value().trim().to_owned();
        if query == self.query {
            return;
        }
        self.query = query;
        self.schedule_search();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for HuggingFaceSearch {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.input.borrow_mut().set_focused(focused);
    }
}

// ============================================================================
// The manager view
// ============================================================================

/// `class LlamaView implements LlamaUi, Focusable`
pub struct LlamaView {
    content: Container,
    /// `inputHandler` — receives the key strokes.
    input_handler: Option<ComponentRef>,
    /// `inputTarget` — receives the focus.
    input_target: Option<ComponentRef>,
    /// The search component while `searchModels` is showing, so the flow can
    /// drive its debounce and its request.
    search: Option<Rc<RefCell<HuggingFaceSearch>>>,
    search_cache: SearchCache,
    /// `progressPromise`/`progressResolver` — the sequence number the stop
    /// answer carries, if a progress promise is outstanding.
    progress_seq: Option<u64>,
    showing_progress: bool,
    focused: bool,
    seq: u64,
    answers: LlamaAnswers,
    request_render: Rc<dyn Fn()>,
}

impl LlamaView {
    /// `new LlamaView(tui, theme, keybindings)`
    pub fn new(answers: LlamaAnswers, request_render: Rc<dyn Fn()>) -> Self {
        let content = frame(
            "llama.cpp models",
            vec![component_ref(Text::new(
                theme().fg(ThemeColor::Muted, "Loading…"),
                1,
                1,
            ))],
            None,
        );
        Self {
            content,
            input_handler: None,
            input_target: None,
            search: None,
            search_cache: Rc::new(RefCell::new(BTreeMap::new())),
            progress_seq: None,
            showing_progress: false,
            focused: false,
            seq: 0,
            answers,
            request_render,
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// `setContent(content, inputHandler, inputTarget)`
    fn set_content(
        &mut self,
        content: Container,
        input_handler: Option<ComponentRef>,
        input_target: Option<ComponentRef>,
    ) {
        if let Some(target) = self.input_target.take()
            && let Some(focusable) = target.borrow_mut().as_focusable()
        {
            focusable.set_focused(false);
        }
        if let Some(search) = self.search.take() {
            search.borrow_mut().dispose();
        }
        self.progress_seq = None;
        self.showing_progress = false;
        self.content = content;
        self.input_handler = input_handler;
        self.input_target = input_target;
        if let Some(target) = self.input_target.as_ref()
            && let Some(focusable) = target.borrow_mut().as_focusable()
        {
            focusable.set_focused(self.focused);
        }
        (self.request_render)();
    }

    /// `showModels(serverUrl, models)`
    pub fn show_models(&mut self, server_url: &str, models: &[LlamaModelInfo]) -> u64 {
        let seq = self.next_seq();
        let mut sorted = models.to_vec();
        sorted.sort_by(|left, right| {
            let loaded = usize::from(right.status.value == LLAMA_STATUS_LOADED)
                .cmp(&usize::from(left.status.value == LLAMA_STATUS_LOADED));
            if loaded != Ordering::Equal {
                return loaded;
            }
            locale_compare(&left.id, &right.id)
        });
        let by_id: Vec<(String, LlamaModelInfo)> = sorted
            .iter()
            .map(|model| (model.id.clone(), model.clone()))
            .collect();
        let mut items: Vec<SelectItem> = sorted
            .iter()
            .map(|model| SelectItem {
                value: model.id.clone(),
                label: model.id.clone(),
                description: Some(model_description(model)),
            })
            .collect();
        items.push(SelectItem {
            value: DOWNLOAD_VALUE.to_owned(),
            label: "Download model…".to_owned(),
            description: Some("Hugging Face owner/repository[:quant]".to_owned()),
        });

        let mut list = SelectList::new(
            items.clone(),
            items.len().min(12),
            select_theme(),
            SelectListLayoutOptions {
                min_primary_column_width: Some(36),
                max_primary_column_width: Some(56),
                truncate_primary: None,
            },
        );
        let answers = self.answers.clone();
        list.on_select = Some(Box::new(move |item: &SelectItem| {
            if item.value == DOWNLOAD_VALUE {
                let _ = answers.send((seq, LlamaAnswer::Models(LlamaManagerAction::Download)));
            } else if let Some((_, model)) = by_id.iter().find(|(id, _)| *id == item.value) {
                let _ = answers.send((
                    seq,
                    LlamaAnswer::Models(LlamaManagerAction::Model(Box::new(model.clone()))),
                ));
            }
        }));
        let answers = self.answers.clone();
        list.on_cancel = Some(Box::new(move || {
            let _ = answers.send((seq, LlamaAnswer::Models(LlamaManagerAction::Close)));
        }));

        let list: ComponentRef = component_ref(list);
        let content = frame(
            "llama.cpp models",
            vec![
                component_ref(Text::new(theme().fg(ThemeColor::Dim, server_url), 1, 0)),
                component_ref(Spacer::new(1)),
                Rc::clone(&list),
            ],
            Some(&format!(
                "{} • {}",
                key_hint("tui.select.confirm", "load/unload/download"),
                key_hint("tui.select.cancel", "close")
            )),
        );
        self.set_content(content, Some(Rc::clone(&list)), Some(list));
        seq
    }

    /// `select(title, options)`
    pub fn select(&mut self, title: &str, options: &[String]) -> u64 {
        let seq = self.next_seq();
        let items: Vec<SelectItem> = options
            .iter()
            .map(|option| SelectItem {
                value: option.clone(),
                label: option.clone(),
                description: None,
            })
            .collect();
        let mut list = SelectList::new(
            items,
            options.len().min(12),
            select_theme(),
            SelectListLayoutOptions::default(),
        );
        let answers = self.answers.clone();
        list.on_select = Some(Box::new(move |item: &SelectItem| {
            let _ = answers.send((seq, LlamaAnswer::Select(Some(item.value.clone()))));
        }));
        let answers = self.answers.clone();
        list.on_cancel = Some(Box::new(move || {
            let _ = answers.send((seq, LlamaAnswer::Select(None)));
        }));

        let list: ComponentRef = component_ref(list);
        let content = frame(
            title,
            vec![component_ref(Spacer::new(1)), Rc::clone(&list)],
            Some(&format!(
                "{} • {}",
                key_hint("tui.select.confirm", "select"),
                key_hint("tui.select.cancel", "cancel")
            )),
        );
        self.set_content(content, Some(Rc::clone(&list)), Some(list));
        seq
    }

    /// `searchModels(search)`
    pub fn search_models(&mut self) -> u64 {
        let seq = self.next_seq();
        let component = Rc::new(RefCell::new(HuggingFaceSearch::new(
            seq,
            self.answers.clone(),
            Rc::clone(&self.search_cache),
            Rc::clone(&self.request_render),
        )));
        let content = frame(
            "Download model",
            vec![
                component_ref(Spacer::new(1)),
                Rc::clone(&component) as ComponentRef,
            ],
            Some(&format!(
                "{} • {}",
                key_hint("tui.select.confirm", "select"),
                key_hint("tui.select.cancel", "back")
            )),
        );
        self.set_content(
            content,
            Some(Rc::clone(&component) as ComponentRef),
            Some(Rc::clone(&component) as ComponentRef),
        );
        self.search = Some(component);
        seq
    }

    /// The search component while `searchModels` is showing.
    pub fn search(&self) -> Option<Rc<RefCell<HuggingFaceSearch>>> {
        self.search.clone()
    }

    /// `showStatus(title, message)`
    pub fn show_status(&mut self, title: &str, message: &str) {
        let content = frame(
            title,
            vec![
                component_ref(Spacer::new(1)),
                component_ref(Text::new(theme().fg(ThemeColor::Muted, message), 1, 0)),
            ],
            None,
        );
        self.set_content(content, None, None);
    }

    /// `progress(state)`
    pub fn progress(&mut self, state: &ProgressState) -> u64 {
        let seq = match self.progress_seq {
            Some(seq) => seq,
            None => {
                let seq = self.next_seq();
                self.progress_seq = Some(seq);
                seq
            }
        };
        self.showing_progress = true;
        self.update_progress(state);
        seq
    }

    /// `updateProgress(state)`
    pub fn update_progress(&mut self, state: &ProgressState) {
        if !self.showing_progress {
            return;
        }
        let theme_instance = theme();
        let mut body: Vec<ComponentRef> = vec![
            component_ref(Text::new(
                theme_instance.fg(ThemeColor::Text, &state.model),
                1,
                0,
            )),
            component_ref(Spacer::new(1)),
            component_ref(Text::new(
                theme_instance.fg(ThemeColor::Muted, &state.message),
                1,
                0,
            )),
        ];
        if let Some(ratio) = state.ratio {
            let available = 40_usize;
            let clamped = ratio.clamp(0.0, 1.0);
            let filled = (clamped * available as f64).round() as usize;
            let filled = filled.min(available);
            body.push(component_ref(Text::new(
                theme_instance.fg(
                    ThemeColor::Accent,
                    &format!(
                        "{}{} {}%",
                        "█".repeat(filled),
                        "─".repeat(available - filled),
                        notagent_ai::utils::js_number::to_js_string((ratio * 100.0).round())
                    ),
                ),
                1,
                0,
            )));
        }
        if let Some(detail) = state.detail.as_ref().filter(|detail| !detail.is_empty()) {
            body.push(component_ref(Text::new(
                theme_instance.fg(ThemeColor::Dim, detail),
                1,
                0,
            )));
        }
        self.content = frame(
            &state.title,
            body,
            Some(&key_hint("tui.select.cancel", "stop")),
        );
        self.input_handler = None;
        (self.request_render)();
    }
}

impl Component for LlamaView {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.content
            .render(width)
            .into_iter()
            .map(|line| {
                if visible_width(&line) > width {
                    truncate_to_width_opts(&line, width, "", false)
                } else {
                    line
                }
            })
            .collect()
    }

    fn invalidate(&mut self) {
        self.content.invalidate();
    }

    fn handle_input(&mut self, data: &str) {
        if let Some(seq) = self.progress_seq
            && keybindings_match(data, "tui.select.cancel")
        {
            self.progress_seq = None;
            let _ = self.answers.send((seq, LlamaAnswer::ProgressStop));
            return;
        }
        if let Some(handler) = self.input_handler.clone() {
            handler.borrow_mut().handle_input(data);
        }
        (self.request_render)();
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for LlamaView {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if let Some(target) = self.input_target.as_ref()
            && let Some(focusable) = target.borrow_mut().as_focusable()
        {
            focusable.set_focused(focused);
        }
    }
}
