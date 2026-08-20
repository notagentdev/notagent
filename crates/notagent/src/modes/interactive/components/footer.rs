//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/footer.ts` (253 LOC).
//!
//! Shows the working directory, the git branch, the session name, the token and
//! cost totals of the whole session, the context usage and the model.
//!
//! Deviation (class 1): TS hands the component the `AgentSession` and the
//! `ReadonlyFooterDataProvider` themselves, and its test passes duck-typed
//! stubs instead. Rust needs a named contract for that, so the component reads
//! through [`FooterSession`] and [`FooterData`]; both are implemented for the
//! real types, and the ported suite implements them with stubs.
//!
//! Deviation (class 2): the extension status line is gone. Its only producer
//! was `ctx.ui.setStatus` of the extension API, and `FooterDataProvider`
//! therefore no longer carries a status map (see the note there and
//! `plans/facts/extension-boundary.md` §6).

use std::path::{Component as PathComponent, Path, PathBuf};
use std::sync::Arc;

use notagent_agent::types::ThinkingLevel;
use notagent_ai::types::Model;
use notagent_ai::types::Usage;
use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};
use serde_json::Value;

use crate::core::agent_session::{AgentSession, ContextUsage};
use crate::core::experimental::are_experimental_features_enabled;
use crate::core::footer_data_provider::FooterDataProvider;
use crate::core::goal::{ThreadGoal, ThreadGoalStatus};
use crate::core::mcp::manager::McpServerStatus;
use crate::core::modes::Mode;
use crate::core::modes::indicator::{format_mode_label, indicator_color_key};
use crate::core::modes::shells::ShellId;
use crate::core::session_manager::SessionEntry;
use crate::core::usage_totals::{add_usage_to_totals, create_usage_totals};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// Format token counts for compact footer display.
pub fn format_tokens(count: u64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    if count < 10_000 {
        return format!("{:.1}k", count as f64 / 1000.0);
    }
    if count < 1_000_000 {
        return format!("{}k", js_round(count as f64 / 1000.0));
    }
    if count < 10_000_000 {
        return format!("{:.1}M", count as f64 / 1_000_000.0);
    }
    format!("{}M", js_round(count as f64 / 1_000_000.0))
}

/// `Math.round`: halves go up.
fn js_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// Replace the home directory prefix with `~`.
pub fn format_cwd_for_footer(cwd: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return cwd.to_string();
    };

    let resolved_cwd = resolve_path(cwd);
    let resolved_home = resolve_path(home);
    let Some(relative_to_home) = relative_path(&resolved_home, &resolved_cwd) else {
        return cwd.to_string();
    };
    let is_inside_home = relative_to_home.is_empty()
        || (relative_to_home != ".."
            && !relative_to_home.starts_with("../")
            && !Path::new(&relative_to_home).is_absolute());

    if !is_inside_home {
        return cwd.to_string();
    }
    if relative_to_home.is_empty() {
        "~".to_string()
    } else {
        format!("~/{relative_to_home}")
    }
}

/// `path.resolve(p)` against the current working directory.
fn resolve_path(value: &str) -> PathBuf {
    let path = Path::new(value);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            PathComponent::CurDir => {}
            PathComponent::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path.relative(from, to)`; `None` when the two have no common root.
fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut parts: Vec<String> = vec!["..".to_string(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    Some(parts.join("/"))
}

/// What the footer reads from the session (`AgentSession` in TS).
/// `[goal ● active · 7/20 turns · 12.4K tokens]`, coloured by status.
///
/// A `Complete` goal is not badged: it is cleared from the session the moment
/// the user is told, and a badge for it would linger over work that is done.
fn render_goal_badge(goal: &ThreadGoal) -> String {
    let (label, color) = match goal.status {
        ThreadGoalStatus::Active if goal.strict => ("active, strict", ThemeColor::Accent),
        ThreadGoalStatus::Active => ("active", ThemeColor::Accent),
        ThreadGoalStatus::Paused => ("paused", ThemeColor::Dim),
        ThreadGoalStatus::Blocked => ("blocked", ThemeColor::Warning),
        ThreadGoalStatus::BudgetLimited => ("budget reached", ThemeColor::Warning),
        ThreadGoalStatus::Complete => ("complete", ThemeColor::Dim),
    };
    let turns = match goal.turn_budget {
        Some(budget) => format!("{}/{budget} turns", goal.turns_used),
        None => format!(
            "{} {}",
            goal.turns_used,
            if goal.turns_used == 1 {
                "turn"
            } else {
                "turns"
            }
        ),
    };
    let tokens = match goal.token_budget {
        Some(budget) => format!(
            "{}/{} tokens",
            format_tokens(goal.tokens_used.max(0) as u64),
            format_tokens(budget.max(0) as u64)
        ),
        None => format!("{} tokens", format_tokens(goal.tokens_used.max(0) as u64)),
    };
    format!(
        "{}{}{}",
        theme().fg(ThemeColor::Dim, "[goal "),
        theme().fg(color, &format!("● {label} · {turns} · {tokens}")),
        theme().fg(ThemeColor::Dim, "]"),
    )
}

pub trait FooterSession: Send + Sync {
    fn model(&self) -> Option<Model>;
    fn thinking_level(&self) -> ThinkingLevel;
    fn entries(&self) -> Vec<SessionEntry>;
    fn cwd(&self) -> String;
    fn session_name(&self) -> Option<String>;
    fn context_usage(&self) -> Option<ContextUsage>;
    fn active_mode(&self) -> Option<Mode>;
    fn is_using_subscription(&self, provider: &str) -> bool;
    /// The running goal, or `None` (port addition, v0.1.21).
    fn goal(&self) -> Option<ThreadGoal>;
    /// How many MCP servers are connected, and how many want attention
    /// (port addition, v0.1.22).
    fn mcp_summary(&self) -> McpSummary;
}

/// The MCP servers reduced to what the footer shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct McpSummary {
    pub connected: usize,
    /// Failed or awaiting authentication: a server that quietly failed at
    /// connect looks exactly like one whose tools the model chose not to use.
    pub needs_attention: usize,
}

impl FooterSession for AgentSession {
    fn model(&self) -> Option<Model> {
        AgentSession::model(self)
    }

    fn thinking_level(&self) -> ThinkingLevel {
        AgentSession::thinking_level(self)
    }

    fn entries(&self) -> Vec<SessionEntry> {
        self.with_session_manager(|manager| manager.get_entries())
    }

    fn cwd(&self) -> String {
        self.with_session_manager(|manager| manager.get_cwd().to_string())
    }

    fn session_name(&self) -> Option<String> {
        self.with_session_manager(|manager| manager.get_session_name())
    }

    fn context_usage(&self) -> Option<ContextUsage> {
        self.get_context_usage()
    }

    fn active_mode(&self) -> Option<Mode> {
        AgentSession::active_mode(self)
    }

    fn goal(&self) -> Option<ThreadGoal> {
        AgentSession::goal(self)
    }

    fn mcp_summary(&self) -> McpSummary {
        // The entries are already in memory; reading them needs the async lock,
        // and the footer renders on the UI thread. `try_lock` is the right
        // trade here: a summary that is one render out of date costs nothing,
        // a blocked render costs everything.
        let manager = AgentSession::mcp(self);
        let Some(servers) = manager.try_entries() else {
            return McpSummary::default();
        };
        let mut summary = McpSummary::default();
        for entry in servers {
            match entry.status {
                McpServerStatus::Connected => summary.connected += 1,
                McpServerStatus::Failed | McpServerStatus::NeedsAuth => {
                    summary.needs_attention += 1;
                }
                _ => {}
            }
        }
        summary
    }

    fn is_using_subscription(&self, provider: &str) -> bool {
        self.model_runtime().is_using_subscription(provider)
    }
}

/// What the footer reads from the data provider
/// (`ReadonlyFooterDataProvider` in TS).
pub trait FooterData: Send + Sync {
    fn get_git_branch(&self) -> Option<String>;
    fn get_available_provider_count(&self) -> u64;
}

impl FooterData for FooterDataProvider {
    fn get_git_branch(&self) -> Option<String> {
        FooterDataProvider::get_git_branch(self)
    }

    fn get_available_provider_count(&self) -> u64 {
        FooterDataProvider::get_available_provider_count(self)
    }
}

/// Footer component that shows pwd, token stats, and context usage.
pub struct FooterComponent {
    session: Arc<dyn FooterSession>,
    footer_data: Arc<dyn FooterData>,
}

impl FooterComponent {
    pub fn new(session: Arc<dyn FooterSession>, footer_data: Arc<dyn FooterData>) -> Self {
        Self {
            session,
            footer_data,
        }
    }

    pub fn set_session(&mut self, session: Arc<dyn FooterSession>) {
        self.session = session;
    }

    /// Clean up resources. Git watcher cleanup is handled by the provider.
    pub fn dispose(&mut self) {}
}

/// The colour of the footer's mode label.
///
/// The built-in modes take the reference's mode palette (takeover, user
/// decision 2026-08-18): plan green, auto yellow, yolo red; `manual` stays
/// neutral like the reference's `permission on`. A user-authored mode keeps
/// the shell signal [`indicator_color_key`] names — read-only green, working
/// yellow — so the one restriction worth seeing survives.
fn mode_indicator_color(id: &str, shell: ShellId) -> ThemeColor {
    match id {
        "plan" => ThemeColor::ModePlan,
        "auto" => ThemeColor::ModeAuto,
        "yolo" => ThemeColor::ModeYolo,
        "accept-edits" => ThemeColor::ModeAcceptEdits,
        "manual" => ThemeColor::Dim,
        _ => match indicator_color_key(shell) {
            "success" => ThemeColor::Success,
            _ => ThemeColor::Warning,
        },
    }
}

/// The `ThinkingLevel` literal, which is also what the footer prints.
fn thinking_level_value(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

fn entry_usage(value: Option<&Value>) -> Option<Usage> {
    serde_json::from_value::<Usage>(value?.clone()).ok()
}

impl Component for FooterComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let model = self.session.model();

        // Calculate cumulative usage from ALL session entries (not just
        // post-compaction messages)
        let mut usage_totals = create_usage_totals();
        let mut latest_cache_hit_rate: Option<f64> = None;

        for entry in self.session.entries() {
            match &entry {
                SessionEntry::Message(entry) => {
                    let role = entry.message.get("role").and_then(Value::as_str);
                    let usage = entry_usage(entry.message.get("usage"));
                    match (role, usage) {
                        (Some("assistant"), Some(usage)) => {
                            add_usage_to_totals(&mut usage_totals, &usage);
                            let latest_prompt_tokens =
                                usage.input + usage.cache_read + usage.cache_write;
                            latest_cache_hit_rate = (latest_prompt_tokens > 0).then(|| {
                                usage.cache_read as f64 / latest_prompt_tokens as f64 * 100.0
                            });
                        }
                        (Some("toolResult"), Some(usage)) => {
                            add_usage_to_totals(&mut usage_totals, &usage);
                        }
                        // An assistant message without usage still resets the
                        // rate, exactly as the TS branch does.
                        (Some("assistant"), None) => latest_cache_hit_rate = None,
                        _ => {}
                    }
                }
                SessionEntry::BranchSummary(entry) => {
                    if let Some(usage) = entry.usage.as_ref() {
                        add_usage_to_totals(&mut usage_totals, usage);
                    }
                }
                SessionEntry::Compaction(entry) => {
                    if let Some(usage) = entry.usage.as_ref() {
                        add_usage_to_totals(&mut usage_totals, usage);
                    }
                }
                _ => {}
            }
        }

        // Calculate context usage from session (handles compaction correctly).
        // After compaction, tokens are unknown until the next LLM response.
        let context_usage = self.session.context_usage();
        let context_window = context_usage
            .as_ref()
            .map(|usage| usage.context_window)
            .or_else(|| model.as_ref().map(|model| model.context_window))
            .unwrap_or(0);
        let context_percent_value = context_usage
            .as_ref()
            .and_then(|usage| usage.percent)
            .unwrap_or(0.0);
        let context_percent = match context_usage.as_ref().map(|usage| usage.percent) {
            Some(None) => "?".to_string(),
            _ => format!("{context_percent_value:.1}"),
        };

        // Replace home directory with ~
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok());
        let cwd = self.session.cwd();
        let mut pwd = format_cwd_for_footer(&cwd, home.as_deref());

        // Add git branch if available
        if let Some(branch) = self.footer_data.get_git_branch() {
            pwd = format!("{pwd} ({branch})");
        }

        // Add session name if set
        if let Some(session_name) = self.session.session_name() {
            pwd = format!("{pwd} • {session_name}");
        }

        // Build stats line
        let mut stats_parts: Vec<String> = Vec::new();
        // The mode leads: whether the agent may modify the workspace is the one
        // thing that must be readable at a glance. Colour comes from the shell,
        // so a user-authored read-only mode inherits the same signal.
        if let Some(active_mode) = self.session.active_mode() {
            stats_parts.push(theme().bold(&theme().fg(
                mode_indicator_color(&active_mode.id, active_mode.shell),
                &format_mode_label(&active_mode.id, active_mode.shell),
            )));
        }
        if usage_totals.input != 0 {
            stats_parts.push(format!("↑{}", format_tokens(usage_totals.input)));
        }
        if usage_totals.output != 0 {
            stats_parts.push(format!("↓{}", format_tokens(usage_totals.output)));
        }
        if usage_totals.cache_read != 0 {
            stats_parts.push(format!("R{}", format_tokens(usage_totals.cache_read)));
        }
        if usage_totals.cache_write != 0 {
            stats_parts.push(format!("W{}", format_tokens(usage_totals.cache_write)));
        }
        if (usage_totals.cache_read > 0 || usage_totals.cache_write > 0)
            && let Some(rate) = latest_cache_hit_rate
        {
            // Label deviates from the reference ("CH"): CHR reads as cache hit
            // rate, while CH next to the R/W token counts was ambiguous
            // (user decision 2026-08-20).
            stats_parts.push(format!("CHR{rate:.1}%"));
        }

        // Which providers count as subscription-backed is the session's answer
        // to give: the API-key-authenticated plans live there too, so every
        // caller sees the same truth (`ModelRuntime::is_using_subscription`).
        let using_subscription = model
            .as_ref()
            .is_some_and(|model| self.session.is_using_subscription(&model.provider));
        // A subscription is paid for by the month, so the per-token figure is
        // not money anyone owes — showing it invites reading a bill into it.
        // Nothing is shown at all in that case (user decision 2026-08-20,
        // superseding the earlier "sub" marker; the reference prints the
        // amount).
        let mcp = self.session.mcp_summary();
        if mcp.connected > 0 || mcp.needs_attention > 0 {
            let mut label = format!("mcp {}", mcp.connected);
            if mcp.needs_attention > 0 {
                label.push_str(&format!("+{}!", mcp.needs_attention));
            }
            stats_parts.push(if mcp.needs_attention > 0 {
                theme().fg(ThemeColor::Warning, &label)
            } else {
                label
            });
        }
        if !using_subscription && usage_totals.cost != 0.0 {
            stats_parts.push(format!("${:.3}", usage_totals.cost));
        }

        // Colorize context percentage based on usage. The reference appends
        // an " (auto)" marker when auto-compaction is on; dropped (user
        // decision 2026-08-20) — it carried no actionable information.
        let context_percent_display = if context_percent == "?" {
            format!("?/{}", format_tokens(context_window))
        } else {
            format!("{context_percent}%/{}", format_tokens(context_window))
        };
        let context_percent_str = if context_percent_value > 90.0 {
            theme().fg(ThemeColor::Error, &context_percent_display)
        } else if context_percent_value > 70.0 {
            theme().fg(ThemeColor::Warning, &context_percent_display)
        } else {
            context_percent_display
        };
        stats_parts.push(context_percent_str);
        if are_experimental_features_enabled() {
            stats_parts.push(format!(
                "{} {}",
                theme().fg(ThemeColor::Dim, "•"),
                theme().bold(&theme().fg(ThemeColor::Warning, "xp"))
            ));
        }

        let mut stats_left = stats_parts.join(" ");

        // Add model name on the right side, plus thinking level if model supports it
        let model_name = model
            .as_ref()
            .map(|model| model.id.clone())
            .unwrap_or_else(|| "no-model".to_string());

        // Every footer row is inset by one column on both sides, like the
        // reference's `FOOTER_PADDING_X` (takeover, user decision 2026-08-18).
        let indent = " ";
        let content_width = width.saturating_sub(2);

        // If statsLeft is too wide, truncate it
        let mut stats_left_width = visible_width(&stats_left);
        if stats_left_width > content_width {
            stats_left = truncate_to_width_opts(&stats_left, content_width, "...", false);
            stats_left_width = visible_width(&stats_left);
        }

        // Calculate available space for padding (minimum 2 spaces between stats
        // and model)
        let min_padding = 2;

        // Add thinking level indicator if model supports reasoning
        let mut right_side_without_provider = model_name.clone();
        if model.as_ref().is_some_and(|model| model.reasoning) {
            let thinking_level = self.session.thinking_level();
            right_side_without_provider = if thinking_level == ThinkingLevel::Off {
                format!("{model_name} • thinking off")
            } else {
                format!("{model_name} • {}", thinking_level_value(thinking_level))
            };
        }

        // Prepend the provider in parentheses if there are multiple providers
        // and there's enough room
        let mut right_side = right_side_without_provider.clone();
        if self.footer_data.get_available_provider_count() > 1
            && let Some(model) = model.as_ref()
        {
            right_side = format!("({}) {right_side_without_provider}", model.provider);
            if stats_left_width + min_padding + visible_width(&right_side) > content_width {
                // Too wide, fall back
                right_side = right_side_without_provider.clone();
            }
        }

        let right_side_width = visible_width(&right_side);
        let total_needed = stats_left_width + min_padding + right_side_width;

        let stats_line = if total_needed <= content_width {
            // Both fit - add padding to right-align model
            let padding = " ".repeat(content_width - stats_left_width - right_side_width);
            format!("{stats_left}{padding}{right_side}")
        } else {
            // Need to truncate right side
            let available_for_right = content_width.saturating_sub(stats_left_width + min_padding);
            if available_for_right > 0 {
                let truncated_right =
                    truncate_to_width_opts(&right_side, available_for_right, "", false);
                let truncated_right_width = visible_width(&truncated_right);
                let padding = " ".repeat(
                    content_width
                        .saturating_sub(stats_left_width)
                        .saturating_sub(truncated_right_width),
                );
                format!("{stats_left}{padding}{truncated_right}")
            } else {
                // Not enough space for right side at all
                stats_left.clone()
            }
        };

        // Apply dim to each part separately. statsLeft may contain color codes
        // (for context %) that end with a reset, which would clear an outer dim
        // wrapper. So we dim the parts before and after the colored section
        // independently.
        let dim_stats_left = theme().fg(ThemeColor::Dim, &stats_left);
        let remainder = &stats_line[stats_left.len()..]; // padding + rightSide
        let dim_remainder = theme().fg(ThemeColor::Dim, remainder);

        // The footer stacks under the input as: the mode/stats line with the
        // model right-aligned on it, then the working directory with its git
        // branch (user decision 2026-08-18).
        let pwd_line = format!(
            "{indent}{}",
            truncate_to_width_opts(
                &theme().fg(ThemeColor::Dim, &pwd),
                content_width,
                &theme().fg(ThemeColor::Dim, "..."),
                false,
            )
        );
        let mut lines = vec![format!("{indent}{dim_stats_left}{dim_remainder}"), pwd_line];
        // The goal badge gets its own line under the stats: a running loop
        // spending the user's money has to be readable at a glance, and it must
        // not compete with the model name for the right edge.
        if let Some(badge) = self.session.goal().as_ref().map(render_goal_badge) {
            lines.push(format!(
                "{indent}{}",
                truncate_to_width_opts(&badge, content_width, "", false)
            ));
        }
        shared_lines(lines)
    }

    /// No-op: the git branch is cached and invalidated by the provider.
    fn invalidate(&mut self) {}
}
