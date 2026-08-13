//! Port of `packages/coding-agent/src/core/experimental.ts`.

use notagent_ai::types::{ConstrainedSampling, ConstrainedSamplingConfig, StrictMode};

pub fn are_experimental_features_enabled() -> bool {
    std::env::var("NOTAGENT_EXPERIMENTAL").is_ok_and(|value| value == "1")
}

/// `{ type: "json_schema", strict: "prefer" }` while experimental features are on.
pub fn get_experimental_tool_sampling() -> Option<ConstrainedSampling> {
    are_experimental_features_enabled().then_some(ConstrainedSampling::Config(
        ConstrainedSamplingConfig::JsonSchema {
            strict: StrictMode::Prefer,
        },
    ))
}
