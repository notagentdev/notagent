//! Port of `packages/coding-agent/src/core/slash-commands.ts`.
//!
//! The built-in slash commands and the shape the autocomplete reads. A command
//! shown to the user comes from one of three places — a prompt template, a
//! skill, or the built-in list below — and `SlashCommandInfo` is what makes
//! them interchangeable at the point of display.
//!
//! The `Extension` source is retained because it is a wire value: the RPC mode
//! reports it and session transcripts written by the TypeScript app carry it.
//! Nothing in this port produces one (see `plans/facts/extension-boundary.md`).

use crate::core::source_info::SourceInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlashCommandSource {
    Extension,
    Prompt,
    Skill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: Option<String>,
    pub source: SlashCommandSource,
    pub source_info: SourceInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinSlashCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub argument_hint: Option<&'static str>,
}

const fn command(name: &'static str, description: &'static str) -> BuiltinSlashCommand {
    BuiltinSlashCommand {
        name,
        description,
        argument_hint: None,
    }
}

const fn command_with_hint(
    name: &'static str,
    description: &'static str,
    argument_hint: &'static str,
) -> BuiltinSlashCommand {
    BuiltinSlashCommand {
        name,
        description,
        argument_hint: Some(argument_hint),
    }
}

/// The built-in commands, in the order the UI lists them.
///
/// `/quit` interpolates the app name in TypeScript; the constant is the same
/// string, so the description is spelled out here rather than formatted at
/// runtime, and the assertion in the tests keeps the two tied together.
pub const BUILTIN_SLASH_COMMANDS: &[BuiltinSlashCommand] = &[
    command("settings", "Open settings menu"),
    command_with_hint(
        "model",
        "Select model (opens selector UI)",
        "<provider/model>",
    ),
    // Addition over the TS original (user decision 2026-08-16, v0.1.6).
    command_with_hint(
        "subagent-model",
        "Select the model subagents run on (default: inherit the main model)",
        "<provider/model|default>",
    ),
    command("scoped-models", "Enable/disable models for Ctrl+P cycling"),
    // Addition over the TS original (user decision 2026-08-16, v0.1.7): the
    // command form of the reference's Indexing settings section.
    command_with_hint(
        "index",
        "Rebuild the local codebase index (find_codebase), or toggle it",
        "[on|off]",
    ),
    command(
        "export",
        "Export session (HTML default, or specify path: .html/.jsonl)",
    ),
    command("import", "Import and resume a session from a JSONL file"),
    command("share", "Share session as a secret GitHub gist"),
    command("copy", "Copy last agent message to clipboard"),
    command("name", "Set session display name"),
    command("session", "Show session info and stats"),
    command("tasks", "Browse background tasks and subagents"),
    command("changelog", "Show changelog entries"),
    command("hotkeys", "Show all keyboard shortcuts"),
    command("fork", "Create a new fork from a previous user message"),
    command(
        "clone",
        "Duplicate the current session at the current position",
    ),
    command("tree", "Navigate session tree (switch branches)"),
    command("trust", "Save project trust decision for future sessions"),
    command_with_hint("login", "Configure provider authentication", "<provider>"),
    command("logout", "Remove provider authentication"),
    command("new", "Start a new session"),
    command("compact", "Manually compact the session context"),
    command("resume", "Resume a different session"),
    command(
        "reload",
        "Reload keybindings, extensions, skills, prompts, themes, and context files",
    ),
    command("quit", "Quit notagent"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::APP_NAME;

    #[test]
    fn the_quit_description_still_carries_the_app_name() {
        let quit = BUILTIN_SLASH_COMMANDS
            .iter()
            .find(|command| command.name == "quit")
            .expect("quit command");
        assert_eq!(quit.description, format!("Quit {APP_NAME}"));
    }

    #[test]
    fn the_command_list_matches_the_typescript_order() {
        let names: Vec<&str> = BUILTIN_SLASH_COMMANDS
            .iter()
            .map(|command| command.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "settings",
                "model",
                // Port addition (user decision, v0.1.6), not in the TS list.
                "subagent-model",
                "scoped-models",
                // Port addition (user decision, v0.1.7), not in the TS list.
                "index",
                "export",
                "import",
                "share",
                "copy",
                "name",
                "session",
                "tasks",
                "changelog",
                "hotkeys",
                "fork",
                "clone",
                "tree",
                "trust",
                "login",
                "logout",
                "new",
                "compact",
                "resume",
                "reload",
                "quit",
            ]
        );
    }

    #[test]
    fn only_the_model_commands_and_login_take_an_argument() {
        let with_hint: Vec<&str> = BUILTIN_SLASH_COMMANDS
            .iter()
            .filter(|command| command.argument_hint.is_some())
            .map(|command| command.name)
            .collect();
        // `subagent-model` and `index` are port additions (user decisions,
        // v0.1.6/v0.1.7).
        assert_eq!(with_hint, vec!["model", "subagent-model", "index", "login"]);
    }
}
