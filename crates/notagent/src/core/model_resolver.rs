use futures::future::BoxFuture;
use globset::GlobBuilder;
use notagent_agent::types::ThinkingLevel;
use notagent_ai::auth::types::AuthOperationOptions;
use notagent_ai::models::models_are_equal;
use notagent_ai::types::Model;

use crate::core::defaults::DEFAULT_THINKING_LEVEL;
use crate::core::model_runtime::ModelRuntime;

/// The `ModelRuntime` surface the resolver reads.
/// structurally typed stubs. Rust has no structural typing, so the four methods the
/// resolver actually calls form a trait that `ModelRuntime` implements.
pub trait ModelCatalog: Send + Sync {
    fn get_models(&self) -> Vec<Model>;
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model>;
    fn get_available_snapshot(&self) -> Vec<Model>;
    fn has_configured_auth(&self, provider: &str) -> bool;
    fn get_available<'a>(
        &'a self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Vec<Model>>;
}

impl ModelCatalog for ModelRuntime {
    fn get_models(&self) -> Vec<Model> {
        ModelRuntime::get_models(self, None)
    }

    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        ModelRuntime::get_model(self, provider, model_id)
    }

    fn get_available_snapshot(&self) -> Vec<Model> {
        ModelRuntime::get_available_snapshot(self)
    }

    fn has_configured_auth(&self, provider: &str) -> bool {
        ModelRuntime::has_configured_auth(self, provider)
    }

    fn get_available<'a>(
        &'a self,
        options: Option<AuthOperationOptions>,
    ) -> BoxFuture<'a, Vec<Model>> {
        Box::pin(async move {
            ModelRuntime::get_available(self, None, options)
                .await
                .unwrap_or_default()
        })
    }
}

/// rather than declaring a second copy.
pub const VALID_THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// `isValidThinkingLevel(level)`
pub fn parse_thinking_level(level: &str) -> Option<ThinkingLevel> {
    match level {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::Xhigh),
        "max" => Some(ThinkingLevel::Max),
        _ => None,
    }
}

pub fn is_valid_thinking_level(level: &str) -> bool {
    parse_thinking_level(level).is_some()
}

/// `defaultModelPerProvider` — default model ids for each known provider, in the
/// declaration order that `findInitialModel` iterates.
pub const DEFAULT_MODEL_PER_PROVIDER: [(&str, &str); 39] = [
    ("amazon-bedrock", "us.anthropic.claude-opus-4-6-v1"),
    ("ant-ling", "Ring-2.6-1T"),
    ("anthropic", "claude-opus-4-8"),
    ("openai", "gpt-5.5"),
    ("azure-openai-responses", "gpt-5.4"),
    ("openai-codex", "gpt-5.5"),
    ("radius", "auto"),
    ("nvidia", "nvidia/nemotron-3-super-120b-a12b"),
    ("deepseek", "deepseek-v4-pro"),
    ("google", "gemini-3.1-pro-preview"),
    ("google-vertex", "gemini-3.1-pro-preview"),
    ("github-copilot", "gpt-5.4"),
    ("openrouter", "moonshotai/kimi-k2.6"),
    ("vercel-ai-gateway", "zai/glm-5.1"),
    ("xai", "grok-4.5"),
    ("groq", "openai/gpt-oss-120b"),
    ("cerebras", "zai-glm-4.7"),
    ("zai", "glm-5.1"),
    ("zai-coding-cn", "glm-5.1"),
    ("mistral", "devstral-medium-latest"),
    ("minimax", "MiniMax-M2.7"),
    ("minimax-cn", "MiniMax-M2.7"),
    ("moonshotai", "kimi-k2.6"),
    ("moonshotai-cn", "kimi-k2.6"),
    ("huggingface", "moonshotai/Kimi-K2.6"),
    ("fireworks", "accounts/fireworks/models/kimi-k2p6"),
    ("together", "moonshotai/Kimi-K2.6"),
    ("baseten", "zai-org/GLM-5.2"),
    ("opencode", "kimi-k2.6"),
    ("opencode-go", "kimi-k2.6"),
    ("kimi-coding", "kimi-for-coding"),
    ("cloudflare-workers-ai", "@cf/moonshotai/kimi-k2.6"),
    (
        "cloudflare-ai-gateway",
        "workers-ai/@cf/moonshotai/kimi-k2.6",
    ),
    ("qwen-token-plan", "qwen3.7-max"),
    ("qwen-token-plan-cn", "qwen3.7-max"),
    ("qwen-token-plan-individual", "qwen3.8-max"),
    ("xiaomi", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-cn", "mimo-v2.5-pro"),
    ("xiaomi-token-plan-ams", "mimo-v2.5-pro"),
];

/// `defaultModelPerProvider[provider]`
pub fn default_model_for_provider(provider: &str) -> Option<&'static str> {
    DEFAULT_MODEL_PER_PROVIDER
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, model)| *model)
}

/// `ScopedModel`
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedModel {
    pub model: Model,
    /// Thinking level if explicitly specified in the pattern (`"model:high"`).
    pub thinking_level: Option<ThinkingLevel>,
}

/// `isAlias(id)` — an id without a `-YYYYMMDD` suffix, or ending in `-latest`.
fn is_alias(id: &str) -> bool {
    if id.ends_with("-latest") {
        return true;
    }
    let bytes = id.as_bytes();
    if bytes.len() < 9 {
        return true;
    }
    let tail = &bytes[bytes.len() - 9..];
    !(tail[0] == b'-' && tail[1..].iter().all(u8::is_ascii_digit))
}

/// `String.prototype.localeCompare` for the model ids used here.
/// Deviation (class 1): the ids are ASCII, where the ICU root collation that
/// `localeCompare` uses orders like a case-insensitive comparison with a
/// case-sensitive tiebreak; `sort((a, b) => b.id.localeCompare(a.id))` therefore
/// reduces to a descending compare on `(lowercase, original)`.
fn locale_key(id: &str) -> (String, String) {
    (id.to_lowercase(), id.to_owned())
}

/// `findExactModelReferenceMatch(modelReference, availableModels)`
pub fn find_exact_model_reference_match(
    model_reference: &str,
    available_models: &[Model],
) -> Option<Model> {
    let trimmed_reference = model_reference.trim();
    if trimmed_reference.is_empty() {
        return None;
    }
    let normalized_reference = trimmed_reference.to_lowercase();

    let canonical_matches: Vec<&Model> = available_models
        .iter()
        .filter(|model| {
            format!("{}/{}", model.provider, model.id).to_lowercase() == normalized_reference
        })
        .collect();
    if canonical_matches.len() == 1 {
        return Some(canonical_matches[0].clone());
    }
    if canonical_matches.len() > 1 {
        return None;
    }

    if let Some(slash_index) = trimmed_reference.find('/') {
        let provider = trimmed_reference[..slash_index].trim();
        let model_id = trimmed_reference[slash_index + 1..].trim();
        if !provider.is_empty() && !model_id.is_empty() {
            let provider_matches: Vec<&Model> = available_models
                .iter()
                .filter(|model| {
                    model.provider.to_lowercase() == provider.to_lowercase()
                        && model.id.to_lowercase() == model_id.to_lowercase()
                })
                .collect();
            if provider_matches.len() == 1 {
                return Some(provider_matches[0].clone());
            }
            if provider_matches.len() > 1 {
                return None;
            }
        }
    }

    let id_matches: Vec<&Model> = available_models
        .iter()
        .filter(|model| model.id.to_lowercase() == normalized_reference)
        .collect();
    (id_matches.len() == 1).then(|| id_matches[0].clone())
}

/// `tryMatchModel(modelPattern, availableModels)`
fn try_match_model(model_pattern: &str, available_models: &[Model]) -> Option<Model> {
    if let Some(exact_match) = find_exact_model_reference_match(model_pattern, available_models) {
        return Some(exact_match);
    }
    let needle = model_pattern.to_lowercase();
    let matches: Vec<&Model> = available_models
        .iter()
        .filter(|model| {
            model.id.to_lowercase().contains(&needle) || model.name.to_lowercase().contains(&needle)
        })
        .collect();
    if matches.is_empty() {
        return None;
    }
    let aliases: Vec<&Model> = matches
        .iter()
        .copied()
        .filter(|model| is_alias(&model.id))
        .collect();
    let mut candidates: Vec<&Model> = if aliases.is_empty() {
        matches
            .iter()
            .copied()
            .filter(|model| !is_alias(&model.id))
            .collect()
    } else {
        aliases
    };
    // Prefer alias — if several, the one that sorts highest.
    candidates.sort_by_key(|model| std::cmp::Reverse(locale_key(&model.id)));
    candidates.first().map(|model| (*model).clone())
}

/// `ParsedModelResult`
#[derive(Debug, Clone, Default)]
pub struct ParsedModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
}

/// `buildFallbackModel(provider, modelId, availableModels)`
fn build_fallback_model(
    provider: &str,
    model_id: &str,
    available_models: &[Model],
) -> Option<Model> {
    let provider_models: Vec<&Model> = available_models
        .iter()
        .filter(|model| model.provider == provider)
        .collect();
    let base_model = match default_model_for_provider(provider) {
        Some(default_id) => provider_models
            .iter()
            .copied()
            .find(|model| model.id == default_id)
            .or_else(|| provider_models.first().copied())?,
        None => provider_models.first().copied()?,
    };
    Some(Model {
        id: model_id.to_owned(),
        name: model_id.to_owned(),
        ..base_model.clone()
    })
}

/// `parseModelPattern(pattern, availableModels, options?)`
pub fn parse_model_pattern(
    pattern: &str,
    available_models: &[Model],
    allow_invalid_thinking_level_fallback: bool,
) -> ParsedModelResult {
    if let Some(exact_match) = try_match_model(pattern, available_models) {
        return ParsedModelResult {
            model: Some(exact_match),
            thinking_level: None,
            warning: None,
        };
    }

    let Some(last_colon_index) = pattern.rfind(':') else {
        return ParsedModelResult::default();
    };
    let prefix = &pattern[..last_colon_index];
    let suffix = &pattern[last_colon_index + 1..];

    if let Some(level) = parse_thinking_level(suffix) {
        let result = parse_model_pattern(
            prefix,
            available_models,
            allow_invalid_thinking_level_fallback,
        );
        if result.model.is_some() {
            // Only use this thinking level if no warning came from the inner recursion.
            return ParsedModelResult {
                thinking_level: result.warning.is_none().then_some(level),
                ..result
            };
        }
        return result;
    }

    if !allow_invalid_thinking_level_fallback {
        // Strict mode (CLI --model parsing): treat it as part of the model id and fail.
        return ParsedModelResult::default();
    }
    let result = parse_model_pattern(
        prefix,
        available_models,
        allow_invalid_thinking_level_fallback,
    );
    if result.model.is_some() {
        return ParsedModelResult {
            model: result.model,
            thinking_level: None,
            warning: Some(format!(
                "Invalid thinking level \"{suffix}\" in pattern \"{pattern}\". Using default instead."
            )),
        };
    }
    result
}

/// `ModelScopeDiagnostic`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelScopeDiagnostic {
    pub code: ModelScopeDiagnosticCode,
    pub message: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelScopeDiagnosticCode {
    NoMatch,
    InvalidThinkingLevel,
}

/// `ResolveModelScopeResult`
#[derive(Debug, Clone, Default)]
pub struct ResolveModelScopeResult {
    pub scoped_models: Vec<ScopedModel>,
    pub diagnostics: Vec<ModelScopeDiagnostic>,
}

/// `minimatch(value, pattern, { nocase: true })`
/// Deviation (class 3, master substitution glob/minimatch → globset): `globset` with
/// `literal_separator(true)` keeps minimatch's rule that `*` does not cross `/`.
fn minimatch_nocase(value: &str, pattern: &str) -> bool {
    GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .backslash_escape(true)
        .empty_alternates(true)
        .build()
        .ok()
        .is_some_and(|glob| glob.compile_matcher().is_match(value))
}

/// `resolveModelScopeFromModels(patterns, models)`
pub fn resolve_model_scope_from_models(
    patterns: &[String],
    models: &[Model],
) -> ResolveModelScopeResult {
    let available_models = models.to_vec();
    let mut scoped_models: Vec<ScopedModel> = Vec::new();
    let mut diagnostics: Vec<ModelScopeDiagnostic> = Vec::new();

    let push_unique = |scoped_models: &mut Vec<ScopedModel>,
                       model: Model,
                       thinking_level: Option<ThinkingLevel>| {
        if !scoped_models
            .iter()
            .any(|scoped| models_are_equal(Some(&scoped.model), Some(&model)))
        {
            scoped_models.push(ScopedModel {
                model,
                thinking_level,
            });
        }
    };

    for pattern in patterns {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let mut glob_pattern = pattern.as_str();
            let mut thinking_level: Option<ThinkingLevel> = None;
            if let Some(colon_index) = pattern.rfind(':')
                && let Some(level) = parse_thinking_level(&pattern[colon_index + 1..])
            {
                thinking_level = Some(level);
                glob_pattern = &pattern[..colon_index];
            }

            if let Some(exact_match) =
                find_exact_model_reference_match(glob_pattern, &available_models)
            {
                push_unique(&mut scoped_models, exact_match, thinking_level);
                continue;
            }

            // Match against "provider/modelId" OR just the model id, so "*sonnet*"
            // works without requiring "anthropic/*sonnet*".
            let matching_models: Vec<&Model> = available_models
                .iter()
                .filter(|model| {
                    let full_id = format!("{}/{}", model.provider, model.id);
                    minimatch_nocase(&full_id, glob_pattern)
                        || minimatch_nocase(&model.id, glob_pattern)
                })
                .collect();

            if matching_models.is_empty() {
                diagnostics.push(ModelScopeDiagnostic {
                    code: ModelScopeDiagnosticCode::NoMatch,
                    message: format!("No models match pattern \"{pattern}\""),
                    pattern: pattern.clone(),
                });
                continue;
            }
            for model in matching_models {
                push_unique(&mut scoped_models, model.clone(), thinking_level);
            }
            continue;
        }

        let result = parse_model_pattern(pattern, &available_models, true);
        if let Some(warning) = &result.warning {
            diagnostics.push(ModelScopeDiagnostic {
                code: ModelScopeDiagnosticCode::InvalidThinkingLevel,
                message: warning.clone(),
                pattern: pattern.clone(),
            });
        }
        let Some(model) = result.model else {
            diagnostics.push(ModelScopeDiagnostic {
                code: ModelScopeDiagnosticCode::NoMatch,
                message: format!("No models match pattern \"{pattern}\""),
                pattern: pattern.clone(),
            });
            continue;
        };
        push_unique(&mut scoped_models, model, result.thinking_level);
    }

    ResolveModelScopeResult {
        scoped_models,
        diagnostics,
    }
}

/// `resolveModelScopeWithDiagnostics(patterns, modelRuntime, options?)`
pub async fn resolve_model_scope_with_diagnostics(
    patterns: &[String],
    model_runtime: &dyn ModelCatalog,
    options: Option<AuthOperationOptions>,
) -> ResolveModelScopeResult {
    let models = model_runtime.get_available(options).await;
    resolve_model_scope_from_models(patterns, &models)
}

/// `resolveModelScope(patterns, modelRuntime, options?)`
/// Deviation (class 1): the diagnostics are returned instead of written to
/// `console.warn`, so the caller owns the output channel.
pub async fn resolve_model_scope(
    patterns: &[String],
    model_runtime: &dyn ModelCatalog,
    options: Option<AuthOperationOptions>,
) -> ResolveModelScopeResult {
    resolve_model_scope_with_diagnostics(patterns, model_runtime, options).await
}

/// `ResolveCliModelResult`
#[derive(Debug, Clone, Default)]
pub struct ResolveCliModelResult {
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub warning: Option<String>,
    /// Error message suitable for CLI display; `model` is `None` when set.
    pub error: Option<String>,
}

/// `resolveCliModel(options)`
pub fn resolve_cli_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_thinking: Option<ThinkingLevel>,
    model_runtime: &dyn ModelCatalog,
) -> ResolveCliModelResult {
    let Some(cli_model) = cli_model else {
        return ResolveCliModelResult::default();
    };

    // Use *all* models here, not just models with pre-configured auth: this allows
    // "--api-key" to be used for first-time setup.
    let available_models = model_runtime.get_models();
    if available_models.is_empty() {
        return ResolveCliModelResult {
            error: Some(
                "No models available. Check your installation or add models to models.json."
                    .to_owned(),
            ),
            ..ResolveCliModelResult::default()
        };
    }

    let canonical_provider = |needle: &str| -> Option<String> {
        available_models
            .iter()
            .rev()
            .find(|model| model.provider.to_lowercase() == needle.to_lowercase())
            .map(|model| model.provider.clone())
    };

    let mut provider = cli_provider.and_then(canonical_provider);
    if cli_provider.is_some() && provider.is_none() {
        return ResolveCliModelResult {
            error: Some(format!(
                "Unknown provider \"{}\". Use --list-models to see available providers/models.",
                cli_provider.unwrap_or_default()
            )),
            ..ResolveCliModelResult::default()
        };
    }

    // Without an explicit --provider, try "provider/model" first: when the prefix
    // before the first slash names a known provider, prefer that reading over model
    // ids that literally contain slashes.
    let mut pattern = cli_model.to_owned();
    let mut inferred_provider = false;

    if provider.is_none()
        && let Some(slash_index) = cli_model.find('/')
        && let Some(canonical) = canonical_provider(&cli_model[..slash_index])
    {
        provider = Some(canonical);
        pattern = cli_model[slash_index + 1..].to_owned();
        inferred_provider = true;
    }

    // No provider inferred from the slash: try exact matches without provider
    // inference, for model ids that naturally contain slashes.
    if provider.is_none() {
        let lower = cli_model.to_lowercase();
        let exact_matches: Vec<&Model> = available_models
            .iter()
            .filter(|model| {
                model.id.to_lowercase() == lower
                    || format!("{}/{}", model.provider, model.id).to_lowercase() == lower
            })
            .collect();
        if exact_matches.len() == 1 {
            return ResolveCliModelResult {
                model: Some(exact_matches[0].clone()),
                ..ResolveCliModelResult::default()
            };
        }
        if exact_matches.len() > 1 {
            let authenticated: Vec<&Model> = exact_matches
                .iter()
                .copied()
                .filter(|model| model_runtime.has_configured_auth(&model.provider))
                .collect();
            if authenticated.len() == 1 {
                return ResolveCliModelResult {
                    model: Some(authenticated[0].clone()),
                    ..ResolveCliModelResult::default()
                };
            }
            let mut names: Vec<String> = exact_matches
                .iter()
                .map(|model| format!("{}/{}", model.provider, model.id))
                .collect();
            names.sort_by_key(|name| locale_key(name));
            let auth_hint = if authenticated.is_empty() {
                "No matching provider is authenticated."
            } else {
                "More than one matching provider is authenticated."
            };
            return ResolveCliModelResult {
                error: Some(format!(
                    "Model \"{cli_model}\" is ambiguous across providers: {}. {auth_hint} Use --provider or provider/model.",
                    names.join(", ")
                )),
                ..ResolveCliModelResult::default()
            };
        }
    }

    if let (Some(_), Some(provider)) = (cli_provider, provider.as_ref()) {
        // Both were provided: tolerate --model <provider>/<pattern>.
        let prefix = format!("{provider}/");
        if cli_model.to_lowercase().starts_with(&prefix.to_lowercase()) {
            pattern = cli_model[prefix.len()..].to_owned();
        }
    }

    let candidates: Vec<Model> = match provider.as_ref() {
        Some(provider) => available_models
            .iter()
            .filter(|model| &model.provider == provider)
            .cloned()
            .collect(),
        None => available_models.clone(),
    };
    let parsed = parse_model_pattern(&pattern, &candidates, false);

    if let Some(model) = parsed.model.clone() {
        // If provider inference matched an unauthenticated provider/model pair, prefer
        // one exact raw model-id match that is authenticated.
        if inferred_provider {
            let raw_exact_matches: Vec<&Model> = available_models
                .iter()
                .filter(|entry| {
                    entry.id.to_lowercase() == cli_model.to_lowercase()
                        && !models_are_equal(Some(entry), Some(&model))
                })
                .collect();
            if !raw_exact_matches.is_empty() && !model_runtime.has_configured_auth(&model.provider)
            {
                let authenticated: Vec<&Model> = raw_exact_matches
                    .iter()
                    .copied()
                    .filter(|entry| model_runtime.has_configured_auth(&entry.provider))
                    .collect();
                if authenticated.len() == 1 {
                    return ResolveCliModelResult {
                        model: Some(authenticated[0].clone()),
                        ..ResolveCliModelResult::default()
                    };
                }
            }
        }
        return ResolveCliModelResult {
            model: Some(model),
            thinking_level: parsed.thinking_level,
            warning: parsed.warning,
            error: None,
        };
    }

    // Provider inferred from the slash but no match inside it: fall back to matching
    // the full input as a raw model id across all models.
    if inferred_provider {
        let lower = cli_model.to_lowercase();
        if let Some(exact) = available_models.iter().find(|model| {
            model.id.to_lowercase() == lower
                || format!("{}/{}", model.provider, model.id).to_lowercase() == lower
        }) {
            return ResolveCliModelResult {
                model: Some(exact.clone()),
                ..ResolveCliModelResult::default()
            };
        }
        let fallback = parse_model_pattern(cli_model, &available_models, false);
        if fallback.model.is_some() {
            return ResolveCliModelResult {
                model: fallback.model,
                thinking_level: fallback.thinking_level,
                warning: fallback.warning,
                error: None,
            };
        }
    }

    if let Some(provider) = provider.as_ref() {
        // Parse a thinking-level suffix before building the fallback model, but only
        // when --thinking is not explicitly provided.
        let mut fallback_pattern = pattern.clone();
        let mut fallback_thinking: Option<ThinkingLevel> = None;
        if cli_thinking.is_none()
            && let Some(last_colon) = pattern.rfind(':')
            && let Some(level) = parse_thinking_level(&pattern[last_colon + 1..])
        {
            fallback_pattern = pattern[..last_colon].to_owned();
            fallback_thinking = Some(level);
        }

        if let Some(fallback_model) =
            build_fallback_model(provider, &fallback_pattern, &available_models)
        {
            let requested_thinking = cli_thinking.or(fallback_thinking);
            let model = match requested_thinking {
                Some(level) if level != ThinkingLevel::Off => Model {
                    reasoning: true,
                    ..fallback_model
                },
                _ => fallback_model,
            };
            let tail = format!(
                "Model \"{fallback_pattern}\" not found for provider \"{provider}\". Using custom model id."
            );
            let fallback_warning = match &parsed.warning {
                Some(warning) => format!("{warning} {tail}"),
                None => tail,
            };
            return ResolveCliModelResult {
                model: Some(model),
                thinking_level: fallback_thinking,
                warning: Some(fallback_warning),
                error: None,
            };
        }
    }

    let display = match provider.as_ref() {
        Some(provider) => format!("{provider}/{pattern}"),
        None => cli_model.to_owned(),
    };
    ResolveCliModelResult {
        model: None,
        thinking_level: None,
        warning: parsed.warning,
        error: Some(format!(
            "Model \"{display}\" not found. Use --list-models to see available models."
        )),
    }
}

/// `InitialModelResult`
#[derive(Debug, Clone)]
pub struct InitialModelResult {
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    pub fallback_message: Option<String>,
}

pub struct FindInitialModelOptions<'a> {
    pub cli_provider: Option<&'a str>,
    pub cli_model: Option<&'a str>,
    pub scoped_models: &'a [ScopedModel],
    pub is_continuing: bool,
    pub default_provider: Option<&'a str>,
    pub default_model_id: Option<&'a str>,
    pub default_thinking_level: Option<ThinkingLevel>,
    pub model_runtime: &'a dyn ModelCatalog,
}

/// `findInitialModel(options)`
/// Deviation (class 1): a CLI resolution error is returned instead of calling
/// `process.exit(1)`; the caller (the CLI entry point) owns the exit.
pub fn find_initial_model(
    options: FindInitialModelOptions<'_>,
) -> Result<InitialModelResult, String> {
    // 1. CLI args take priority.
    if let (Some(cli_provider), Some(cli_model)) = (options.cli_provider, options.cli_model) {
        let resolved = resolve_cli_model(
            Some(cli_provider),
            Some(cli_model),
            None,
            options.model_runtime,
        );
        if let Some(error) = resolved.error {
            return Err(error);
        }
        if let Some(model) = resolved.model {
            return Ok(InitialModelResult {
                model: Some(model),
                thinking_level: DEFAULT_THINKING_LEVEL,
                fallback_message: None,
            });
        }
    }

    // 2. Use the first scoped model (skip when continuing/resuming).
    if let Some(first) = options.scoped_models.first()
        && !options.is_continuing
    {
        return Ok(InitialModelResult {
            model: Some(first.model.clone()),
            thinking_level: first
                .thinking_level
                .or(options.default_thinking_level)
                .unwrap_or(DEFAULT_THINKING_LEVEL),
            fallback_message: None,
        });
    }

    // 3. Saved default from settings, if auth is configured.
    if let (Some(default_provider), Some(default_model_id)) =
        (options.default_provider, options.default_model_id)
        && let Some(found) = options
            .model_runtime
            .get_model(default_provider, default_model_id)
        && options.model_runtime.has_configured_auth(&found.provider)
    {
        return Ok(InitialModelResult {
            thinking_level: options
                .default_thinking_level
                .unwrap_or(DEFAULT_THINKING_LEVEL),
            model: Some(found),
            fallback_message: None,
        });
    }

    // 4. First available model with valid API key.
    let available_models = options.model_runtime.get_available_snapshot();
    if let Some(model) = pick_default_available(&available_models) {
        return Ok(InitialModelResult {
            model: Some(model),
            thinking_level: DEFAULT_THINKING_LEVEL,
            fallback_message: None,
        });
    }

    // 5. No model found.
    Ok(InitialModelResult {
        model: None,
        thinking_level: DEFAULT_THINKING_LEVEL,
        fallback_message: None,
    })
}

/// The shared tail of steps 4/5: a known provider default, else the first available.
fn pick_default_available(available_models: &[Model]) -> Option<Model> {
    if available_models.is_empty() {
        return None;
    }
    for (provider, default_id) in DEFAULT_MODEL_PER_PROVIDER {
        if let Some(model) = available_models
            .iter()
            .find(|model| model.provider == provider && model.id == default_id)
        {
            return Some(model.clone());
        }
    }
    available_models.first().cloned()
}

/// `RestoredModel`
#[derive(Debug, Clone, Default)]
pub struct RestoredModel {
    pub model: Option<Model>,
    pub fallback_message: Option<String>,
    /// `shouldPrintMessages` is set; returned so the caller owns the output channel.
    pub messages: Vec<RestoreMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreMessage {
    Info(String),
    Warning(String),
}

/// `restoreModelFromSession(savedProvider, savedModelId, currentModel, shouldPrintMessages, modelRuntime)`
pub fn restore_model_from_session(
    saved_provider: &str,
    saved_model_id: &str,
    current_model: Option<&Model>,
    should_print_messages: bool,
    model_runtime: &dyn ModelCatalog,
) -> RestoredModel {
    let restored_model = model_runtime.get_model(saved_provider, saved_model_id);
    let has_configured_auth = restored_model
        .as_ref()
        .is_some_and(|model| model_runtime.has_configured_auth(&model.provider));

    let mut messages = Vec::new();
    if let Some(restored_model) = restored_model.clone()
        && has_configured_auth
    {
        if should_print_messages {
            messages.push(RestoreMessage::Info(format!(
                "Restored model: {saved_provider}/{saved_model_id}"
            )));
        }
        return RestoredModel {
            model: Some(restored_model),
            fallback_message: None,
            messages,
        };
    }

    let reason = if restored_model.is_none() {
        "model no longer exists"
    } else {
        "no auth configured"
    };
    if should_print_messages {
        messages.push(RestoreMessage::Warning(format!(
            "Could not restore model {saved_provider}/{saved_model_id} ({reason})."
        )));
    }

    if let Some(current_model) = current_model {
        if should_print_messages {
            messages.push(RestoreMessage::Info(format!(
                "Falling back to: {}/{}",
                current_model.provider, current_model.id
            )));
        }
        return RestoredModel {
            fallback_message: Some(format!(
                "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
                current_model.provider, current_model.id
            )),
            model: Some(current_model.clone()),
            messages,
        };
    }

    let available_models = model_runtime.get_available_snapshot();
    if let Some(fallback_model) = pick_default_available(&available_models) {
        if should_print_messages {
            messages.push(RestoreMessage::Info(format!(
                "Falling back to: {}/{}",
                fallback_model.provider, fallback_model.id
            )));
        }
        return RestoredModel {
            fallback_message: Some(format!(
                "Could not restore model {saved_provider}/{saved_model_id} ({reason}). Using {}/{}.",
                fallback_model.provider, fallback_model.id
            )),
            model: Some(fallback_model),
            messages,
        };
    }

    RestoredModel {
        model: None,
        fallback_message: None,
        messages,
    }
}
