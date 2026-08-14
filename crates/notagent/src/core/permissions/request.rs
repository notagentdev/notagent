//! Port of `packages/coding-agent/src/core/permissions/request.ts`.
//!
//! The approval request: what a user is shown when a policy asks.
//!
//! Presentation is kept separate from the dialog itself so the wording is
//! testable without a terminal, and so the same request can be rendered by the
//! TUI, by a headless runner, or in a transcript.
//!
//! The three things a user needs to answer are always present: which tool is
//! asking, what it would touch, and which rule decided that asking was
//! necessary. The last one matters most — without it an unexpected prompt has
//! no explanation, and a user cannot tell a sensitive-file guard from an
//! ordinary unrecognised call.

use std::sync::LazyLock;

use regex::Regex;

use super::policy::{PermissionContext, PermissionDecision, PermissionEvaluation};
use crate::utils::paths::{node_relative, node_resolve};

/// What the user may answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalAnswer {
    ApproveOnce,
    ApproveAlways,
    Deny,
}

impl ApprovalAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalAnswer::ApproveOnce => "approve-once",
            ApprovalAnswer::ApproveAlways => "approve-always",
            ApprovalAnswer::Deny => "deny",
        }
    }
}

impl std::fmt::Display for ApprovalAnswer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Tool requesting permission.
    pub tool_name: String,
    /// Path or argument summary the call would act on, if any.
    pub target: Option<String>,
    /// Policy that decided this needs asking.
    pub policy_name: String,
    /// Why it asked, when the policy supplied a reason.
    pub reason: Option<String>,
    /// Active mode, so the prompt is legible in context.
    pub mode_id: Option<String>,
}

const PATH_ARGUMENTS: [&str; 2] = ["path", "file_path"];

static WHITESPACE_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("whitespace"));

/// Shortens a path against the working directory, leaving outside paths absolute.
pub fn describe_target(context: &PermissionContext) -> Option<String> {
    for key in PATH_ARGUMENTS {
        let Some(value) = context.input.get(key).and_then(|value| value.as_str()) else {
            continue;
        };
        if value.trim().is_empty() {
            continue;
        }
        let absolute = node_resolve(&[&context.cwd, value]);
        let relative = node_relative(&node_resolve(&[&context.cwd]), &absolute);
        return Some(if !relative.is_empty() && !relative.starts_with("..") {
            relative
        } else {
            absolute
        });
    }

    let command = context
        .input
        .get("command")
        .and_then(|value| value.as_str())?;
    if command.trim().is_empty() {
        return None;
    }
    let single = WHITESPACE_RUN.replace_all(command, " ").trim().to_string();
    // `slice(0, 60)` counts UTF-16 units.
    let units: Vec<u16> = single.encode_utf16().collect();
    Some(if units.len() > 60 {
        format!("{}…", String::from_utf16_lossy(&units[..60]))
    } else {
        single
    })
}

/// Builds the request from an asking evaluation. Returns `None` otherwise.
pub fn build_approval_request(
    context: &PermissionContext,
    evaluation: &PermissionEvaluation,
) -> Option<ApprovalRequest> {
    let PermissionDecision::Ask { reason } = &evaluation.decision else {
        return None;
    };
    Some(ApprovalRequest {
        tool_name: context.tool_name.clone(),
        target: describe_target(context),
        policy_name: evaluation.policy_name.clone(),
        reason: reason.clone(),
        mode_id: context.mode_id.clone(),
    })
}

/// One-line summary: the tool and what it would touch.
pub fn format_request_summary(request: &ApprovalRequest) -> String {
    match &request.target {
        Some(target) => format!("{} {target}", request.tool_name),
        None => request.tool_name.clone(),
    }
}

/// The explanation line. Always names the deciding policy, so an unexpected
/// prompt can be traced to the rule that produced it.
pub fn format_request_explanation(request: &ApprovalRequest) -> String {
    match &request.reason {
        Some(reason) => format!("{reason} ({})", request.policy_name),
        None => format!("Asked by {}.", request.policy_name),
    }
}

/// Answer labels, in the order a dialog should offer them.
pub const APPROVAL_ANSWERS: [(ApprovalAnswer, &str); 3] = [
    (ApprovalAnswer::ApproveOnce, "Allow once"),
    (ApprovalAnswer::ApproveAlways, "Allow for this session"),
    (ApprovalAnswer::Deny, "Deny"),
];

/// Whether an answer permits the call. Kept as a function rather than a
/// comparison at each call site so adding an answer cannot silently default to
/// permitting.
pub fn answer_allows(answer: ApprovalAnswer) -> bool {
    matches!(
        answer,
        ApprovalAnswer::ApproveOnce | ApprovalAnswer::ApproveAlways
    )
}

/// Whether an answer should be remembered for the rest of the session.
pub fn answer_persists(answer: ApprovalAnswer) -> bool {
    answer == ApprovalAnswer::ApproveAlways
}
