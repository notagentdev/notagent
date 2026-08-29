use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PermissionRequest,
    PermissionResult,
    UserPromptSubmit,
    UserPromptQueued,
    TurnStarted,
    Stop,
    StopFailure,
    Interrupt,
    SessionStart,
    SessionEnd,
    SubagentStart,
    SubagentStop,
    TaskStarted,
    PreCompact,
    PostCompact,
    Notification,
}

pub const HOOK_EVENTS: [HookEvent; 19] = [
    HookEvent::PreToolUse,
    HookEvent::PostToolUse,
    HookEvent::PostToolUseFailure,
    HookEvent::PermissionRequest,
    HookEvent::PermissionResult,
    HookEvent::UserPromptSubmit,
    HookEvent::UserPromptQueued,
    HookEvent::TurnStarted,
    HookEvent::Stop,
    HookEvent::StopFailure,
    HookEvent::Interrupt,
    HookEvent::SessionStart,
    HookEvent::SessionEnd,
    HookEvent::SubagentStart,
    HookEvent::SubagentStop,
    HookEvent::TaskStarted,
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
            HookEvent::UserPromptQueued => "UserPromptQueued",
            HookEvent::TurnStarted => "TurnStarted",
            HookEvent::Stop => "Stop",
            HookEvent::StopFailure => "StopFailure",
            HookEvent::Interrupt => "Interrupt",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::SubagentStart => "SubagentStart",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::TaskStarted => "TaskStarted",
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

    /// Events whose commands run before the guarded action and may refuse it.
    pub fn is_blocking(self) -> bool {
        matches!(self, HookEvent::PreToolUse | HookEvent::UserPromptSubmit)
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// `HOOK_EVENTS.join(", ")`, for the diagnostic that lists what is valid.
pub fn hook_event_list() -> String {
    HOOK_EVENTS
        .iter()
        .map(|event| event.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
