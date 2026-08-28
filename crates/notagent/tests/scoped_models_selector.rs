use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::core::keybindings::KeybindingsManager;
use notagent::modes::interactive::components::scoped_models_selector::{
    ModelsCallbacks, ModelsConfig, ScopedModelsSelectorComponent,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_ai::types::{Modality, Model, ModelCost};
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

/// The theme and the keybindings registry are process globals.
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    // Ensure test isolation: keybindings are a global singleton
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

const ENTER: &str = "\r";
/// Ctrl+S — `app.models.save`
const CTRL_S: &str = "\x13";
/// Alt+Down — `app.models.reorderDown`
const ALT_DOWN: &str = "\x1b[1;3B";
/// Alt+Up — `app.models.reorderUp`
const ALT_UP: &str = "\x1b[1;3A";
/// Ctrl+A — `app.models.enableAll`
const CTRL_A: &str = "\x01";
/// Ctrl+X — `app.models.clearAll`
const CTRL_X: &str = "\x18";
/// Ctrl+P — `app.models.toggleProvider`
const CTRL_P: &str = "\x10";

fn model(id: &str, name: &str, provider: &str) -> Model {
    Model {
        id: id.to_string(),
        name: name.to_string(),
        api: "openai-completions".to_string(),
        provider: provider.to_string(),
        base_url: "http://127.0.0.1:9".to_string(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_tokens: 32_000,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

struct Recorder {
    changes: Rc<RefCell<Vec<Option<Vec<String>>>>>,
    persisted: Rc<RefCell<Vec<Option<Vec<String>>>>>,
    cancels: Rc<RefCell<usize>>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            changes: Rc::new(RefCell::new(Vec::new())),
            persisted: Rc::new(RefCell::new(Vec::new())),
            cancels: Rc::new(RefCell::new(0)),
        }
    }

    fn callbacks(&self) -> ModelsCallbacks {
        let changes = Rc::clone(&self.changes);
        let persisted = Rc::clone(&self.persisted);
        let cancels = Rc::clone(&self.cancels);
        ModelsCallbacks {
            on_change: Box::new(move |ids| changes.borrow_mut().push(ids)),
            on_persist: Box::new(move |ids| persisted.borrow_mut().push(ids)),
            on_cancel: Box::new(move || *cancels.borrow_mut() += 1),
        }
    }
}

#[test]
fn propagates_reordered_scoped_models_back_to_the_session_state() {
    let _guard = test_lock();
    let models = vec![
        model("faux-1", "One", "faux"),
        model("faux-2", "Two", "faux"),
        model("faux-3", "Three", "faux"),
    ];
    let ordered_ids: Vec<String> = models
        .iter()
        .map(|model| format!("{}/{}", model.provider, model.id))
        .collect();
    let recorder = Recorder::new();
    let mut selector = ScopedModelsSelectorComponent::new(
        ModelsConfig {
            all_models: models,
            enabled_model_ids: Some(ordered_ids.clone()),
            refresh_status: None,
        },
        recorder.callbacks(),
    );

    selector.handle_input(ALT_DOWN);

    assert_eq!(
        *recorder.changes.borrow(),
        [Some(vec![
            ordered_ids[1].clone(),
            ordered_ids[0].clone(),
            ordered_ids[2].clone()
        ])]
    );
}

#[test]
fn shows_and_removes_an_enabled_model_without_a_catalog_entry() {
    let _guard = test_lock();
    let available = model("available", "Available", "faux");
    let available_id = format!("{}/{}", available.provider, available.id);
    let unavailable_id = format!("{}/unavailable", available.provider);
    let recorder = Recorder::new();
    let mut selector = ScopedModelsSelectorComponent::new(
        ModelsConfig {
            all_models: vec![available],
            enabled_model_ids: Some(vec![unavailable_id.clone(), available_id.clone()]),
            refresh_status: None,
        },
        recorder.callbacks(),
    );

    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(
        rendered.contains(&format!("{unavailable_id} [unavailable] ✗")),
        "{rendered}"
    );

    selector.handle_input(ENTER);
    assert_eq!(
        *recorder.changes.borrow(),
        [Some(vec![available_id.clone()])]
    );

    selector.handle_input(CTRL_S);
    assert_eq!(*recorder.persisted.borrow(), [Some(vec![available_id])]);
}

#[test]
fn starts_with_every_model_enabled_and_toggles_into_an_explicit_list() {
    let _guard = test_lock();
    let recorder = Recorder::new();
    let mut selector = ScopedModelsSelectorComponent::new(
        ModelsConfig {
            all_models: vec![model("a", "A", "faux"), model("b", "B", "faux")],
            enabled_model_ids: None,
            refresh_status: None,
        },
        recorder.callbacks(),
    );

    // `null` means "all enabled", which the footer states and the rows leave unmarked.
    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("all enabled"), "{rendered}");
    assert!(!rendered.contains('✓'), "{rendered}");

    // The first toggle starts an explicit list holding only that model.
    selector.handle_input(ENTER);
    assert_eq!(
        *recorder.changes.borrow(),
        [Some(vec!["faux/a".to_string()])]
    );
    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("1/2 enabled"), "{rendered}");
    assert!(rendered.contains("(unsaved)"), "{rendered}");

    // Enabling everything again collapses back to `null`.
    selector.handle_input(CTRL_A);
    assert_eq!(recorder.changes.borrow().last(), Some(&None));
}

#[test]
fn clears_and_reorders_only_within_the_explicit_list() {
    let _guard = test_lock();
    let recorder = Recorder::new();
    let mut selector = ScopedModelsSelectorComponent::new(
        ModelsConfig {
            all_models: vec![
                model("a", "A", "faux"),
                model("b", "B", "faux"),
                model("c", "C", "other"),
            ],
            enabled_model_ids: Some(vec!["faux/a".to_string(), "faux/b".to_string()]),
            refresh_status: None,
        },
        recorder.callbacks(),
    );

    // The cursor sits on the first enabled model; it cannot move further up.
    selector.handle_input(ALT_UP);
    assert!(recorder.changes.borrow().is_empty());

    // Ctrl+P toggles every model of the selected model's provider.
    selector.handle_input(CTRL_P);
    assert_eq!(recorder.changes.borrow().last(), Some(&Some(Vec::new())));

    selector.handle_input(CTRL_X);
    assert_eq!(recorder.changes.borrow().last(), Some(&Some(Vec::new())));
}

#[test]
fn filters_through_the_search_input_and_cancels_on_escape() {
    let _guard = test_lock();
    let recorder = Recorder::new();
    let mut selector = ScopedModelsSelectorComponent::new(
        ModelsConfig {
            all_models: vec![
                model("alpha", "Alpha", "faux"),
                model("beta", "Beta", "faux"),
            ],
            enabled_model_ids: None,
            refresh_status: Some("Refreshing catalogs…".to_string()),
        },
        recorder.callbacks(),
    );

    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("Refreshing catalogs…"), "{rendered}");

    for character in "beta".chars() {
        selector.handle_input(&character.to_string());
    }
    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("beta [faux]"), "{rendered}");
    assert!(!rendered.contains("alpha [faux]"), "{rendered}");

    // Ctrl+C clears the query first and only cancels once it is empty.
    selector.handle_input("\x03");
    assert_eq!(*recorder.cancels.borrow(), 0);
    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("alpha [faux]"), "{rendered}");

    selector.handle_input("\x03");
    assert_eq!(*recorder.cancels.borrow(), 1);

    selector.handle_input("\x1b");
    assert_eq!(*recorder.cancels.borrow(), 2);
}

#[test]
fn reports_an_empty_filter_result() {
    let _guard = test_lock();
    let recorder = Recorder::new();
    let mut selector = ScopedModelsSelectorComponent::new(
        ModelsConfig {
            all_models: vec![model("alpha", "Alpha", "faux")],
            enabled_model_ids: None,
            refresh_status: None,
        },
        recorder.callbacks(),
    );

    for character in "zzz".chars() {
        selector.handle_input(&character.to_string());
    }
    let rendered = strip_ansi(&selector.render(100).join("\n"));
    assert!(rendered.contains("No matching models"), "{rendered}");
}
