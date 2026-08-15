//! Interactive-mode components — port of
//! `packages/coding-agent/src/modes/interactive/components/`.
//!
//! The module list mirrors the re-exports of `components/index.ts`; modules
//! appear as their batch is ported (see `crates/notagent/PARITY.md`, section
//! "A: interactive components").

pub mod approval_selector;
pub mod armin;
pub mod assistant_message;
pub mod bash_execution;
pub mod bordered_loader;
pub mod branch_summary_message;
pub mod compaction_summary_message;
pub mod config_selector;
pub mod countdown_timer;
pub mod custom_editor;
pub mod custom_message;
pub mod daxnuts;
pub mod diff;
pub mod dynamic_border;
pub mod earendil_announcement;
pub mod first_time_setup;
pub mod keybinding_hints;
pub mod list_selector;
pub mod login_dialog;
pub mod markdown_transform;
pub mod model_selector;
pub mod oauth_selector;
pub mod scoped_models_selector;
pub mod session_selector;
pub mod session_selector_search;
pub mod settings_selector;
pub mod show_images_selector;
pub mod skill_invocation_message;
pub mod status_indicator;
pub mod subagent_panel;
pub mod tasks_browser;
pub mod tasks_panel;
pub mod theme_selector;
pub mod thinking_selector;
pub mod todo_list;
pub mod tree_selector;
pub mod trust_selector;
pub mod user_message;
pub mod user_message_selector;
pub mod visual_truncate;

/// `Number.prototype.toLocaleString()` for the token counts the message
/// components print.
///
/// Node resolves the default locale from the environment; in this application
/// it is `en-US`, whose only grouping rule is a comma every three digits. The
/// port fixes that rule instead of pulling in an ICU dependency the master plan
/// does not list.
pub fn to_locale_string(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::to_locale_string;

    #[test]
    fn groups_thousands_like_node() {
        // Values taken from `node -e "console.log((1234).toLocaleString())"`.
        assert_eq!(to_locale_string(0), "0");
        assert_eq!(to_locale_string(1), "1");
        assert_eq!(to_locale_string(999), "999");
        assert_eq!(to_locale_string(1_000), "1,000");
        assert_eq!(to_locale_string(1_234), "1,234");
        assert_eq!(to_locale_string(1_000_000), "1,000,000");
        assert_eq!(to_locale_string(12_345_678), "12,345,678");
    }
}
