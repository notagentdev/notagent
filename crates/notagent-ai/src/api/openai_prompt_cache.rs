//! 1:1 port of `packages/ai/src/api/openai-prompt-cache.ts`.

/// `OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH`
pub const OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

/// `clampOpenAIPromptCacheKey(key)` — clamps to 64 code points, not bytes.
///
/// TS counts with `Array.from(key)`, which iterates code points, so an astral
/// character counts once here and twice in `key.length`.
pub fn clamp_openai_prompt_cache_key(key: Option<&str>) -> Option<String> {
    let key = key?;
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH {
        return Some(key.to_string());
    }
    Some(
        chars
            .into_iter()
            .take(OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH)
            .collect(),
    )
}
