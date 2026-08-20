//! Port of `packages/coding-agent/src/core/modes/shells.ts`.
//!
//! Shells: the single permission axis shared by main-agent modes and subagents.
//!
//! A shell decides what a session may *do*; a mode's skills only decide how it
//! behaves. Keeping the two apart is deliberate — a skill is text the model may
//! ignore, while a shell is a tool allowlist the runtime enforces. Both the
//! main agent's modes and delegated children derive their tools from here, so
//! the two can never drift apart.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::core::tools::ToolName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShellId {
    ReadOnly,
    Worker,
}

/// How much runs without the user, the second axis beside the shell.
///
/// The shell decides which tools exist at all; the approval level decides
/// whether an existing tool runs unattended. Ordered from most to least
/// supervised, which is also the ring order once the read-only stop is put in
/// front of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalLevel {
    /// The default everywhere an approval level is missing: an undeclared
    /// level must never resolve to running unattended.
    #[default]
    Manual,
    Auto,
    Yolo,
}

pub const APPROVAL_LEVELS: [ApprovalLevel; 3] = [
    ApprovalLevel::Manual,
    ApprovalLevel::Auto,
    ApprovalLevel::Yolo,
];

/// Level applied when a mode declares none. Always the supervised end: an
/// undeclared approval level must never resolve to running unattended.
pub const DEFAULT_APPROVAL_LEVEL: ApprovalLevel = ApprovalLevel::Manual;

pub const SHELL_IDS: [ShellId; 2] = [ShellId::ReadOnly, ShellId::Worker];

impl ApprovalLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalLevel::Manual => "manual",
            ApprovalLevel::Auto => "auto",
            ApprovalLevel::Yolo => "yolo",
        }
    }

    /// The `isApprovalLevel` guard: only these three spellings are a level.
    pub fn parse(value: &str) -> Option<Self> {
        APPROVAL_LEVELS
            .into_iter()
            .find(|level| level.as_str() == value)
    }
}

impl std::fmt::Display for ApprovalLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl ShellId {
    pub fn as_str(self) -> &'static str {
        match self {
            ShellId::ReadOnly => "read-only",
            ShellId::Worker => "worker",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        SHELL_IDS.into_iter().find(|shell| shell.as_str() == value)
    }
}

impl std::fmt::Display for ShellId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Tools available to a read-only shell.
///
/// `bash` is deliberately absent. A shell is a hard gate, and a shell that can
/// spawn arbitrary commands is not read-only in any meaningful sense — the
/// model could write files through a shell command. Exploration is covered by
/// grep, find and ls.
const READ_ONLY_TOOLS: &[ToolName] = &[
    ToolName::Read,
    ToolName::ReadMinified,
    ToolName::Grep,
    ToolName::FindFilesystem,
    ToolName::FindCodebase,
    ToolName::Ls,
    // Loading guidance and delegating read-only work change nothing, so both
    // belong in the read-only set. A child never inherits `task`, and a
    // read-only session may only delegate read-only work, so neither is a way
    // around the gate.
    ToolName::Skill,
    ToolName::Task,
    // Observing background work touches no file either. `task_stop` does end a
    // running command, but only one this session started — and a session that
    // may start background work has to be able to stop it, or a read-only
    // session could strand a process it can see and cannot reach.
    ToolName::TaskList,
    ToolName::TaskOutput,
    ToolName::TaskStop,
    // Keeping a checklist changes nothing in the workspace, and a read-only
    // session doing multi-step research wants one as much as a worker does.
    ToolName::TodoWrite,
    // Port addition (v0.1.24). The one tool here that writes to disk, and the
    // exception is narrow enough to keep the gate meaningful: it only ever
    // creates a new file under `plans/`, never overwrites, and derives the
    // name rather than taking it. A read-only session cannot reach anything
    // that was already there — and a plan mode that cannot record its plan is
    // the contradiction the shell exists to avoid.
    ToolName::PlanCreate,
];

/// Tools a worker shell adds on top of the read-only set.
const WORKER_ADDITIONAL_TOOLS: &[ToolName] = &[
    ToolName::Bash,
    ToolName::Edit,
    ToolName::Write,
    ToolName::PatchMinified,
    ToolName::MultiPatchMinified,
];

/// Tools that mutate the workspace. Never present in a read-only shell.
pub const MUTATING_TOOLS: &[ToolName] = WORKER_ADDITIONAL_TOOLS;

/// The tool allowlist for a shell, in registry order.
pub fn tools_for_shell(shell: ShellId) -> Vec<ToolName> {
    match shell {
        ShellId::ReadOnly => READ_ONLY_TOOLS.to_vec(),
        ShellId::Worker => READ_ONLY_TOOLS
            .iter()
            .chain(WORKER_ADDITIONAL_TOOLS)
            .copied()
            .collect(),
    }
}

/// What `applyToolDelta` produced: the allowlist and what could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDeltaResult {
    pub tools: Vec<ToolName>,
    pub problems: Vec<String>,
}

/// Applies a mode's optional tool delta to its shell's allowlist.
///
/// Entries are tool names, optionally prefixed with `+` to add or `-` to
/// remove. Adding a mutating tool to a read-only shell is rejected: that would
/// turn the gate into a suggestion, which is exactly what shells exist to
/// prevent. Unknown names are reported rather than silently dropped.
pub fn apply_tool_delta(
    shell: ShellId,
    delta: Option<&[String]>,
    known_tool_names: &HashSet<String>,
) -> ToolDeltaResult {
    // A `Set` in TypeScript: insertion order decides the resulting order, and
    // adding a tool that is already there does not move it.
    let mut tools = tools_for_shell(shell);
    let mut problems: Vec<String> = Vec::new();
    let Some(delta) = delta else {
        return ToolDeltaResult { tools, problems };
    };

    for raw in delta {
        let entry = raw.trim();
        if entry.is_empty() {
            continue;
        }
        let remove = entry.starts_with('-');
        let name = if remove || entry.starts_with('+') {
            entry[1..].trim()
        } else {
            entry
        };

        if !known_tool_names.contains(name) {
            problems.push(format!("unknown tool \"{name}\""));
            continue;
        }
        // A name the session knows but this build has no `ToolName` for cannot
        // enter the allowlist; `knownToolNames` is the wider set in TypeScript
        // too, since an extension could contribute one.
        let Some(tool) = ToolName::parse(name) else {
            if !remove {
                problems.push(format!("unknown tool \"{name}\""));
            }
            continue;
        };
        if remove {
            tools.retain(|existing| *existing != tool);
            continue;
        }
        if shell == ShellId::ReadOnly && MUTATING_TOOLS.contains(&tool) {
            problems.push(format!(
                "tool \"{name}\" mutates the workspace and cannot be added to a read-only shell"
            ));
            continue;
        }
        if !tools.contains(&tool) {
            tools.push(tool);
        }
    }

    ToolDeltaResult { tools, problems }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_the_declared_spellings() {
        assert_eq!(ShellId::parse("read-only"), Some(ShellId::ReadOnly));
        assert_eq!(ShellId::parse("readonly"), None);
        assert_eq!(ApprovalLevel::parse("yolo"), Some(ApprovalLevel::Yolo));
        assert_eq!(ApprovalLevel::parse("YOLO"), None);
    }
}
