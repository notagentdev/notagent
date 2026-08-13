//! Shared option building for the `streamSimple` entry points.
//!
//! 1:1 port of `packages/ai/src/api/simple-options.ts` (86 LOC).

use crate::types::{
    Context, Model, SimpleStreamOptions, StreamOptions, ThinkingBudgets, ThinkingLevel,
};
use crate::utils::estimate::estimate_context_tokens;

const CONTEXT_SAFETY_TOKENS: u64 = 4096;
const MIN_MAX_TOKENS: u64 = 1;

/// Tokens always left for the answer when a thinking budget shares the response ceiling.
pub const MIN_ANSWER_TOKENS: u64 = 1024;

/// `clampMaxTokensToContext(model, context, maxTokens)`
pub fn clamp_max_tokens_to_context(model: &Model, context: &Context, max_tokens: u64) -> u64 {
    if model.context_window == 0 {
        return max_tokens.max(MIN_MAX_TOKENS);
    }
    // TS computes in signed arithmetic, so an overlong context yields a negative value
    // that `Math.max(MIN_MAX_TOKENS, ...)` lifts back to 1.
    let used = estimate_context_tokens(context).tokens as i128 + CONTEXT_SAFETY_TOKENS as i128;
    let available = model.context_window as i128 - used;
    let available = if available < MIN_MAX_TOKENS as i128 {
        MIN_MAX_TOKENS
    } else {
        available as u64
    };
    max_tokens.min(available)
}

/// `buildBaseOptions(model, context, options?, apiKey?)`
pub fn build_base_options(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
    api_key: Option<String>,
) -> StreamOptions {
    let sampling_params = match (
        &model.sampling_params,
        options.and_then(|options| options.base.sampling_params.as_ref()),
    ) {
        (None, None) => None,
        (model_params, request_params) => {
            let mut merged = model_params.clone().unwrap_or_default();
            for (key, value) in request_params.cloned().unwrap_or_default() {
                merged.insert(key, value);
            }
            Some(merged)
        }
    };

    let requested_max_tokens = options
        .and_then(|options| options.base.max_tokens)
        .unwrap_or(model.max_tokens);
    let base = options
        .map(|options| options.base.clone())
        .unwrap_or_default();

    StreamOptions {
        temperature: base.temperature,
        sampling_params,
        max_tokens: Some(clamp_max_tokens_to_context(
            model,
            context,
            requested_max_tokens,
        )),
        transport: base.transport,
        cache_retention: base.cache_retention,
        session_id: base.session_id.clone(),
        websocket_connect_timeout_ms: base.websocket_connect_timeout_ms,
        metadata: base.metadata.clone(),
        base: crate::types::ProviderRequestOptions {
            // An explicitly passed key wins, as `apiKey || options?.apiKey` does in TS.
            api_key: api_key
                .filter(|key| !key.is_empty())
                .or(base.base.api_key.clone()),
            ..base.base
        },
    }
}

/// `clampReasoning(effort)` — `xhigh`/`max` collapse to `high` for budget models.
pub fn clamp_reasoning(effort: Option<ThinkingLevel>) -> Option<ThinkingLevel> {
    match effort {
        Some(ThinkingLevel::Xhigh | ThinkingLevel::Max) => Some(ThinkingLevel::High),
        other => other,
    }
}

/// Result of [`adjust_max_tokens_for_thinking`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdjustedThinking {
    pub max_tokens: u64,
    pub thinking_budget: u64,
}

/// `adjustMaxTokensForThinking(baseMaxTokens, modelMaxTokens, reasoningLevel, customBudgets?)`
///
/// `base_max_tokens` of `None` means the caller set no cap: the model cap is used and
/// thinking has to fit inside it.
pub fn adjust_max_tokens_for_thinking(
    base_max_tokens: Option<u64>,
    model_max_tokens: u64,
    reasoning_level: ThinkingLevel,
    custom_budgets: Option<&ThinkingBudgets>,
) -> AdjustedThinking {
    let defaults = ThinkingBudgets {
        minimal: Some(1024),
        low: Some(2048),
        medium: Some(8192),
        high: Some(16384),
    };
    let budgets = ThinkingBudgets {
        minimal: custom_budgets
            .and_then(|budgets| budgets.minimal)
            .or(defaults.minimal),
        low: custom_budgets
            .and_then(|budgets| budgets.low)
            .or(defaults.low),
        medium: custom_budgets
            .and_then(|budgets| budgets.medium)
            .or(defaults.medium),
        high: custom_budgets
            .and_then(|budgets| budgets.high)
            .or(defaults.high),
    };

    let level =
        clamp_reasoning(Some(reasoning_level)).expect("a concrete level maps to a concrete level");
    let mut thinking_budget = match level {
        ThinkingLevel::Minimal => budgets.minimal,
        ThinkingLevel::Low => budgets.low,
        ThinkingLevel::Medium => budgets.medium,
        ThinkingLevel::High => budgets.high,
        // `clamp_reasoning` already mapped these to `high`.
        ThinkingLevel::Xhigh | ThinkingLevel::Max => budgets.high,
    }
    .unwrap_or(0);

    let max_tokens = match base_max_tokens {
        None => model_max_tokens,
        Some(base) => (base + thinking_budget).min(model_max_tokens),
    };

    if max_tokens <= thinking_budget {
        thinking_budget = max_tokens.saturating_sub(MIN_ANSWER_TOKENS);
    }

    AdjustedThinking {
        max_tokens,
        thinking_budget,
    }
}
