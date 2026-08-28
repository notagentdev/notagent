use notagent_ai::types::Usage;
use serde_json::Value;

use crate::core::session_manager::SessionEntry;

/// `UsageTotals`
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

/// `createUsageTotals()`
pub fn create_usage_totals() -> UsageTotals {
    UsageTotals::default()
}

/// `addUsageToTotals(totals, usage)`
pub fn add_usage_to_totals(totals: &mut UsageTotals, usage: &Usage) {
    totals.input += usage.input;
    totals.output += usage.output;
    totals.cache_read += usage.cache_read;
    totals.cache_write += usage.cache_write;
    totals.cost += usage.cost.total;
}

/// `UsageCostBreakdownEntry`
#[derive(Debug, Clone, PartialEq)]
pub struct UsageCostBreakdownEntry {
    pub key: String,
    pub cost: f64,
    pub tokens: u64,
}

/// `getUsageCostBreakdown(entries)` — group attributable assistant usage by
/// model and all other usage into a separate bucket.
/// Deviation (class 1): the insertion order of a JavaScript `Map` is kept by an
/// explicit key list, because Rust's `HashMap` has none and the sort below is
/// not total — entries with equal cost keep the order in which they first
/// appeared, exactly as `Array.prototype.sort` does for a `Map` iteration.
pub fn get_usage_cost_breakdown(entries: &[SessionEntry]) -> Vec<UsageCostBreakdownEntry> {
    let mut order: Vec<String> = Vec::new();
    let mut totals_by_key: std::collections::HashMap<String, UsageTotals> =
        std::collections::HashMap::new();

    for entry in entries {
        let (key, usage) = match entry {
            SessionEntry::Message(entry) => {
                let message = &entry.message;
                match message.get("role").and_then(Value::as_str) {
                    Some("assistant") => {
                        let provider = message
                            .get("provider")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let model = message
                            .get("responseModel")
                            .and_then(Value::as_str)
                            .or_else(|| message.get("model").and_then(Value::as_str))
                            .unwrap_or_default();
                        (
                            format!("{provider}/{model}"),
                            parse_usage(message.get("usage")),
                        )
                    }
                    Some("toolResult") => (
                        "Tools/summaries".to_owned(),
                        parse_usage(message.get("usage")),
                    ),
                    _ => continue,
                }
            }
            SessionEntry::BranchSummary(entry) => {
                ("Tools/summaries".to_owned(), entry.usage.as_ref().copied())
            }
            SessionEntry::Compaction(entry) => {
                ("Tools/summaries".to_owned(), entry.usage.as_ref().copied())
            }
            _ => continue,
        };
        let Some(usage) = usage else { continue };

        let totals = totals_by_key.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            create_usage_totals()
        });
        add_usage_to_totals(totals, &usage);
    }

    let mut breakdown: Vec<UsageCostBreakdownEntry> = order
        .into_iter()
        .map(|key| {
            let totals = totals_by_key[&key];
            UsageCostBreakdownEntry {
                key,
                cost: totals.cost,
                tokens: totals.input + totals.output + totals.cache_read + totals.cache_write,
            }
        })
        .filter(|entry| entry.cost > 0.0 || entry.tokens > 0)
        .collect();
    breakdown.sort_by(|left, right| {
        right
            .cost
            .partial_cmp(&left.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    breakdown
}

/// An absent or malformed `usage` object drops the entry, the same way the
fn parse_usage(value: Option<&Value>) -> Option<Usage> {
    serde_json::from_value::<Usage>(value?.clone()).ok()
}
