//! Port of `packages/coding-agent/test/footer-width.test.ts` (252 LOC, 9 cases).
//!
//! The TS suite hands the component duck-typed stubs for the session and the
//! data provider; here they implement the two traits the component reads
//! through (class-1 deviation documented in `components/footer.rs`).

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notagent::core::agent_session::ContextUsage;
use notagent::core::modes::Mode;
use notagent::core::session_manager::SessionEntry;
use notagent::modes::interactive::components::footer::{
    FooterComponent, FooterData, FooterSession, format_cwd_for_footer,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_agent::types::ThinkingLevel;
use notagent_ai::types::{Modality, Model, ModelCost};
use notagent_tui::tui::Component;
use notagent_tui::utils::visible_width;
use serde_json::{Value, json};

/// The theme is process-global.
fn guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

type Usage = (u64, u64, u64, u64, f64);

#[derive(Default)]
struct SessionOptions {
    session_name: String,
    model_id: Option<String>,
    provider: Option<String>,
    reasoning: bool,
    thinking_level: Option<ThinkingLevel>,
    usage: Option<Usage>,
    branch_usage: Option<Usage>,
    compaction_usage: Option<Usage>,
    tool_usage: Option<Usage>,
    using_subscription: bool,
}

fn usage_json((input, output, cache_read, cache_write, cost): Usage) -> Value {
    json!({
        "input": input,
        "output": output,
        "cacheRead": cache_read,
        "cacheWrite": cache_write,
        // The TS fake only sets `cost.total`; `UsageCost` is a struct here, so
        // the fixture fills the whole breakdown (class-1 harness deviation).
        "cost": {
            "input": 0.0,
            "output": 0.0,
            "cacheRead": 0.0,
            "cacheWrite": 0.0,
            "total": cost,
        },
    })
}

struct StubSession {
    entries: Vec<SessionEntry>,
    model: Model,
    thinking_level: ThinkingLevel,
    session_name: String,
    using_subscription: bool,
}

impl FooterSession for StubSession {
    fn model(&self) -> Option<Model> {
        Some(self.model.clone())
    }

    fn thinking_level(&self) -> ThinkingLevel {
        self.thinking_level
    }

    fn entries(&self) -> Vec<SessionEntry> {
        self.entries.clone()
    }

    fn cwd(&self) -> String {
        "/tmp/project".to_string()
    }

    fn session_name(&self) -> Option<String> {
        (!self.session_name.is_empty()).then(|| self.session_name.clone())
    }

    fn context_usage(&self) -> Option<ContextUsage> {
        Some(ContextUsage {
            tokens: Some(24_600),
            context_window: 200_000,
            percent: Some(12.3),
        })
    }

    fn active_mode(&self) -> Option<Mode> {
        None
    }

    /// Mirrors `ModelRuntime::is_using_subscription`: the plans that
    /// authenticate with an API key are known by name, everything else is
    /// whatever the fixture declares.
    fn goal(&self) -> Option<notagent::core::goal::ThreadGoal> {
        None
    }

    fn mcp_summary(&self) -> notagent::modes::interactive::components::footer::McpSummary {
        Default::default()
    }

    fn is_using_subscription(&self, provider: &str) -> bool {
        self.using_subscription
            || notagent::core::model_runtime::is_subscription_api_key_provider(provider)
    }
}

fn create_session(options: SessionOptions) -> Arc<dyn FooterSession> {
    let mut entries: Vec<Value> = Vec::new();
    if let Some(usage) = options.usage {
        entries.push(json!({
            "type": "message",
            "message": { "role": "assistant", "usage": usage_json(usage) },
        }));
    }
    if let Some(usage) = options.branch_usage {
        entries.push(json!({ "type": "branch_summary", "usage": usage_json(usage) }));
    }
    if let Some(usage) = options.compaction_usage {
        entries.push(json!({ "type": "compaction", "usage": usage_json(usage) }));
    }
    if let Some(usage) = options.tool_usage {
        entries.push(json!({
            "type": "message",
            "message": { "role": "toolResult", "usage": usage_json(usage) },
        }));
    }

    let model = Model {
        id: options.model_id.unwrap_or_else(|| "test-model".to_string()),
        name: "mock".to_string(),
        api: "anthropic-messages".to_string(),
        provider: options.provider.unwrap_or_else(|| "test".to_string()),
        base_url: "https://example.invalid".to_string(),
        reasoning: options.reasoning,
        thinking_level_map: None,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_tokens: 8_192,
        sampling_params: None,
        headers: None,
        compat: None,
    };

    Arc::new(StubSession {
        entries: entries
            .into_iter()
            .map(|entry| serde_json::from_value(entry).expect("entry parses"))
            .collect(),
        model,
        thinking_level: options.thinking_level.unwrap_or(ThinkingLevel::Off),
        session_name: options.session_name,
        using_subscription: options.using_subscription,
    })
}

struct StubFooterData {
    provider_count: u64,
}

impl FooterData for StubFooterData {
    fn get_git_branch(&self) -> Option<String> {
        Some("main".to_string())
    }

    fn get_available_provider_count(&self) -> u64 {
        self.provider_count
    }
}

fn create_footer_data(provider_count: u64) -> Arc<dyn FooterData> {
    Arc::new(StubFooterData { provider_count })
}

fn themed_footer(session: Arc<dyn FooterSession>, provider_count: u64) -> FooterComponent {
    init_theme(None, false);
    FooterComponent::new(session, create_footer_data(provider_count))
}

#[test]
fn does_not_abbreviate_sibling_paths_that_share_the_home_prefix() {
    assert_eq!(
        format_cwd_for_footer("/home/user2", Some("/home/user")),
        "/home/user2"
    );
}

#[test]
fn abbreviates_the_home_directory_and_descendants() {
    assert_eq!(format_cwd_for_footer("/home/user", Some("/home/user")), "~");
    assert_eq!(
        format_cwd_for_footer("/home/user/project", Some("/home/user")),
        "~/project"
    );
}

#[test]
fn keeps_all_lines_within_width_for_wide_session_names() {
    let _guard = guard();
    let width = 93;
    let session = create_session(SessionOptions {
        session_name: "한글".repeat(30),
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 1);

    for line in footer.render(width) {
        assert!(
            visible_width(&line) <= width,
            "line too wide ({}): {line:?}",
            visible_width(&line)
        );
    }
}

#[test]
fn keeps_stats_line_within_width_for_wide_model_and_provider_names() {
    let _guard = guard();
    let width = 60;
    let session = create_session(SessionOptions {
        session_name: String::new(),
        model_id: Some("模".repeat(30)),
        provider: Some("공급자".to_string()),
        reasoning: true,
        thinking_level: Some(ThinkingLevel::High),
        usage: Some((12_345, 6_789, 0, 0, 1.234)),
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 2);

    for line in footer.render(width) {
        assert!(
            visible_width(&line) <= width,
            "line too wide ({}): {line:?}",
            visible_width(&line)
        );
    }
}

#[test]
fn includes_summary_and_tool_result_usage_in_the_total_cost() {
    let _guard = guard();
    let session = create_session(SessionOptions {
        session_name: String::new(),
        usage: Some((100, 10, 0, 0, 0.5)),
        branch_usage: Some((20, 5, 0, 0, 0.25)),
        compaction_usage: Some((5, 2, 0, 0, 0.125)),
        tool_usage: Some((15, 3, 0, 0, 0.375)),
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 1);

    let stats_line = strip_ansi(&footer.render(120)[1]);
    assert!(stats_line.contains("$1.250"), "stats: {stats_line}");
}

#[test]
fn shows_the_latest_cache_hit_rate_when_cache_usage_is_present() {
    let _guard = guard();
    let session = create_session(SessionOptions {
        session_name: String::new(),
        usage: Some((100, 10, 50, 50, 0.001)),
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 1);

    let stats_line = strip_ansi(&footer.render(120)[1]);
    assert!(stats_line.contains("CH25.0%"), "stats: {stats_line}");
}

/// A subscription is paid by the month, so the per-token amount is not money
/// anyone owes; the footer says the tokens are covered and names no figure
/// (user decision 2026-08-17, v0.1.15).
#[test]
fn a_subscription_reports_no_price() {
    let _guard = guard();
    let session = create_session(SessionOptions {
        session_name: String::new(),
        provider: Some("kimi-coding".to_string()),
        usage: Some((100, 10, 0, 0, 1.234)),
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 1);

    let stats = strip_ansi(&footer.render(120)[1]);
    assert!(stats.contains("sub"), "stats: {stats}");
    assert!(!stats.contains('$'), "no amount at all: {stats}");
    assert!(!stats.contains("1.234"), "stats: {stats}");
}

#[test]
fn an_explicitly_identified_subscription_reports_no_price() {
    let _guard = guard();
    let session = create_session(SessionOptions {
        session_name: String::new(),
        provider: Some("anthropic".to_string()),
        using_subscription: true,
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 1);

    let stats = strip_ansi(&footer.render(120)[1]);
    assert!(stats.contains("sub"), "stats: {stats}");
    assert!(!stats.contains('$'), "no amount at all: {stats}");
}

#[test]
fn does_not_mark_generic_oauth_sign_in_as_a_subscription() {
    let _guard = guard();
    let session = create_session(SessionOptions {
        session_name: String::new(),
        provider: Some("openrouter".to_string()),
        usage: Some((100, 10, 0, 0, 1.234)),
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 1);

    // Paid per token, so the amount is real money and stays on screen.
    let stats = strip_ansi(&footer.render(120)[1]);
    assert!(stats.contains("$1.234"), "stats: {stats}");
    assert!(!stats.contains("sub"), "stats: {stats}");
}

/// ClinePass is a subscription behind API-key authentication, like Kimi
/// Coding: the plan covers the tokens, so no price is reported.
#[test]
fn cline_pass_reports_no_price() {
    let _guard = guard();
    let session = create_session(SessionOptions {
        session_name: String::new(),
        provider: Some("cline-pass".to_string()),
        usage: Some((100, 10, 0, 0, 0.5)),
        ..SessionOptions::default()
    });
    let mut footer = themed_footer(session, 1);

    let stats = strip_ansi(&footer.render(120)[1]);
    assert!(stats.contains("sub"), "stats: {stats}");
    assert!(!stats.contains('$'), "no amount at all: {stats}");
}

/// The rule itself, without the footer: a plan bought outside the tool is a
/// subscription regardless of how it authenticates, so every caller of the
/// runtime — not just the footer — gets the same answer.
#[test]
fn the_api_key_backed_plans_are_known_to_the_runtime() {
    use notagent::core::model_runtime::is_subscription_api_key_provider;

    assert!(is_subscription_api_key_provider("cline-pass"));
    assert!(is_subscription_api_key_provider("kimi-coding"));
    // Pay-per-token providers are not, however they sign in.
    assert!(!is_subscription_api_key_provider("openrouter"));
    assert!(!is_subscription_api_key_provider("anthropic"));
    assert!(!is_subscription_api_key_provider("zai"));
}
