//! The two subagent types.
//!
//! Delegation used to take a *mode*, and the four shipped modes are `auto`,
//! `manual`, `plan` and `yolo`. Three of those are worker shells that differ only in how
//! often the user is asked — an axis that means nothing to a child, because the
//! child is not the one being asked. Presenting them as three roles gave the
//! model a choice with no content, which is why it kept choosing `plan`: the
//! only entry that visibly differed.
//!
//! So the roster is the one distinction that survives contact with a subagent:
//! may it change the workspace or not. Everything else a child needs in order to
//! be good at something arrives as a skill, loaded at the start of its run.
//!
//! Keeping this an enumeration rather than a loadable roster is the design, not
//! a shortcut. A loadable agent type carries its own prompt, which can disagree
//! with the skill it loads, and the user then has two places to look when a
//! child misbehaves. Two types and a skill catalogue have only one.

use crate::core::modes::shells::{ShellId, tools_for_shell};
use crate::core::tools::ToolName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubagentType {
    /// Reads, searches, plans. Cannot change anything.
    ReadOnly,
    /// Everything the parent can do.
    Worker,
}

pub const SUBAGENT_TYPES: [SubagentType; 2] = [SubagentType::ReadOnly, SubagentType::Worker];

impl SubagentType {
    /// Spelled exactly like the shell it runs in. One vocabulary for one
    /// distinction: a reader who knows what `read-only` means as a shell
    /// already knows what it means as an agent type.
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentType::ReadOnly => "read-only",
            SubagentType::Worker => "worker",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        SUBAGENT_TYPES
            .into_iter()
            .find(|agent| agent.as_str() == value)
    }

    pub fn shell(self) -> ShellId {
        match self {
            SubagentType::ReadOnly => ShellId::ReadOnly,
            SubagentType::Worker => ShellId::Worker,
        }
    }

    /// What the type is for, as the delegating model reads it.
    ///
    /// Says what the child can accomplish rather than which tools it holds. The
    /// tool list was what made the old four-mode listing unreadable — a wall of
    /// names that is identical between the entries a model has to tell apart.
    pub fn purpose(self) -> &'static str {
        match self {
            SubagentType::ReadOnly => {
                "Reads, searches and reasons about the codebase, and can record a plan under plans/. Cannot edit, write or run commands — enforced by the runtime, so a task that turns out to need a change comes back reporting that rather than making it."
            }
            SubagentType::Worker => {
                "Everything this session can do: reading, editing, writing and the shell. Its tool calls are approved exactly like this session's, so delegating does not move work out from under the rules the user set."
            }
        }
    }

    /// The type's full allowlist, before the exclusions every child gets.
    pub fn tools(self) -> Vec<ToolName> {
        tools_for_shell(self.shell())
    }
}

impl std::fmt::Display for SubagentType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a parent in `parent` may delegate to `child`.
///
/// The approval level is no longer part of this question — a child's level is
/// copied from its parent at every call rather than chosen, so there is nothing
/// to escalate. What remains is the shell: a read-only session must not be able
/// to reach a worker, or delegation would be a way out of the gate.
pub fn may_delegate_to(parent: Option<ShellId>, child: SubagentType) -> bool {
    parent != Some(ShellId::ReadOnly) || child == SubagentType::ReadOnly
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_the_declared_spellings() {
        assert_eq!(
            SubagentType::parse("read-only"),
            Some(SubagentType::ReadOnly)
        );
        assert_eq!(SubagentType::parse("worker"), Some(SubagentType::Worker));
        assert_eq!(SubagentType::parse("readonly"), None);
        assert_eq!(SubagentType::parse("plan"), None);
    }

    #[test]
    fn a_read_only_type_holds_no_mutating_tool() {
        let tools = SubagentType::ReadOnly.tools();
        for mutating in crate::core::modes::shells::MUTATING_TOOLS {
            assert!(
                !tools.contains(mutating),
                "the read-only type holds {}",
                mutating.as_str()
            );
        }
    }

    #[test]
    fn a_read_only_type_can_still_record_a_plan() {
        assert!(
            SubagentType::ReadOnly
                .tools()
                .contains(&ToolName::PlanCreate)
        );
    }

    #[test]
    fn a_read_only_parent_may_only_reach_a_read_only_child() {
        assert!(may_delegate_to(
            Some(ShellId::ReadOnly),
            SubagentType::ReadOnly
        ));
        assert!(!may_delegate_to(
            Some(ShellId::ReadOnly),
            SubagentType::Worker
        ));
        assert!(may_delegate_to(Some(ShellId::Worker), SubagentType::Worker));
        assert!(may_delegate_to(None, SubagentType::Worker));
    }
}
