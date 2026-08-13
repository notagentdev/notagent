//! The built-in model catalog.
//!
//! 1:1 port of `packages/ai/src/model-catalog.ts` (27 LOC), `models.generated.ts`
//! (124 LOC, a pure aggregator) and the catalog-facing parts of
//! `packages/ai/src/providers/all.ts` (155 LOC).
//!
//! The data itself is the byte-identical snapshot of
//! `packages/ai/src/providers/data/` (39 files, 1 224 models). Per the workstream plan
//! the generator (`scripts/generate-models.ts`) is not ported: the snapshot is the
//! source, and updates keep happening in the TS repo. TypeScript imports the files as
//! JSON modules; here they are embedded with `include_str!` and parsed once on first use.
//!
//! `.manifest.json` is stored as `manifest.json` (leading dots are awkward for build
//! tooling); its content is unchanged and `tests/model_catalog.rs` verifies every
//! SHA-256 against it.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::types::Model;

/// The embedded per-provider catalog files, keyed by provider id.
pub const MODEL_DATA: [(&str, &str); 39] = [
    (
        "amazon-bedrock",
        include_str!("../data/amazon-bedrock.json"),
    ),
    ("ant-ling", include_str!("../data/ant-ling.json")),
    ("anthropic", include_str!("../data/anthropic.json")),
    (
        "azure-openai-responses",
        include_str!("../data/azure-openai-responses.json"),
    ),
    ("baseten", include_str!("../data/baseten.json")),
    ("cerebras", include_str!("../data/cerebras.json")),
    (
        "cloudflare-ai-gateway",
        include_str!("../data/cloudflare-ai-gateway.json"),
    ),
    (
        "cloudflare-workers-ai",
        include_str!("../data/cloudflare-workers-ai.json"),
    ),
    ("deepseek", include_str!("../data/deepseek.json")),
    ("fireworks", include_str!("../data/fireworks.json")),
    (
        "github-copilot",
        include_str!("../data/github-copilot.json"),
    ),
    ("google-vertex", include_str!("../data/google-vertex.json")),
    ("google", include_str!("../data/google.json")),
    ("groq", include_str!("../data/groq.json")),
    ("huggingface", include_str!("../data/huggingface.json")),
    ("kimi-coding", include_str!("../data/kimi-coding.json")),
    ("minimax-cn", include_str!("../data/minimax-cn.json")),
    ("minimax", include_str!("../data/minimax.json")),
    ("mistral", include_str!("../data/mistral.json")),
    ("moonshotai-cn", include_str!("../data/moonshotai-cn.json")),
    ("moonshotai", include_str!("../data/moonshotai.json")),
    ("nvidia", include_str!("../data/nvidia.json")),
    ("openai-codex", include_str!("../data/openai-codex.json")),
    ("openai", include_str!("../data/openai.json")),
    ("opencode-go", include_str!("../data/opencode-go.json")),
    ("opencode", include_str!("../data/opencode.json")),
    ("openrouter", include_str!("../data/openrouter.json")),
    (
        "qwen-token-plan-cn",
        include_str!("../data/qwen-token-plan-cn.json"),
    ),
    (
        "qwen-token-plan-individual",
        include_str!("../data/qwen-token-plan-individual.json"),
    ),
    (
        "qwen-token-plan",
        include_str!("../data/qwen-token-plan.json"),
    ),
    ("together", include_str!("../data/together.json")),
    (
        "vercel-ai-gateway",
        include_str!("../data/vercel-ai-gateway.json"),
    ),
    ("xai", include_str!("../data/xai.json")),
    (
        "xiaomi-token-plan-ams",
        include_str!("../data/xiaomi-token-plan-ams.json"),
    ),
    (
        "xiaomi-token-plan-cn",
        include_str!("../data/xiaomi-token-plan-cn.json"),
    ),
    (
        "xiaomi-token-plan-sgp",
        include_str!("../data/xiaomi-token-plan-sgp.json"),
    ),
    ("xiaomi", include_str!("../data/xiaomi.json")),
    ("zai-coding-cn", include_str!("../data/zai-coding-cn.json")),
    ("zai", include_str!("../data/zai.json")),
];

/// The embedded `.manifest.json` (schemaVersion 3).
pub const MODEL_DATA_MANIFEST: &str = include_str!("../data/manifest.json");

/// `flattenModelCatalog(provider, groups)` — merges the api groups of one provider file
/// into a flat model list, preserving file order.
pub fn flatten_model_catalog(groups: &Value) -> Vec<Model> {
    let Some(groups) = groups.as_object() else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for (_api, entries) in groups {
        let Some(entries) = entries.as_object() else {
            continue;
        };
        for (_id, model) in entries {
            match serde_json::from_value::<Model>(model.clone()) {
                Ok(model) => models.push(model),
                // A malformed entry would be a corrupted snapshot; the manifest test
                // guards against that, so skipping keeps `getModels()` non-throwing.
                Err(_) => continue,
            }
        }
    }
    models
}

fn catalog() -> &'static BTreeMap<String, Vec<Model>> {
    static CATALOG: OnceLock<BTreeMap<String, Vec<Model>>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        MODEL_DATA
            .iter()
            .map(|(provider, raw)| {
                let groups: Value =
                    serde_json::from_str(raw).expect("embedded catalog is valid JSON");
                ((*provider).to_string(), flatten_model_catalog(&groups))
            })
            .collect()
    })
}

/// `getBuiltinProviders()` — providers present in the generated catalog.
pub fn get_builtin_providers() -> Vec<&'static str> {
    MODEL_DATA.iter().map(|(provider, _)| *provider).collect()
}

/// `getBuiltinModels(provider)`
pub fn get_builtin_models(provider: &str) -> Vec<Model> {
    catalog().get(provider).cloned().unwrap_or_default()
}

/// `getBuiltinModel(provider, modelId)`
pub fn get_builtin_model(provider: &str, model_id: &str) -> Option<Model> {
    catalog()
        .get(provider)?
        .iter()
        .find(|model| model.id == model_id)
        .cloned()
}

/// Every built-in model across all providers.
pub fn all_builtin_models() -> Vec<Model> {
    catalog().values().flatten().cloned().collect()
}

/// `getBuiltinModelDataGeneratedAt()` — `Date.parse(manifest.generatedAt)`.
pub fn get_builtin_model_data_generated_at() -> Option<i64> {
    let manifest: Value = serde_json::from_str(MODEL_DATA_MANIFEST).ok()?;
    let generated_at = manifest.get("generatedAt")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(generated_at)
        .ok()
        .map(|value| value.timestamp_millis())
}
