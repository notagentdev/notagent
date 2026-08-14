//! Port of `packages/coding-agent/src/core/defaults.ts` (3 LOC).

use notagent_agent::types::ThinkingLevel;

/// `DEFAULT_THINKING_LEVEL: ThinkingLevel = "medium"`
pub const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Medium;
