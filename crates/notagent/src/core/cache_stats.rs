use std::collections::HashMap;

use notagent_ai::types::AssistantMessage;
use serde_json::Value;

use crate::core::session_manager::SessionEntry;

/// Prompt-cache TTL: idle gaps longer than this are worth mentioning as the
/// likely cause of a miss. Anthropic's default cache TTL is 5 minutes.
pub const CACHE_TTL_MS: i64 = 5 * 60 * 1000;

/// Per-turn misses at or below this are cache breakpoint granularity noise.
const NOISE_FLOOR_TOKENS: u64 = 1024;

/// A counted cache miss on a single assistant message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheMiss {
    /// Prompt tokens that were in the previous turn's prompt but not read from cache.
    pub missed_tokens: u64,
    /// Extra dollars paid vs. a full cache hit; 0 when pricing is unknown.
    pub missed_cost: f64,
    /// Milliseconds since the previous request (which last refreshed the cache).
    pub idle_ms: i64,
    /// True when the model changed relative to the previous request.
    pub model_changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CacheWasteTotals {
    pub missed_tokens: u64,
    pub missed_cost: f64,
    /// Number of counted misses (turns above the noise floor).
    pub miss_count: u64,
}

/// Minimal pricing lookup, satisfied by `ModelRuntime`. Cost is $/million tokens.
pub trait ModelPriceSource {
    /// `getModel(provider, modelId)?.cost.cacheRead`
    fn get_cache_read_cost(&self, provider: &str, model_id: &str) -> Option<f64>;
}

impl ModelPriceSource for crate::core::model_runtime::ModelRuntime {
    fn get_cache_read_cost(&self, provider: &str, model_id: &str) -> Option<f64> {
        self.get_model(provider, model_id)
            .map(|model| model.cost.cache_read)
    }
}

/// The last request seen by the scan; everything in its prompt should be cached.
#[derive(Debug, Clone)]
struct PreviousRequest {
    prompt_tokens: u64,
    model_key: String,
    timestamp: i64,
    /// Sticky: some earlier request in this scan segment reported cache activity.
    /// Distinguishes a total miss on a cache-read-only provider (OpenAI-style,
    /// writes unreported) from a provider that never reports caching at all.
    reported_cache: bool,
}

/// The fields of an assistant message the scan reads. A session entry keeps its
/// message as raw JSON (see `session_manager`), so both shapes are reduced to
/// this before the arithmetic.
struct ScannedMessage {
    provider: String,
    model: String,
    timestamp: i64,
    input: u64,
    output_cost_input: f64,
    cache_read: u64,
    cache_write: u64,
    cost_cache_read: f64,
    cost_cache_write: f64,
}

impl ScannedMessage {
    fn from_assistant(message: &AssistantMessage) -> Self {
        let usage = &message.usage;
        ScannedMessage {
            provider: message.provider.clone(),
            model: message.model.clone(),
            timestamp: message.timestamp,
            input: usage.input,
            output_cost_input: usage.cost.input,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cost_cache_read: usage.cost.cache_read,
            cost_cache_write: usage.cost.cache_write,
        }
    }

    /// The raw message of a session entry. Missing fields read as zero, which is
    /// what JavaScript's arithmetic does with `undefined` fields on a message
    /// written by an older build.
    fn from_value(message: &Value) -> Self {
        let string = |key: &str| {
            message
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let usage = message.get("usage");
        let number = |object: Option<&Value>, key: &str| {
            object
                .and_then(|object| object.get(key))
                .and_then(Value::as_f64)
                .unwrap_or_default()
        };
        let cost = usage.and_then(|usage| usage.get("cost"));
        ScannedMessage {
            provider: string("provider"),
            model: string("model"),
            timestamp: message
                .get("timestamp")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            input: number(usage, "input") as u64,
            output_cost_input: number(cost, "input"),
            cache_read: number(usage, "cacheRead") as u64,
            cache_write: number(usage, "cacheWrite") as u64,
            cost_cache_read: number(cost, "cacheRead"),
            cost_cache_write: number(cost, "cacheWrite"),
        }
    }

    fn prompt_tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    fn model_key(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// Compute the cache miss for one assistant message relative to the previous
/// request. Returns `None` when nothing is counted: first turn, after a reset,
/// no cache activity ever reported (provider without cache support), or miss
/// below the noise floor.
fn detect_miss(
    previous: Option<&PreviousRequest>,
    message: &ScannedMessage,
    models: &dyn ModelPriceSource,
) -> Option<CacheMiss> {
    let prompt_tokens = message.prompt_tokens();
    // A zero-cache turn only counts when cache activity was reported before:
    // on cache-read-only providers that is a total miss, while on providers
    // that never report caching it means nothing.
    let previous = previous?;
    if prompt_tokens == 0
        || (message.cache_read + message.cache_write == 0 && !previous.reported_cache)
    {
        return None;
    }

    let missed_tokens =
        previous.prompt_tokens.min(prompt_tokens) as i64 - message.cache_read as i64;
    if missed_tokens <= NOISE_FLOOR_TOKENS as i64 {
        return None;
    }
    let missed_tokens = missed_tokens as u64;

    // Extra cost = missed tokens billed at the actual paid rate (input/cacheWrite,
    // incl. write premium) instead of the cache-read rate. Missed tokens can only
    // land in the input or cacheWrite buckets, so the paid rate comes straight
    // from this message's own cost breakdown.
    let paid_tokens = message.input + message.cache_write;
    let paid_per_token = if paid_tokens > 0 {
        (message.output_cost_input + message.cost_cache_write) / paid_tokens as f64
    } else {
        0.0
    };
    let read_per_token = if message.cache_read > 0 {
        message.cost_cache_read / message.cache_read as f64
    } else {
        models
            .get_cache_read_cost(&message.provider, &message.model)
            .unwrap_or(0.0)
            / 1_000_000.0
    };

    Some(CacheMiss {
        missed_tokens,
        missed_cost: missed_tokens as f64 * (paid_per_token - read_per_token).max(0.0),
        idle_ms: (message.timestamp - previous.timestamp).max(0),
        model_changed: message.model_key() != previous.model_key,
    })
}

fn as_previous_request(message: &ScannedMessage, reported_cache: bool) -> Option<PreviousRequest> {
    let prompt_tokens = message.prompt_tokens();
    if prompt_tokens == 0 {
        return None;
    }
    Some(PreviousRequest {
        prompt_tokens,
        model_key: message.model_key(),
        timestamp: message.timestamp,
        reported_cache: reported_cache || message.cache_read + message.cache_write > 0,
    })
}

struct Scan {
    previous: Option<PreviousRequest>,
    totals: CacheWasteTotals,
    /// reference identity, which Rust has no equivalent for. The key is the id
    /// of the session entry that carries the message instead — stable across the
    /// rebuild the map exists for.
    misses: HashMap<String, CacheMiss>,
}

fn scan(entries: &[SessionEntry], models: &dyn ModelPriceSource) -> Scan {
    let mut previous: Option<PreviousRequest> = None;
    let mut totals = CacheWasteTotals::default();
    let mut misses: HashMap<String, CacheMiss> = HashMap::new();

    for entry in entries {
        match entry {
            // The context legitimately changed; the next turn's prompt is new content,
            // not re-billed content. Model switches are NOT exempt: they re-bill the
            // full prompt and should be counted.
            SessionEntry::Compaction(_) | SessionEntry::BranchSummary(_) => {
                previous = None;
            }
            SessionEntry::Message(message_entry)
                if message_entry.message.get("role").and_then(Value::as_str)
                    == Some("assistant") =>
            {
                let message = ScannedMessage::from_value(&message_entry.message);
                if let Some(miss) = detect_miss(previous.as_ref(), &message, models) {
                    totals.missed_tokens += miss.missed_tokens;
                    totals.missed_cost += miss.missed_cost;
                    totals.miss_count += 1;
                    misses.insert(message_entry.id.clone(), miss);
                }
                previous = as_previous_request(
                    &message,
                    previous
                        .as_ref()
                        .is_some_and(|previous| previous.reported_cache),
                )
                .or(previous);
            }
            _ => {}
        }
    }
    Scan {
        previous,
        totals,
        misses,
    }
}

/// Cumulative cache waste across a session: prompt tokens that should have been
/// cache reads (they were in the previous turn's prompt) but were re-billed.
pub fn compute_cache_waste(
    entries: &[SessionEntry],
    models: &dyn ModelPriceSource,
) -> CacheWasteTotals {
    scan(entries, models).totals
}

/// All counted cache misses across a session, keyed by the id of the session
/// entry that paid for them. Used to re-derive transcript notices when
/// rebuilding the chat from entries (resume, post-compaction rebuild).
pub fn collect_cache_misses(
    entries: &[SessionEntry],
    models: &dyn ModelPriceSource,
) -> HashMap<String, CacheMiss> {
    scan(entries, models).misses
}

/// Detect a cache miss on a just-completed assistant message.
/// `entries` must not yet contain `message` (message_end fires before persistence).
pub fn detect_cache_miss(
    entries: &[SessionEntry],
    message: &AssistantMessage,
    models: &dyn ModelPriceSource,
) -> Option<CacheMiss> {
    detect_miss(
        scan(entries, models).previous.as_ref(),
        &ScannedMessage::from_assistant(message),
        models,
    )
}
