//! Detection of context-overflow errors across providers.
//!
//! 1:1 port of `packages/ai/src/utils/overflow.ts` (180 LOC). All 25 overflow patterns
//! and the 3 non-overflow exclusions keep their original order and wording.

use std::sync::OnceLock;

use regex::RegexSet;

use crate::types::{AssistantMessage, StopReason};

/// The overflow patterns from `overflow.ts:52-78`, in source order.
pub const OVERFLOW_PATTERNS: [&str; 25] = [
    r"(?i)prompt is too long",                    // Anthropic token overflow
    r"(?i)request_too_large",                     // Anthropic request byte-size overflow (HTTP 413)
    r"(?i)input is too long for requested model", // Amazon Bedrock
    r"(?i)exceeds the context window",            // OpenAI (Completions & Responses API)
    r"(?i)exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))", // LiteLLM
    r"(?i)input token count.*exceeds the maximum", // Google (Gemini)
    r"(?i)maximum prompt length is \d+",           // xAI (Grok)
    r"(?i)reduce the length of the messages",      // Groq
    r"(?i)maximum context length is \d+ tokens",   // OpenRouter (most backends)
    r"(?i)exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?", // OpenRouter/Poolside
    r"(?i)input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)", // Together AI
    r"(?i)exceeds the limit of \d+",           // GitHub Copilot
    r"(?i)exceeds the available context size", // llama.cpp server
    r"(?i)greater than the context length",    // LM Studio
    r"(?i)context window exceeds limit",       // MiniMax
    r"(?i)exceeded model token limit",         // Kimi For Coding
    r"(?i)too large for model with \d+ maximum context length", // Mistral
    r"(?i)prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?", // DS4 server
    r"(?i)model_context_window_exceeded",                                                // z.ai
    r"(?i)prompt too long; exceeded (?:max )?context length",                            // Ollama
    r"(?i)range of input length should be", // DashScope / Qwen Token Plan
    r"(?i)context[_ ]length[_ ]exceeded",   // Generic fallback
    r"(?i)too many tokens",                 // Generic fallback
    r"(?i)token limit exceeded",            // Generic fallback
    r"(?i)^4(?:00|13)\s*(?:status code)?\s*\(no body\)", // Cerebras: 400/413 with no body
];

/// Patterns that mark an error as *not* an overflow (`overflow.ts:89-93`).
pub const NON_OVERFLOW_PATTERNS: [&str; 3] = [
    r"(?i)^(Throttling error|Service unavailable):", // AWS Bedrock non-overflow errors
    r"(?i)rate limit",                               // Generic rate limiting
    r"(?i)too many requests",                        // Generic HTTP 429 style
];

fn overflow_set() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| RegexSet::new(OVERFLOW_PATTERNS).expect("overflow patterns compile"))
}

fn non_overflow_set() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| RegexSet::new(NON_OVERFLOW_PATTERNS).expect("non-overflow patterns compile"))
}

/// `isContextOverflow(message, contextWindow?)`
pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<u64>) -> bool {
    // Case 1: error message patterns.
    if message.stop_reason == StopReason::Error
        && let Some(error_message) = &message.error_message
        && !error_message.is_empty()
    {
        let is_non_overflow = non_overflow_set().is_match(error_message);
        if !is_non_overflow && overflow_set().is_match(error_message) {
            return true;
        }
    }

    // Case 2: silent overflow (z.ai style).
    if let Some(context_window) = context_window.filter(|window| *window != 0)
        && message.stop_reason == StopReason::Stop
    {
        let input_tokens = message.usage.input + message.usage.cache_read;
        if input_tokens > context_window {
            return true;
        }
    }

    // Case 3: length-stop overflow (Xiaomi MiMo style).
    if let Some(context_window) = context_window.filter(|window| *window != 0)
        && message.stop_reason == StopReason::Length
        && message.usage.output == 0
    {
        let input_tokens = message.usage.input + message.usage.cache_read;
        if input_tokens as f64 >= context_window as f64 * 0.99 {
            return true;
        }
    }

    false
}

/// `isRecoverableLength(message, desiredMaxOutput)`
pub fn is_recoverable_length(message: &AssistantMessage, desired_max_output: u64) -> bool {
    message.stop_reason == StopReason::Length
        && desired_max_output > 0
        && message.usage.output < desired_max_output
}

/// `getOverflowPatterns()` — exposed for tests, as in TS.
pub fn get_overflow_patterns() -> Vec<&'static str> {
    OVERFLOW_PATTERNS.to_vec()
}
