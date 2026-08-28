use futures::future::BoxFuture;
use notagent::core::model_resolver::{
    DEFAULT_MODEL_PER_PROVIDER, FindInitialModelOptions, ModelCatalog, ModelScopeDiagnostic,
    ModelScopeDiagnosticCode, ScopedModel, default_model_for_provider, find_initial_model,
    parse_model_pattern, resolve_cli_model, resolve_model_scope,
    resolve_model_scope_with_diagnostics,
};
use notagent_agent::types::ThinkingLevel;
use notagent_ai::auth::types::AuthOperationOptions;
use notagent_ai::types::{Modality, Model, ModelCost};

fn model(id: &str, name: &str, provider: &str, base_url: &str, reasoning: bool) -> Model {
    Model {
        id: id.to_owned(),
        name: name.to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: provider.to_owned(),
        base_url: base_url.to_owned(),
        reasoning,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write: 1.0,
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 8_192,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

fn mock_models() -> Vec<Model> {
    vec![
        Model {
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
                tiers: None,
            },
            context_window: 200_000,
            ..model(
                "claude-sonnet-4-5",
                "Claude Sonnet 4.5",
                "anthropic",
                "https://api.anthropic.com",
                true,
            )
        },
        Model {
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost {
                input: 5.0,
                output: 15.0,
                cache_read: 0.5,
                cache_write: 5.0,
                tiers: None,
            },
            max_tokens: 4_096,
            ..model(
                "gpt-4o",
                "GPT-4o",
                "openai",
                "https://api.openai.com",
                false,
            )
        },
    ]
}

fn mock_openrouter_models() -> Vec<Model> {
    vec![
        model(
            "qwen/qwen3-coder:exacto",
            "Qwen3 Coder Exacto",
            "openrouter",
            "https://openrouter.ai/api/v1",
            true,
        ),
        Model {
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost {
                input: 5.0,
                output: 15.0,
                cache_read: 0.5,
                cache_write: 5.0,
                tiers: None,
            },
            max_tokens: 4_096,
            ..model(
                "openai/gpt-4o:extended",
                "GPT-4o Extended",
                "openrouter",
                "https://openrouter.ai/api/v1",
                false,
            )
        },
    ]
}

fn all_models() -> Vec<Model> {
    let mut models = mock_models();
    models.extend(mock_openrouter_models());
    models
}

type ModelLookup = Box<dyn Fn(&str, &str) -> Option<Model> + Send + Sync>;

#[derive(Default)]
struct StubCatalog {
    models: Vec<Model>,
    available: Vec<Model>,
    authenticated: Vec<String>,
    all_authenticated: bool,
    model_lookup: Option<ModelLookup>,
}

impl StubCatalog {
    fn with_models(models: Vec<Model>) -> Self {
        StubCatalog {
            models,
            ..StubCatalog::default()
        }
    }
}

impl ModelCatalog for StubCatalog {
    fn get_models(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        match &self.model_lookup {
            Some(lookup) => lookup(provider, model_id),
            None => self
                .models
                .iter()
                .find(|model| model.provider == provider && model.id == model_id)
                .cloned(),
        }
    }

    fn get_available_snapshot(&self) -> Vec<Model> {
        self.available.clone()
    }

    fn has_configured_auth(&self, provider: &str) -> bool {
        self.all_authenticated || self.authenticated.iter().any(|entry| entry == provider)
    }

    fn get_available<'a>(
        &'a self,
        _options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Vec<Model>> {
        Box::pin(async move { self.available.clone() })
    }
}

// ---------------------------------------------------------------------------
// parseModelPattern
// ---------------------------------------------------------------------------

#[test]
fn exact_match_returns_model_with_undefined_thinking_level() {
    let result = parse_model_pattern("claude-sonnet-4-5", &all_models(), true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(result.thinking_level, None);
    assert_eq!(result.warning, None);
}

#[test]
fn partial_match_returns_best_model() {
    let result = parse_model_pattern("sonnet", &all_models(), true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(result.thinking_level, None);
    assert_eq!(result.warning, None);
}

#[test]
fn no_match_returns_nothing() {
    let result = parse_model_pattern("nonexistent", &all_models(), true);
    assert!(result.model.is_none());
    assert_eq!(result.thinking_level, None);
    assert_eq!(result.warning, None);
}

#[test]
fn valid_thinking_level_suffixes_are_parsed() {
    let models = all_models();
    let result = parse_model_pattern("sonnet:high", &models, true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(result.thinking_level, Some(ThinkingLevel::High));
    assert_eq!(result.warning, None);

    let result = parse_model_pattern("gpt-4o:medium", &models, true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("gpt-4o")
    );
    assert_eq!(result.thinking_level, Some(ThinkingLevel::Medium));

    for (level, expected) in [
        ("off", ThinkingLevel::Off),
        ("minimal", ThinkingLevel::Minimal),
        ("low", ThinkingLevel::Low),
        ("medium", ThinkingLevel::Medium),
        ("high", ThinkingLevel::High),
        ("xhigh", ThinkingLevel::Xhigh),
        ("max", ThinkingLevel::Max),
    ] {
        let result = parse_model_pattern(&format!("sonnet:{level}"), &models, true);
        assert_eq!(
            result.model.map(|model| model.id).as_deref(),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(result.thinking_level, Some(expected));
        assert_eq!(result.warning, None);
    }
}

#[test]
fn invalid_thinking_levels_warn_and_fall_back() {
    let models = all_models();
    let result = parse_model_pattern("sonnet:random", &models, true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(result.thinking_level, None);
    let warning = result.warning.expect("warning");
    assert!(warning.contains("Invalid thinking level"));
    assert!(warning.contains("random"));

    let result = parse_model_pattern("gpt-4o:invalid", &models, true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("gpt-4o")
    );
    assert_eq!(result.thinking_level, None);
    assert!(
        result
            .warning
            .expect("warning")
            .contains("Invalid thinking level")
    );
}

#[test]
fn openrouter_ids_with_colons_match() {
    let models = all_models();
    let result = parse_model_pattern("qwen/qwen3-coder:exacto", &models, true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("qwen/qwen3-coder:exacto")
    );
    assert_eq!(result.thinking_level, None);
    assert_eq!(result.warning, None);

    let result = parse_model_pattern("openrouter/qwen/qwen3-coder:exacto", &models, true);
    let model = result.model.expect("model");
    assert_eq!(model.id, "qwen/qwen3-coder:exacto");
    assert_eq!(model.provider, "openrouter");
    assert_eq!(result.thinking_level, None);

    let result = parse_model_pattern("qwen/qwen3-coder:exacto:high", &models, true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("qwen/qwen3-coder:exacto")
    );
    assert_eq!(result.thinking_level, Some(ThinkingLevel::High));

    let result = parse_model_pattern("openrouter/qwen/qwen3-coder:exacto:high", &models, true);
    let model = result.model.expect("model");
    assert_eq!(model.id, "qwen/qwen3-coder:exacto");
    assert_eq!(model.provider, "openrouter");
    assert_eq!(result.thinking_level, Some(ThinkingLevel::High));

    let result = parse_model_pattern("openai/gpt-4o:extended", &models, true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("openai/gpt-4o:extended")
    );
    assert_eq!(result.thinking_level, None);
}

#[test]
fn invalid_thinking_levels_with_openrouter_models() {
    let models = all_models();
    for pattern in [
        "qwen/qwen3-coder:exacto:random",
        "qwen/qwen3-coder:exacto:high:random",
    ] {
        let result = parse_model_pattern(pattern, &models, true);
        assert_eq!(
            result.model.map(|model| model.id).as_deref(),
            Some("qwen/qwen3-coder:exacto")
        );
        assert_eq!(result.thinking_level, None);
        let warning = result.warning.expect("warning");
        assert!(warning.contains("Invalid thinking level"));
        assert!(warning.contains("random"));
    }
}

#[test]
fn empty_pattern_matches_via_partial_matching() {
    let result = parse_model_pattern("", &all_models(), true);
    assert!(result.model.is_some());
    assert_eq!(result.thinking_level, None);
}

#[test]
fn pattern_ending_with_colon_treats_empty_suffix_as_invalid() {
    let result = parse_model_pattern("sonnet:", &all_models(), true);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("claude-sonnet-4-5")
    );
    assert!(
        result
            .warning
            .expect("warning")
            .contains("Invalid thinking level")
    );
}

// ---------------------------------------------------------------------------
// resolveModelScopeWithDiagnostics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn returns_scoped_models_and_structured_diagnostics() {
    let catalog = StubCatalog {
        available: all_models(),
        ..StubCatalog::default()
    };
    let patterns = [
        "sonnet:high".to_owned(),
        "gpt-4o:invalid".to_owned(),
        "missing".to_owned(),
    ];
    let result = resolve_model_scope_with_diagnostics(&patterns, &catalog, None).await;
    assert_eq!(
        result
            .scoped_models
            .iter()
            .map(|scoped| scoped.model.id.clone())
            .collect::<Vec<_>>(),
        vec!["claude-sonnet-4-5", "gpt-4o"]
    );
    assert_eq!(
        result.scoped_models[0].thinking_level,
        Some(ThinkingLevel::High)
    );
    assert_eq!(result.scoped_models[1].thinking_level, None);
    assert_eq!(
        result.diagnostics,
        vec![
            ModelScopeDiagnostic {
                code: ModelScopeDiagnosticCode::InvalidThinkingLevel,
                message:
                    "Invalid thinking level \"invalid\" in pattern \"gpt-4o:invalid\". Using default instead."
                        .to_owned(),
                pattern: "gpt-4o:invalid".to_owned(),
            },
            ModelScopeDiagnostic {
                code: ModelScopeDiagnosticCode::NoMatch,
                message: "No models match pattern \"missing\"".to_owned(),
                pattern: "missing".to_owned(),
            },
        ]
    );
}

#[tokio::test]
async fn resolve_model_scope_reports_the_cli_warning_as_a_diagnostic() {
    let catalog = StubCatalog {
        available: all_models(),
        ..StubCatalog::default()
    };
    let result = resolve_model_scope(&["missing".to_owned()], &catalog, None).await;
    assert!(result.scoped_models.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert!(
        result.diagnostics[0]
            .message
            .contains("No models match pattern \"missing\"")
    );
}

#[tokio::test]
async fn resolves_bracketed_model_ids_as_exact_references_before_glob_matching() {
    let mut available = all_models();
    available.push(model(
        "bracketed-model[1m]",
        "Bracketed Model",
        "custom",
        "https://example.invalid",
        true,
    ));
    let catalog = StubCatalog {
        available,
        ..StubCatalog::default()
    };

    let result = resolve_model_scope_with_diagnostics(
        &["custom/bracketed-model[1m]".to_owned()],
        &catalog,
        None,
    )
    .await;
    assert_eq!(
        result
            .scoped_models
            .iter()
            .map(|scoped| scoped.model.id.clone())
            .collect::<Vec<_>>(),
        vec!["bracketed-model[1m]"]
    );
    assert!(result.diagnostics.is_empty());

    let result = resolve_model_scope_with_diagnostics(
        &["custom/bracketed-model[1m]:high".to_owned()],
        &catalog,
        None,
    )
    .await;
    assert_eq!(
        result
            .scoped_models
            .iter()
            .map(|scoped| scoped.model.id.clone())
            .collect::<Vec<_>>(),
        vec!["bracketed-model[1m]"]
    );
    assert_eq!(
        result.scoped_models[0].thinking_level,
        Some(ThinkingLevel::High)
    );
    assert!(result.diagnostics.is_empty());
}

// ---------------------------------------------------------------------------
// resolveCliModel
// ---------------------------------------------------------------------------

#[test]
fn resolves_model_provider_id_without_provider_flag() {
    let catalog = StubCatalog::with_models(all_models());
    let result = resolve_cli_model(None, Some("openai/gpt-4o"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openai");
    assert_eq!(model.id, "gpt-4o");
}

#[test]
fn resolves_fuzzy_patterns_within_an_explicit_provider() {
    let catalog = StubCatalog::with_models(all_models());
    let result = resolve_cli_model(Some("openai"), Some("4o"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openai");
    assert_eq!(model.id, "gpt-4o");
}

#[test]
fn supports_pattern_thinking_without_explicit_thinking_flag() {
    let catalog = StubCatalog::with_models(all_models());
    let result = resolve_cli_model(None, Some("sonnet:high"), None, &catalog);
    assert_eq!(result.error, None);
    assert_eq!(
        result.model.map(|model| model.id).as_deref(),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(result.thinking_level, Some(ThinkingLevel::High));
}

#[test]
fn prefers_exact_model_id_match_over_provider_inference() {
    let catalog = StubCatalog::with_models(all_models());
    let result = resolve_cli_model(None, Some("openai/gpt-4o:extended"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openrouter");
    assert_eq!(model.id, "openai/gpt-4o:extended");
}

#[test]
fn does_not_strip_invalid_suffix_as_thinking_level() {
    let catalog = StubCatalog::with_models(all_models());
    let result = resolve_cli_model(Some("openai"), Some("gpt-4o:extended"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openai");
    assert_eq!(model.id, "gpt-4o:extended");
}

#[test]
fn allows_custom_model_ids_for_explicit_providers_without_double_prefixing() {
    let catalog = StubCatalog::with_models(all_models());
    let result = resolve_cli_model(
        Some("openrouter"),
        Some("openrouter/openai/ghost-model"),
        None,
        &catalog,
    );
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openrouter");
    assert_eq!(model.id, "openai/ghost-model");
}

#[test]
fn returns_a_clear_error_when_there_are_no_models() {
    let catalog = StubCatalog::default();
    let result = resolve_cli_model(Some("openai"), Some("gpt-4o"), None, &catalog);
    assert!(result.model.is_none());
    assert!(result.error.expect("error").contains("No models available"));
}

#[test]
fn prefers_the_sole_authenticated_provider_for_an_ambiguous_bare_exact_model_id() {
    let base = &mock_models()[1];
    let azure = Model {
        id: "gpt-5.6-sol".to_owned(),
        name: "GPT 5.6 Sol".to_owned(),
        provider: "azure-openai-responses".to_owned(),
        ..base.clone()
    };
    let codex = Model {
        provider: "openai-codex".to_owned(),
        ..azure.clone()
    };
    let catalog = StubCatalog {
        models: vec![azure, codex],
        authenticated: vec!["openai-codex".to_owned()],
        ..StubCatalog::default()
    };
    let result = resolve_cli_model(None, Some("gpt-5.6-sol"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openai-codex");
    assert_eq!(model.id, "gpt-5.6-sol");
}

#[test]
fn requires_an_explicit_provider_for_an_ambiguous_bare_exact_model_id() {
    let base = &mock_models()[1];
    let azure = Model {
        id: "gpt-5.6-sol".to_owned(),
        name: "GPT 5.6 Sol".to_owned(),
        provider: "azure-openai-responses".to_owned(),
        ..base.clone()
    };
    let codex = Model {
        provider: "openai-codex".to_owned(),
        ..azure.clone()
    };
    let catalog = StubCatalog {
        models: vec![azure, codex],
        ..StubCatalog::default()
    };
    let result = resolve_cli_model(None, Some("gpt-5.6-sol"), None, &catalog);
    assert!(result.model.is_none());
    let error = result.error.expect("error");
    assert!(error.contains("Model \"gpt-5.6-sol\" is ambiguous across providers"));
    assert!(error.contains("azure-openai-responses/gpt-5.6-sol"));
    assert!(error.contains("openai-codex/gpt-5.6-sol"));
    assert!(error.contains("Use --provider or provider/model"));
}

#[test]
fn prefers_provider_model_split_over_gateway_model_with_matching_id() {
    let mut models = all_models();
    models.push(model(
        "glm-5",
        "GLM-5",
        "zai",
        "https://open.bigmodel.cn/api/paas/v4",
        true,
    ));
    models.push(model(
        "zai/glm-5",
        "GLM-5",
        "vercel-ai-gateway",
        "https://ai-gateway.vercel.sh",
        true,
    ));
    let catalog = StubCatalog {
        models,
        all_authenticated: true,
        ..StubCatalog::default()
    };
    let result = resolve_cli_model(None, Some("zai/glm-5"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "zai");
    assert_eq!(model.id, "glm-5");
}

#[test]
fn prefers_an_authenticated_exact_raw_model_id_over_an_unauthenticated_inferred_provider() {
    let mut models = all_models();
    models.push(model(
        "xiaomi/mimo-v2.5-pro",
        "Xiaomi MiMo via Commandcode",
        "commandcode",
        "https://example.invalid",
        false,
    ));
    models.push(model(
        "mimo-v2.5-pro",
        "Xiaomi MiMo",
        "xiaomi",
        "https://api.xiaomimimo.com",
        false,
    ));
    let catalog = StubCatalog {
        models,
        authenticated: vec!["commandcode".to_owned()],
        ..StubCatalog::default()
    };
    let result = resolve_cli_model(None, Some("xiaomi/mimo-v2.5-pro"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "commandcode");
    assert_eq!(model.id, "xiaomi/mimo-v2.5-pro");
}

#[test]
fn resolves_provider_prefixed_fuzzy_patterns() {
    let catalog = StubCatalog::with_models(all_models());
    let result = resolve_cli_model(None, Some("openrouter/qwen"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openrouter");
    assert_eq!(model.id, "qwen/qwen3-coder:exacto");
}

// -- custom model fallback with :thinking suffix (#5552) --------------------

fn models_with_neuralwatt() -> Vec<Model> {
    let mut models = all_models();
    models.push(model(
        "some-base-model",
        "Some Base Model",
        "neuralwatt",
        "https://api.neuralwatt.com",
        false,
    ));
    models
}

#[test]
fn strips_thinking_suffix_from_custom_model_id_in_fallback_path() {
    let catalog = StubCatalog::with_models(models_with_neuralwatt());
    let result = resolve_cli_model(
        None,
        Some("neuralwatt/zai-org/GLM-5.1-FP8:high"),
        None,
        &catalog,
    );
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "neuralwatt");
    assert_eq!(model.id, "zai-org/GLM-5.1-FP8");
    assert!(model.reasoning);
    assert_eq!(result.thinking_level, Some(ThinkingLevel::High));
}

#[test]
fn custom_model_without_thinking_suffix_works_in_fallback_path() {
    let catalog = StubCatalog::with_models(models_with_neuralwatt());
    let result = resolve_cli_model(None, Some("neuralwatt/zai-org/GLM-5.1-FP8"), None, &catalog);
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "neuralwatt");
    assert_eq!(model.id, "zai-org/GLM-5.1-FP8");
    assert_eq!(result.thinking_level, None);
}

#[test]
fn all_valid_thinking_levels_work_in_fallback_path() {
    let catalog = StubCatalog::with_models(models_with_neuralwatt());
    for (level, expected) in [
        ("off", ThinkingLevel::Off),
        ("minimal", ThinkingLevel::Minimal),
        ("low", ThinkingLevel::Low),
        ("medium", ThinkingLevel::Medium),
        ("high", ThinkingLevel::High),
        ("xhigh", ThinkingLevel::Xhigh),
        ("max", ThinkingLevel::Max),
    ] {
        let result = resolve_cli_model(
            None,
            Some(&format!("neuralwatt/zai-org/GLM-5.1-FP8:{level}")),
            None,
            &catalog,
        );
        assert_eq!(result.error, None);
        assert_eq!(
            result.model.map(|model| model.id).as_deref(),
            Some("zai-org/GLM-5.1-FP8")
        );
        assert_eq!(result.thinking_level, Some(expected));
    }
}

#[test]
fn invalid_thinking_suffix_on_custom_model_stays_part_of_the_id() {
    let catalog = StubCatalog::with_models(models_with_neuralwatt());
    let result = resolve_cli_model(
        None,
        Some("neuralwatt/zai-org/GLM-5.1-FP8:banana"),
        None,
        &catalog,
    );
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "neuralwatt");
    assert_eq!(model.id, "zai-org/GLM-5.1-FP8:banana");
    assert_eq!(result.thinking_level, None);
}

#[test]
fn explicit_provider_with_custom_model_thinking_strips_the_suffix() {
    let catalog = StubCatalog::with_models(models_with_neuralwatt());
    let result = resolve_cli_model(
        Some("neuralwatt"),
        Some("zai-org/GLM-5.1-FP8:high"),
        None,
        &catalog,
    );
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "neuralwatt");
    assert_eq!(model.id, "zai-org/GLM-5.1-FP8");
    assert_eq!(result.thinking_level, Some(ThinkingLevel::High));
}

#[test]
fn with_explicit_thinking_the_suffix_is_kept_in_the_model_id() {
    let catalog = StubCatalog::with_models(models_with_neuralwatt());
    let result = resolve_cli_model(
        None,
        Some("neuralwatt/zai-org/GLM-5.1-FP8:high"),
        Some(ThinkingLevel::Medium),
        &catalog,
    );
    assert_eq!(result.error, None);
    let model = result.model.expect("model");
    assert_eq!(model.provider, "neuralwatt");
    assert_eq!(model.id, "zai-org/GLM-5.1-FP8:high");
    assert_eq!(result.thinking_level, None);
}

// ---------------------------------------------------------------------------
// default model selection
// ---------------------------------------------------------------------------

#[test]
fn provider_defaults_track_current_models() {
    assert_eq!(default_model_for_provider("openai"), Some("gpt-5.5"));
    assert_eq!(default_model_for_provider("openai-codex"), Some("gpt-5.5"));
    assert_eq!(default_model_for_provider("zai"), Some("glm-5.1"));
    assert_eq!(default_model_for_provider("minimax"), Some("MiniMax-M2.7"));
    assert_eq!(
        default_model_for_provider("minimax-cn"),
        Some("MiniMax-M2.7")
    );
    assert_eq!(default_model_for_provider("cerebras"), Some("zai-glm-4.7"));
    assert_eq!(default_model_for_provider("ant-ling"), Some("Ring-2.6-1T"));
    assert_eq!(
        default_model_for_provider("vercel-ai-gateway"),
        Some("zai/glm-5.1")
    );
    assert_eq!(
        default_model_for_provider("qwen-token-plan-individual"),
        Some("qwen3.8-max")
    );
    assert_eq!(DEFAULT_MODEL_PER_PROVIDER.len(), 39);
}

#[test]
fn find_initial_model_accepts_explicit_provider_custom_model_ids() {
    let catalog = StubCatalog::with_models(all_models());
    let scoped: Vec<ScopedModel> = Vec::new();
    let result = find_initial_model(FindInitialModelOptions {
        cli_provider: Some("openrouter"),
        cli_model: Some("openrouter/openai/ghost-model"),
        scoped_models: &scoped,
        is_continuing: false,
        default_provider: None,
        default_model_id: None,
        default_thinking_level: None,
        model_runtime: &catalog,
    })
    .expect("initial model");
    let model = result.model.expect("model");
    assert_eq!(model.provider, "openrouter");
    assert_eq!(model.id, "openai/ghost-model");
}

#[test]
fn find_initial_model_selects_the_ai_gateway_default_when_available() {
    let ai_gateway = Model {
        input: vec![Modality::Text, Modality::Image],
        context_window: 200_000,
        ..model(
            "anthropic/claude-opus-4-6",
            "Claude Opus 4.6",
            "vercel-ai-gateway",
            "https://ai-gateway.vercel.sh",
            true,
        )
    };
    let catalog = StubCatalog {
        available: vec![ai_gateway],
        ..StubCatalog::default()
    };
    let scoped: Vec<ScopedModel> = Vec::new();
    let result = find_initial_model(FindInitialModelOptions {
        cli_provider: None,
        cli_model: None,
        scoped_models: &scoped,
        is_continuing: false,
        default_provider: None,
        default_model_id: None,
        default_thinking_level: None,
        model_runtime: &catalog,
    })
    .expect("initial model");
    let model = result.model.expect("model");
    assert_eq!(model.provider, "vercel-ai-gateway");
    assert_eq!(model.id, "anthropic/claude-opus-4-6");
}

#[test]
fn find_initial_model_ignores_an_unauthenticated_saved_default() {
    let saved = model(
        "deepseek-v4-flash",
        "DeepSeek V4 Flash",
        "deepseek",
        "https://api.deepseek.com",
        true,
    );
    let local = Model {
        provider: "spark-two".to_owned(),
        base_url: "http://spark-two:8000/v1".to_owned(),
        ..saved.clone()
    };
    let lookup_saved = saved.clone();
    let catalog = StubCatalog {
        available: vec![local],
        authenticated: vec!["spark-two".to_owned()],
        model_lookup: Some(Box::new(move |provider, model_id| {
            (provider == lookup_saved.provider && model_id == lookup_saved.id)
                .then(|| lookup_saved.clone())
        })),
        ..StubCatalog::default()
    };
    let scoped: Vec<ScopedModel> = Vec::new();
    let result = find_initial_model(FindInitialModelOptions {
        cli_provider: None,
        cli_model: None,
        scoped_models: &scoped,
        is_continuing: false,
        default_provider: Some("deepseek"),
        default_model_id: Some("deepseek-v4-flash"),
        default_thinking_level: None,
        model_runtime: &catalog,
    })
    .expect("initial model");
    let model = result.model.expect("model");
    assert_eq!(model.provider, "spark-two");
    assert_eq!(model.id, "deepseek-v4-flash");
}
