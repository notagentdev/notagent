//! Port of `packages/coding-agent/src/core/hooks/events.ts`.
//!
//! The lifecycle events a hook can be declared for.
//!
//! Taken verbatim from the reference implementation. The names are a public
//! interface in two directions: users write them in hook files, and external
//! supervisors watch for them to follow what the agent is doing. NotMux already
//! recognises this vocabulary, so a rename here silently stops it seeing us —
//! treat any change as breaking.
//!
//! NotMux spells the prompt event `beforeSubmitPrompt`; we emit
//! `UserPromptSubmit`, matching the reference rather than the consumer, so
//! there is one name rather than an alias to keep in step.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionRequest,
    PermissionResult,
    UserPromptSubmit,
    Stop,
    StopFailure,
    Interrupt,
    SessionStart,
    SessionEnd,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    Notification,
}

pub const HOOK_EVENTS: [HookEvent; 16] = [
    HookEvent::PreToolUse,
    HookEvent::PostToolUse,
    HookEvent::PostToolUseFailure,
    HookEvent::PermissionRequest,
    HookEvent::PermissionResult,
    HookEvent::UserPromptSubmit,
    HookEvent::Stop,
    HookEvent::StopFailure,
    HookEvent::Interrupt,
    HookEvent::SessionStart,
    HookEvent::SessionEnd,
    HookEvent::SubagentStart,
    HookEvent::SubagentStop,
    HookEvent::PreCompact,
    HookEvent::PostCompact,
    HookEvent::Notification,
];

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::PermissionRequest => "PermissionRequest",
            HookEvent::PermissionResult => "PermissionResult",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Stop => "Stop",
            HookEvent::StopFailure => "StopFailure",
            HookEvent::Interrupt => "Interrupt",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::SubagentStart => "SubagentStart",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::PostCompact => "PostCompact",
            HookEvent::Notification => "Notification",
        }
    }

    /// `isHookEvent`: the exact spelling, nothing else.
    pub fn parse(value: &str) -> Option<Self> {
        HOOK_EVENTS
            .into_iter()
            .find(|event| event.as_str() == value)
    }

    /// Events that carry a tool name, and are therefore the ones a matcher can
    /// narrow. A matcher on any other event would silently never apply.
    pub fn is_tool_scoped(self) -> bool {
        matches!(
            self,
            HookEvent::PreToolUse
                | HookEvent::PostToolUse
                | HookEvent::PostToolUseFailure
                | HookEvent::PermissionRequest
                | HookEvent::PermissionResult
        )
    }

    /// Events not emitted yet because the feature that would raise them does not
    /// exist. Declaring one is accepted and reported, rather than silently doing
    /// nothing.
    pub fn is_unemitted(self) -> bool {
        matches!(self, HookEvent::SubagentStart | HookEvent::SubagentStop)
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The only event whose hook can refuse a call. Blocking anywhere else has no
/// meaning: the action has already happened, or there is nothing to stop.
pub const BLOCKING_EVENT: HookEvent = HookEvent::PreToolUse;

/// `HOOK_EVENTS.join(", ")`, for the diagnostic that lists what is valid.
pub fn hook_event_list() -> String {
    HOOK_EVENTS
        .iter()
        .map(|event| event.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
