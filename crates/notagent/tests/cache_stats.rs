use notagent::core::cache_stats::{
    ModelPriceSource, collect_cache_misses, compute_cache_waste, detect_cache_miss,
};
use notagent::core::session_manager::{CompactionEntry, SessionEntry, SessionMessageEntry};
use notagent_agent::types::AgentMessage;
use notagent_ai::types::{AssistantMessage, StopReason, Usage, UsageCost};

/// $/million tokens; used as cache-read price fallback on full-miss turns.
struct Models;

impl ModelPriceSource for Models {
    fn get_cache_read_cost(&self, _provider: &str, _model_id: &str) -> Option<f64> {
        Some(0.3)
    }
}

#[derive(Default)]
struct AssistantOptions {
    input: u64,
    cache_read: u64,
    cache_write: u64,
    cost_cache_read: f64,
    cost_cache_write: f64,
    model: Option<&'static str>,
    timestamp: i64,
}

fn assistant(options: AssistantOptions) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: "anthropic-messages".to_owned(),
        provider: "test".to_owned(),
        model: options.model.unwrap_or("test-model").to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            input: options.input,
            output: 10,
            cache_read: options.cache_read,
            cache_write: options.cache_write,
            cache_write1h: None,
            reasoning: None,
            total_tokens: Some(0),
            cost: UsageCost {
                input: 0.0,
                output: 0.0,
                cache_read: options.cost_cache_read,
                cache_write: options.cost_cache_write,
                total: 0.0,
            },
        },
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: options.timestamp,
    }
}

fn entry(id: &str, message: &AssistantMessage) -> SessionEntry {
    SessionEntry::Message(SessionMessageEntry {
        id: id.to_owned(),
        parent_id: None,
        timestamp: String::new(),
        message: serde_json::to_value(AgentMessage::Assistant(message.clone())).expect("message"),
        extra: serde_json::Map::new(),
    })
}

/// Turn 1: fresh 100k cache write at $3.75/M.
fn turn1() -> AssistantMessage {
    assistant(AssistantOptions {
        cache_write: 100_000,
        cost_cache_write: 0.375,
        timestamp: 0,
        ..AssistantOptions::default()
    })
}

/// Turn 2: healthy, everything read back at $0.30/M.
fn turn2() -> AssistantMessage {
    assistant(AssistantOptions {
        cache_read: 100_000,
        cache_write: 5_000,
        cost_cache_read: 0.03,
        cost_cache_write: 0.019,
        timestamp: 60_000,
        ..AssistantOptions::default()
    })
}

fn close_to(actual: f64, expected: f64, digits: i32) {
    let tolerance = 10_f64.powi(-digits) / 2.0;
    assert!(
        (actual - expected).abs() < tolerance,
        "{actual} is not close to {expected}"
    );
}

// ---------------------------------------------------------------------------
// computeCacheWaste
// ---------------------------------------------------------------------------

#[test]
fn accumulates_missed_tokens_and_cost_across_turns() {
    // Turn 3: full miss, previous 105k prompt re-billed at $3.75/M write
    let turn3 = assistant(AssistantOptions {
        cache_write: 110_000,
        cost_cache_write: 0.4125,
        timestamp: 120_000,
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(
        &[
            entry("a", &turn1()),
            entry("b", &turn2()),
            entry("c", &turn3),
        ],
        &Models,
    );
    assert_eq!(totals.missed_tokens, 105_000);
    // 105k at ($3.75 - $0.30)/M
    close_to(totals.missed_cost, 0.36225, 5);
}

#[test]
fn counts_nothing_for_healthy_sessions() {
    let totals = compute_cache_waste(&[entry("a", &turn1()), entry("b", &turn2())], &Models);
    assert_eq!(totals.missed_tokens, 0);
    assert_eq!(totals.missed_cost, 0.0);
}

#[test]
fn skips_the_turn_after_a_compaction_reset() {
    let reset = SessionEntry::Compaction(CompactionEntry {
        id: "c".to_owned(),
        ..CompactionEntry::default()
    });
    let after_reset = assistant(AssistantOptions {
        cache_write: 20_000,
        cost_cache_write: 0.075,
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(
        &[entry("a", &turn1()), reset, entry("b", &after_reset)],
        &Models,
    );
    assert_eq!(totals.missed_tokens, 0);
}

#[test]
fn counts_misses_caused_by_model_switches() {
    let other_model = assistant(AssistantOptions {
        cache_write: 100_000,
        cost_cache_write: 0.375,
        model: Some("other-model"),
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(&[entry("a", &turn1()), entry("b", &other_model)], &Models);
    assert_eq!(totals.missed_tokens, 100_000);
    assert_eq!(totals.miss_count, 1);
}

#[test]
fn skips_providers_that_report_no_cache_activity() {
    let first = assistant(AssistantOptions {
        input: 100_000,
        ..AssistantOptions::default()
    });
    let second = assistant(AssistantOptions {
        input: 110_000,
        ..AssistantOptions::default()
    });
    let totals = compute_cache_waste(&[entry("a", &first), entry("b", &second)], &Models);
    assert_eq!(totals.missed_tokens, 0);
}

// ---------------------------------------------------------------------------
// collectCacheMisses
// ---------------------------------------------------------------------------

#[test]
fn maps_counted_misses_to_the_entries_that_paid_for_them() {
    let miss_turn = assistant(AssistantOptions {
        cache_write: 110_000,
        cost_cache_write: 0.4125,
        timestamp: 120_000,
        ..AssistantOptions::default()
    });
    let misses = collect_cache_misses(
        &[
            entry("a", &turn1()),
            entry("b", &turn2()),
            entry("miss", &miss_turn),
        ],
        &Models,
    );
    assert_eq!(misses.len(), 1);
    assert_eq!(
        misses.get("miss").map(|miss| miss.missed_tokens),
        Some(105_000)
    );
}

// ---------------------------------------------------------------------------
// detectCacheMiss
// ---------------------------------------------------------------------------

#[test]
fn detects_a_miss_on_a_just_completed_message_with_idle_time() {
    let miss_message = assistant(AssistantOptions {
        cache_write: 110_000,
        cost_cache_write: 0.4125,
        timestamp: 600_000,
        ..AssistantOptions::default()
    });
    let miss = detect_cache_miss(
        &[entry("a", &turn1()), entry("b", &turn2())],
        &miss_message,
        &Models,
    )
    .expect("miss");
    assert_eq!(miss.missed_tokens, 105_000);
    close_to(miss.missed_cost, 0.36225, 5);
    // 600s - 60s since the previous request
    assert_eq!(miss.idle_ms, 540_000);
    assert!(!miss.model_changed);
}

#[test]
fn flags_model_switches_on_detected_misses() {
    let other_model = assistant(AssistantOptions {
        cache_write: 110_000,
        cost_cache_write: 0.4125,
        model: Some("other-model"),
        timestamp: 120_000,
        ..AssistantOptions::default()
    });
    let miss = detect_cache_miss(
        &[entry("a", &turn1()), entry("b", &turn2())],
        &other_model,
        &Models,
    )
    .expect("miss");
    assert_eq!(miss.missed_tokens, 105_000);
    assert!(miss.model_changed);
}

#[test]
fn returns_none_for_healthy_turns() {
    let healthy = assistant(AssistantOptions {
        cache_read: 105_000,
        cache_write: 2_000,
        cost_cache_read: 0.0315,
        cost_cache_write: 0.0075,
        timestamp: 120_000,
        ..AssistantOptions::default()
    });
    assert_eq!(
        detect_cache_miss(
            &[entry("a", &turn1()), entry("b", &turn2())],
            &healthy,
            &Models
        ),
        None
    );
}

#[test]
fn returns_none_for_the_first_turn_of_a_session() {
    assert_eq!(detect_cache_miss(&[], &turn1(), &Models), None);
}
