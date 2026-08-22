//! Immutable, credential-blind models.json snapshot.
//!
//! Port of `packages/coding-agent/src/core/model-config.ts` (298 LOC).

use std::collections::BTreeMap;

use notagent_ai::types::{Modality, ModelCostTier, ThinkingLevelMap};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::utils::json::strip_json_comments;
use crate::utils::paths::normalize_path_default;

// ---------------------------------------------------------------------------
// Typed shape of models.json
//
// Deviation (class 3): TypeBox `Compile(ModelsConfigSchema)` becomes a
// handwritten validator (`validate_models_config`) plus serde deserialization.
// The messages follow TypeBox's localized wording ("Expected string", "Expected
// required property", …) and the same `<path>: <message>` layout; they are not
// pinned by any TS test.
//
// `compat` stays raw JSON: the TS schema is a union of three all-optional
// objects, so every object passes, and the composer merges it key-wise.
// ---------------------------------------------------------------------------

/// `Static<typeof ModelDefinitionSchema>`
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsJsonModel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    #[serde(default)]
    pub input: Option<Vec<Modality>>,
    #[serde(default)]
    pub cost: Option<ModelsJsonCost>,
    #[serde(default)]
    pub context_window: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<f64>,
    #[serde(default)]
    pub sampling_params: Option<Map<String, Value>>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub compat: Option<Value>,
}

/// `ModelCostSchema` — all four rates required, tiers optional.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsJsonCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    #[serde(default)]
    pub tiers: Option<Vec<ModelCostTier>>,
}

/// The cost half of `ModelOverrideSchema` — every rate optional.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsJsonCostOverride {
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub tiers: Option<Vec<ModelCostTier>>,
}

/// `Static<typeof ModelOverrideSchema>`
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsJsonModelOverride {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    #[serde(default)]
    pub input: Option<Vec<Modality>>,
    #[serde(default)]
    pub cost: Option<ModelsJsonCostOverride>,
    #[serde(default)]
    pub context_window: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<f64>,
    #[serde(default)]
    pub sampling_params: Option<Map<String, Value>>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub compat: Option<Value>,
}

/// `Static<typeof ProviderConfigSchema>`
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsJsonProvider {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api: Option<String>,
    /// `Type.Literal("radius")`
    #[serde(default)]
    pub oauth: Option<String>,
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub compat: Option<Value>,
    #[serde(default)]
    pub auth_header: Option<bool>,
    #[serde(default)]
    pub models: Option<Vec<ModelsJsonModel>>,
    #[serde(default)]
    pub model_overrides: Option<BTreeMap<String, ModelsJsonModelOverride>>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

struct Validator {
    errors: Vec<String>,
}

/// `formatValidationPath(error)` — `/a/b` → `a.b`, empty → `root`.
fn format_path(path: &[String]) -> String {
    if path.is_empty() {
        "root".to_owned()
    } else {
        path.join(".")
    }
}

const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const MODALITIES: [&str; 2] = ["text", "image"];
const MAX_TOKENS_FIELDS: [&str; 2] = ["max_completion_tokens", "max_tokens"];
const THINKING_FORMATS: [&str; 11] = [
    "openai",
    "openrouter",
    "together",
    "baseten",
    "deepseek",
    "zai",
    "qwen",
    "chat-template",
    "qwen-chat-template",
    "string-thinking",
    "ant-ling",
];
const SESSION_AFFINITY_FORMATS: [&str; 4] = ["openai", "openai-nosession", "openrouter", "mtplx"];

impl Validator {
    fn push(&mut self, path: &[String], message: &str) {
        self.errors
            .push(format!("  - {}: {message}", format_path(path)));
    }

    fn required(&mut self, path: &[String], object: &Map<String, Value>, key: &str) -> bool {
        if object.contains_key(key) {
            return true;
        }
        let mut path = path.to_vec();
        path.push(key.to_owned());
        self.push(&path, "Expected required property");
        false
    }

    fn child(path: &[String], key: &str) -> Vec<String> {
        let mut child = path.to_vec();
        child.push(key.to_owned());
        child
    }

    fn object<'a>(&mut self, path: &[String], value: &'a Value) -> Option<&'a Map<String, Value>> {
        match value.as_object() {
            Some(object) => Some(object),
            None => {
                self.push(path, "Expected object");
                None
            }
        }
    }

    fn string(&mut self, path: &[String], value: &Value, min_length: usize) {
        match value.as_str() {
            Some(text) if text.chars().count() >= min_length => {}
            Some(_) => self.push(
                path,
                &format!("Expected string length greater or equal to {min_length}"),
            ),
            None => self.push(path, "Expected string"),
        }
    }

    fn number(&mut self, path: &[String], value: &Value) {
        if !value.is_number() {
            self.push(path, "Expected number");
        }
    }

    fn boolean(&mut self, path: &[String], value: &Value) {
        if !value.is_boolean() {
            self.push(path, "Expected boolean");
        }
    }

    fn literal_union(&mut self, path: &[String], value: &Value, allowed: &[&str]) {
        let matches = value.as_str().is_some_and(|text| allowed.contains(&text));
        if !matches {
            self.push(path, "Expected union value");
        }
    }

    fn string_record(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        for (key, entry) in object.clone() {
            self.string(&Self::child(path, &key), &entry, 0);
        }
    }

    fn unknown_record(&mut self, path: &[String], value: &Value) {
        self.object(path, value);
    }

    fn modality_array(&mut self, path: &[String], value: &Value) {
        let Some(items) = value.as_array() else {
            self.push(path, "Expected array");
            return;
        };
        for (index, item) in items.iter().enumerate() {
            self.literal_union(&Self::child(path, &index.to_string()), item, &MODALITIES);
        }
    }

    fn string_array(&mut self, path: &[String], value: &Value) {
        let Some(items) = value.as_array() else {
            self.push(path, "Expected array");
            return;
        };
        for (index, item) in items.iter().enumerate() {
            self.string(&Self::child(path, &index.to_string()), item, 0);
        }
    }

    fn thinking_level_map(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        for level in THINKING_LEVELS {
            let Some(entry) = object.get(level) else {
                continue;
            };
            if !entry.is_null() && !entry.is_string() {
                self.push(&Self::child(path, level), "Expected union value");
            }
        }
    }

    fn cost_rates(&mut self, path: &[String], object: &Map<String, Value>, required: bool) {
        for key in ["input", "output", "cacheRead", "cacheWrite"] {
            match object.get(key) {
                Some(entry) => self.number(&Self::child(path, key), entry),
                None if required => {
                    self.push(&Self::child(path, key), "Expected required property");
                }
                None => {}
            }
        }
    }

    fn cost_tiers(&mut self, path: &[String], value: &Value) {
        let Some(items) = value.as_array() else {
            self.push(path, "Expected array");
            return;
        };
        for (index, item) in items.iter().enumerate() {
            let path = Self::child(path, &index.to_string());
            let Some(object) = self.object(&path, item) else {
                continue;
            };
            let object = object.clone();
            if self.required(&path, &object, "inputTokensAbove")
                && let Some(entry) = object.get("inputTokensAbove")
            {
                self.number(&Self::child(&path, "inputTokensAbove"), entry);
            }
            for key in ["input", "output", "cacheRead", "cacheWrite"] {
                if self.required(&path, &object, key)
                    && let Some(entry) = object.get(key)
                {
                    self.number(&Self::child(&path, key), entry);
                }
            }
        }
    }

    fn cost(&mut self, path: &[String], value: &Value, rates_required: bool) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        let object = object.clone();
        self.cost_rates(path, &object, rates_required);
        if let Some(tiers) = object.get("tiers") {
            self.cost_tiers(&Self::child(path, "tiers"), tiers);
        }
    }

    fn chat_template_kwargs(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        for (key, entry) in object.clone() {
            let path = Self::child(path, &key);
            let scalar =
                entry.is_string() || entry.is_number() || entry.is_boolean() || entry.is_null();
            if scalar {
                continue;
            }
            let variable = entry.as_object().is_some_and(|object| {
                object
                    .get("$var")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == "thinking.enabled" || value == "thinking.effort")
                    && object
                        .get("omitWhenOff")
                        .is_none_or(|value| value.is_boolean())
            });
            if !variable {
                self.push(&path, "Expected union value");
            }
        }
    }

    fn routing(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        for (key, entry) in object.clone() {
            let path = Self::child(path, &key);
            match key.as_str() {
                "allow_fallbacks" | "require_parameters" | "zdr" | "enforce_distillable_text" => {
                    self.boolean(&path, &entry);
                }
                "data_collection" => self.literal_union(&path, &entry, &["deny", "allow"]),
                "order" | "only" | "ignore" | "quantizations" => self.string_array(&path, &entry),
                "sort" if !entry.is_string() && !entry.is_object() => {
                    self.push(&path, "Expected union value");
                }
                "max_price" => {
                    self.object(&path, &entry);
                }
                "preferred_min_throughput" | "preferred_max_latency"
                    if !entry.is_number() && !entry.is_object() =>
                {
                    self.push(&path, "Expected union value");
                }
                _ => {}
            }
        }
    }

    /// `ProviderCompatSchema` — a union of three all-optional objects, so only the
    /// shape of the known keys is checked.
    fn compat(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        for (key, entry) in object.clone() {
            let path = Self::child(path, &key);
            match key.as_str() {
                "supportsStore"
                | "supportsDeveloperRole"
                | "supportsReasoningEffort"
                | "supportsUsageInStreaming"
                | "requiresToolResultName"
                | "requiresAssistantAfterToolResult"
                | "requiresThinkingAsText"
                | "requiresReasoningContentOnAssistantMessages"
                | "supportsOpenAIGrammarTools"
                | "supportsStrictMode"
                | "sendSessionAffinityHeaders"
                | "supportsLongCacheRetention"
                | "supportsAdditionalTools"
                | "supportsToolSearch"
                | "supportsEagerToolInputStreaming"
                | "supportsCacheControlOnTools"
                | "supportsTemperature"
                | "forceAdaptiveThinking"
                | "allowEmptySignature"
                | "supportsStrictTools"
                | "supportsToolReferences" => self.boolean(&path, &entry),
                "maxTokensField" => self.literal_union(&path, &entry, &MAX_TOKENS_FIELDS),
                "thinkingFormat" => self.literal_union(&path, &entry, &THINKING_FORMATS),
                "sessionAffinityFormat" => {
                    self.literal_union(&path, &entry, &SESSION_AFFINITY_FORMATS);
                }
                "cacheControlFormat" => self.literal_union(&path, &entry, &["anthropic"]),
                "deferredToolsMode" => self.literal_union(&path, &entry, &["kimi"]),
                "chatTemplateKwargs" | "chatTemplateArgs" => {
                    self.chat_template_kwargs(&path, &entry);
                }
                "openRouterRouting" => self.routing(&path, &entry),
                "vercelGatewayRouting" => {
                    let Some(object) = self.object(&path, &entry) else {
                        continue;
                    };
                    for (key, entry) in object.clone() {
                        if key == "only" || key == "order" {
                            self.string_array(&Self::child(&path, &key), &entry);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn model_definition(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        let object = object.clone();
        if self.required(path, &object, "id")
            && let Some(entry) = object.get("id")
        {
            self.string(&Self::child(path, "id"), entry, 1);
        }
        self.model_shared(path, &object, 1);
        if let Some(entry) = object.get("api") {
            self.string(&Self::child(path, "api"), entry, 1);
        }
        if let Some(entry) = object.get("baseUrl") {
            self.string(&Self::child(path, "baseUrl"), entry, 1);
        }
        if let Some(entry) = object.get("cost") {
            self.cost(&Self::child(path, "cost"), entry, true);
        }
    }

    fn model_override(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        let object = object.clone();
        self.model_shared(path, &object, 1);
        if let Some(entry) = object.get("cost") {
            self.cost(&Self::child(path, "cost"), entry, false);
        }
    }

    /// Fields shared by `ModelDefinitionSchema` and `ModelOverrideSchema`.
    fn model_shared(&mut self, path: &[String], object: &Map<String, Value>, name_min: usize) {
        if let Some(entry) = object.get("name") {
            self.string(&Self::child(path, "name"), entry, name_min);
        }
        if let Some(entry) = object.get("reasoning") {
            self.boolean(&Self::child(path, "reasoning"), entry);
        }
        if let Some(entry) = object.get("thinkingLevelMap") {
            self.thinking_level_map(&Self::child(path, "thinkingLevelMap"), entry);
        }
        if let Some(entry) = object.get("input") {
            self.modality_array(&Self::child(path, "input"), entry);
        }
        if let Some(entry) = object.get("contextWindow") {
            self.number(&Self::child(path, "contextWindow"), entry);
        }
        if let Some(entry) = object.get("maxTokens") {
            self.number(&Self::child(path, "maxTokens"), entry);
        }
        if let Some(entry) = object.get("samplingParams") {
            self.unknown_record(&Self::child(path, "samplingParams"), entry);
        }
        if let Some(entry) = object.get("headers") {
            self.string_record(&Self::child(path, "headers"), entry);
        }
        if let Some(entry) = object.get("compat") {
            self.compat(&Self::child(path, "compat"), entry);
        }
    }

    fn provider(&mut self, path: &[String], value: &Value) {
        let Some(object) = self.object(path, value) else {
            return;
        };
        let object = object.clone();
        for key in ["name", "baseUrl", "apiKey", "api"] {
            if let Some(entry) = object.get(key) {
                self.string(&Self::child(path, key), entry, 1);
            }
        }
        if let Some(entry) = object.get("oauth") {
            self.literal_union(&Self::child(path, "oauth"), entry, &["radius"]);
        }
        if let Some(entry) = object.get("headers") {
            self.string_record(&Self::child(path, "headers"), entry);
        }
        if let Some(entry) = object.get("compat") {
            self.compat(&Self::child(path, "compat"), entry);
        }
        if let Some(entry) = object.get("authHeader") {
            self.boolean(&Self::child(path, "authHeader"), entry);
        }
        if let Some(entry) = object.get("models") {
            let path = Self::child(path, "models");
            match entry.as_array() {
                Some(items) => {
                    for (index, item) in items.iter().enumerate() {
                        self.model_definition(&Self::child(&path, &index.to_string()), item);
                    }
                }
                None => self.push(&path, "Expected array"),
            }
        }
        if let Some(entry) = object.get("modelOverrides") {
            let path = Self::child(path, "modelOverrides");
            match entry.as_object() {
                Some(overrides) => {
                    for (key, item) in overrides.clone() {
                        self.model_override(&Self::child(&path, &key), &item);
                    }
                }
                None => self.push(&path, "Expected object"),
            }
        }
    }
}

/// `validateModelsConfig.Check(parsed)` plus `.Errors(parsed)`.
fn validate_models_config(value: &Value) -> Vec<String> {
    let mut validator = Validator { errors: Vec::new() };
    let root: Vec<String> = Vec::new();
    let Some(object) = validator.object(&root, value) else {
        return validator.errors;
    };
    let object = object.clone();
    if !validator.required(&root, &object, "providers") {
        return validator.errors;
    }
    let providers_path = Validator::child(&root, "providers");
    let Some(providers) = object.get("providers") else {
        return validator.errors;
    };
    match providers.as_object() {
        Some(providers) => {
            for (provider_id, provider) in providers.clone() {
                validator.provider(&Validator::child(&providers_path, &provider_id), &provider);
            }
        }
        None => validator.push(&providers_path, "Expected object"),
    }
    validator.errors
}

/// One immutable load of models.json.
#[derive(Debug, Clone, Default)]
pub struct ModelConfig {
    providers: BTreeMap<String, ModelsJsonProvider>,
    /// Insertion order of the JSON object, which `getProviderIds` preserves.
    order: Vec<String>,
    error: Option<String>,
}

impl ModelConfig {
    fn empty() -> Self {
        ModelConfig::default()
    }

    fn failed(error: String) -> Self {
        ModelConfig {
            error: Some(error),
            ..ModelConfig::default()
        }
    }

    /// `ModelConfig.load(modelsJsonPath)`
    pub async fn load(models_json_path: Option<&str>) -> Self {
        let Some(models_json_path) = models_json_path else {
            return ModelConfig::empty();
        };
        let path = match normalize_path_default(models_json_path) {
            Ok(path) => path,
            Err(error) => {
                return ModelConfig::failed(format!(
                    "Failed to load models.json: {error}\n\nFile: {models_json_path}"
                ));
            }
        };
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ModelConfig::empty();
            }
            Err(error) => {
                return ModelConfig::failed(format!(
                    "Failed to load models.json: {error}\n\nFile: {path}"
                ));
            }
        };
        ModelConfig::parse(&content, &path)
    }

    /// The parse/validate half of `load`, so tests do not need a file.
    pub fn parse(content: &str, path: &str) -> Self {
        let parsed: Value = match serde_json::from_str(&strip_json_comments(content)) {
            Ok(parsed) => parsed,
            Err(error) => {
                return ModelConfig::failed(format!(
                    "Failed to parse models.json: {error}\n\nFile: {path}"
                ));
            }
        };

        let errors = validate_models_config(&parsed);
        if !errors.is_empty() {
            return ModelConfig::failed(format!(
                "Invalid models.json schema:\n{}\n\nFile: {path}",
                errors.join("\n")
            ));
        }

        let raw = parsed
            .get("providers")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut providers = BTreeMap::new();
        let mut order = Vec::new();
        for (provider_id, provider) in raw {
            let provider: ModelsJsonProvider = match serde_json::from_value(provider) {
                Ok(provider) => provider,
                Err(error) => {
                    return ModelConfig::failed(format!(
                        "Invalid models.json schema:\n  - providers.{provider_id}: {error}\n\nFile: {path}"
                    ));
                }
            };
            order.push(provider_id.clone());
            providers.insert(provider_id, provider);
        }
        ModelConfig {
            providers,
            order,
            error: None,
        }
    }

    pub fn get_provider(&self, provider_id: &str) -> Option<&ModelsJsonProvider> {
        self.providers.get(provider_id)
    }

    pub fn get_provider_ids(&self) -> &[String] {
        &self.order
    }

    pub fn get_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}
