use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use notagent::core::auth_storage::AuthStorage;
use notagent::core::keybindings::KeybindingsManager;
use notagent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
use notagent::core::settings_manager::{
    InMemorySettingsStorage, SettingsManager, SettingsManagerCreateOptions,
};
use notagent::modes::interactive::components::model_selector::{
    ModelRefreshOutcome, ModelSelectorComponent,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_ai::auth::types::CredentialStore;
use notagent_ai::models::Provider;
use notagent_ai::models_store::InMemoryModelsStore;
use notagent_ai::providers::faux::{FauxModelDefinition, FauxProviderOptions, faux_provider};
use notagent_ai::types::Model;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

/// The theme and the keybindings registry are process globals; the cases here
/// await the runtime while holding the lock, so it is the async mutex.
async fn test_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    init_theme(Some("dark"), false);
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

/// Down arrow — `tui.select.down`.
const DOWN: &str = "\x1b[B";

struct Harness {
    runtime: Arc<ModelRuntime>,
    settings_manager: Arc<SettingsManager>,
    models: Vec<Model>,
}

impl Harness {
    /// `createHarness({ models })` — reduced to the model plumbing.
    async fn create(models: Vec<FauxModelDefinition>) -> Self {
        let faux = faux_provider(FauxProviderOptions {
            models: Some(models),
            ..FauxProviderOptions::default()
        });
        let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
            credentials: Some(Arc::new(AuthStorage::in_memory(serde_json::Map::new()))
                as Arc<dyn CredentialStore>),
            models_store: Some(Arc::new(InMemoryModelsStore::new())),
            models_path: Some(None),
            allow_model_network: Some(false),
            ..CreateModelRuntimeOptions::default()
        })
        .await
        .expect("runtime");
        runtime
            .register_native_provider(Arc::clone(&faux.provider) as Arc<dyn Provider>)
            .expect("register faux provider");
        Harness {
            runtime,
            settings_manager: Arc::new(SettingsManager::from_storage(
                Arc::new(InMemorySettingsStorage::default()),
                SettingsManagerCreateOptions::default(),
            )),
            models: faux.models().to_vec(),
        }
    }

    /// `harness.getModel(id?)`
    fn get_model(&self, model_id: Option<&str>) -> Option<Model> {
        match model_id {
            None => self.models.first().cloned(),
            Some(model_id) => self
                .models
                .iter()
                .find(|model| model.id == model_id)
                .cloned(),
        }
    }

    fn selector(
        &self,
        current: Option<Model>,
        scoped_models: Vec<notagent::core::model_resolver::ScopedModel>,
    ) -> ModelSelectorComponent {
        ModelSelectorComponent::new(
            Rc::new(|| {}),
            current,
            Arc::clone(&self.settings_manager),
            Arc::clone(&self.runtime),
            scoped_models,
            Box::new(|_model| {}),
            Box::new(|| {}),
            None,
        )
    }
}

fn faux_model(id: &str, name: &str) -> FauxModelDefinition {
    FauxModelDefinition {
        name: Some(name.to_owned()),
        reasoning: Some(true),
        ..FauxModelDefinition::new(id)
    }
}

fn rendered(selector: &mut ModelSelectorComponent) -> String {
    strip_ansi(&selector.render(120).join("\n"))
}

/// Return the model id of the highlighted (→) row in the rendered selector.
fn selected_model_id(rendered: &str) -> Option<String> {
    let line = rendered.lines().find(|line| line.starts_with("→ "))?;
    let rest = line.trim_start_matches('→').trim_start();
    let id = rest.split(" [").next()?.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

#[tokio::test]
async fn lists_every_catalog_that_failed_to_refresh() {
    let _guard = test_lock().await;
    let harness = Harness::create(Vec::new()).await;
    let mut selector = harness.selector(harness.get_model(None), Vec::new());

    // `vi.spyOn(harness.session.modelRuntime, "refresh").mockResolvedValue(...)`:
    // a Rust method cannot be replaced at runtime, so the same seam is used from
    // the other side — the refresh outcome the component renders.
    selector.apply_refresh(ModelRefreshOutcome {
        aborted: false,
        timed_out: false,
        failed_providers: vec!["openai".to_owned(), "anthropic".to_owned()],
    });

    assert!(rendered(&mut selector).contains(
        "Could not refresh 2 model catalogs (openai, anthropic); showing cached models."
    ));
}

/// source but never renders.
#[tokio::test]
async fn names_the_single_catalog_that_failed_to_refresh() {
    let _guard = test_lock().await;
    let harness = Harness::create(Vec::new()).await;
    let mut selector = harness.selector(harness.get_model(None), Vec::new());

    selector.apply_refresh(ModelRefreshOutcome {
        aborted: false,
        timed_out: false,
        failed_providers: vec!["openai".to_owned()],
    });

    assert!(rendered(&mut selector).contains("Could not refresh openai; showing cached models."));
}

/// `refreshModels` after the 15 s timer aborted the shared signal.
#[tokio::test]
async fn reports_a_timed_out_refresh() {
    let _guard = test_lock().await;
    let harness = Harness::create(Vec::new()).await;
    let mut selector = harness.selector(harness.get_model(None), Vec::new());

    selector.apply_refresh(ModelRefreshOutcome {
        aborted: true,
        timed_out: true,
        failed_providers: Vec::new(),
    });

    assert!(rendered(&mut selector).contains("Model refresh timed out; showing cached models."));
}

#[tokio::test]
async fn moves_selection_to_the_first_row_in_the_all_tab_when_typing_a_query() {
    let _guard = test_lock().await;
    let harness = Harness::create(vec![
        faux_model("alpha-1", "Alpha One"),
        faux_model("alpha-2", "Alpha Two"),
        faux_model("alpha-3", "Alpha Three"),
        faux_model("beta-1", "Beta One"),
    ])
    .await;

    let current = harness.get_model(Some("alpha-1")).expect("alpha-1");
    let mut selector = harness.selector(Some(current), Vec::new());
    let outcome = selector.refresh_models().await;
    selector.apply_refresh(outcome);
    assert!(rendered(&mut selector).contains("Model catalogs refreshed."));

    // Current model (alpha-1) is sorted first, so selection starts on row 0.
    assert_eq!(
        selected_model_id(&rendered(&mut selector)).as_deref(),
        Some("alpha-1")
    );

    // Move selection down two rows to alpha-3.
    selector.handle_input(DOWN);
    selector.handle_input(DOWN);
    assert_eq!(
        selected_model_id(&rendered(&mut selector)).as_deref(),
        Some("alpha-3")
    );

    // Type a query that matches the three alpha models. The selection must
    // move back to the top row (alpha-1), not stay clamped at index 2.
    for character in "alpha".chars() {
        selector.handle_input(&character.to_string());
    }

    let output = rendered(&mut selector);
    assert_eq!(selected_model_id(&output).as_deref(), Some("alpha-1"));
    // Sanity: the filter actually narrowed the list.
    assert!(!output.contains("beta-1"));
}

#[tokio::test]
async fn moves_selection_to_the_first_row_in_the_scoped_tab_when_typing_a_query() {
    let _guard = test_lock().await;
    let harness = Harness::create(vec![
        faux_model("alpha-1", "Alpha One"),
        faux_model("alpha-2", "Alpha Two"),
        faux_model("alpha-3", "Alpha Three"),
    ])
    .await;

    let alpha1 = harness.get_model(Some("alpha-1")).expect("alpha-1");
    let alpha2 = harness.get_model(Some("alpha-2")).expect("alpha-2");
    let alpha3 = harness.get_model(Some("alpha-3")).expect("alpha-3");

    // Scoped list is intentionally not in current-model-first order; the
    // current model (alpha-1) sits at index 2.
    let mut selector = harness.selector(
        Some(alpha1.clone()),
        vec![scoped(alpha2), scoped(alpha3), scoped(alpha1)],
    );
    let outcome = selector.refresh_models().await;
    selector.apply_refresh(outcome);
    assert!(rendered(&mut selector).contains("Model catalogs refreshed."));

    // Selection starts on the current model (alpha-1), which is row 2 here.
    assert_eq!(
        selected_model_id(&rendered(&mut selector)).as_deref(),
        Some("alpha-1")
    );

    // Type a query matching all three scoped models. Selection must move to
    // the top row (alpha-2), not stay clamped at index 2 (alpha-1).
    for character in "alpha".chars() {
        selector.handle_input(&character.to_string());
    }

    assert_eq!(
        selected_model_id(&rendered(&mut selector)).as_deref(),
        Some("alpha-2")
    );
}

fn scoped(model: Model) -> notagent::core::model_resolver::ScopedModel {
    notagent::core::model_resolver::ScopedModel {
        model,
        thinking_level: None,
    }
}

/// `handleSelect` persists the pick before the callback runs.
#[tokio::test]
async fn selecting_a_model_saves_it_as_the_new_default() {
    let _guard = test_lock().await;
    let harness = Harness::create(vec![
        faux_model("alpha-1", "Alpha One"),
        faux_model("alpha-2", "Alpha Two"),
    ])
    .await;
    let selected: Rc<RefCell<Option<Model>>> = Rc::new(RefCell::new(None));

    let mut selector = {
        let selected = Rc::clone(&selected);
        ModelSelectorComponent::new(
            Rc::new(|| {}),
            harness.get_model(Some("alpha-1")),
            Arc::clone(&harness.settings_manager),
            Arc::clone(&harness.runtime),
            Vec::new(),
            Box::new(move |model| *selected.borrow_mut() = Some(model)),
            Box::new(|| {}),
            None,
        )
    };
    let outcome = selector.refresh_models().await;
    selector.apply_refresh(outcome);

    selector.handle_input(DOWN);
    selector.handle_input("\r");

    assert_eq!(
        selected.borrow().as_ref().map(|model| model.id.clone()),
        Some("alpha-2".to_owned())
    );
    assert_eq!(
        harness.settings_manager.get_default_model().as_deref(),
        Some("alpha-2")
    );
}
