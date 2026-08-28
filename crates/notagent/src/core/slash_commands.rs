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
/// string, so the description is spelled out here rather than formatted at
/// runtime, and the assertion in the tests keeps the two tied together.
pub const BUILTIN_SLASH_COMMANDS: &[BuiltinSlashCommand] = &[
    command("settings", "Open settings menu"),
    command_with_hint(
        "model",
        "Select model (opens selector UI)",
        "<provider/model>",
    ),
    command_with_hint(
        "subagent-model",
        "Select the model subagents run on (default: inherit the main model)",
        "<provider/model|default>",
    ),
    command("scoped-models", "Enable/disable models for Ctrl+P cycling"),
    // command form of the reference's Indexing settings section.
    command_with_hint(
        "index",
        "Rebuild the local codebase index (find_codebase), or toggle it",
        "[on|off]",
    ),
    // command form of the reference's Atomic leases setting.
    command_with_hint(
        "leases",
        "Toggle atomic file leases for the mutating file tools (default: off)",
        "[on|off]",
    ),
    // command form of the reference's shell-output filter setting.
    command_with_hint(
        "bash-filter",
        "Toggle compaction of bash output before it enters the context (default: off)",
        "[on|off]",
    ),
    // child explores the project and writes its `AGENTS.md`.
    command("init", "Explore the project and write AGENTS.md for it"),
    // the conversation does not have to carry.
    command_with_hint(
        "btw",
        "Ask a forked side agent a question, without touching this conversation",
        "[question]",
    ),
    // mode, where the agent keeps working toward an objective on its own.
    command_with_hint(
        "goal",
        "Set a goal the agent pursues across turns, or show, pause, resume or clear it",
        "[replace] [strict] <objective> [--turns N] [--tokens N] | pause | resume | clear",
    ),
    // configured MCP servers and their state.
    command_with_hint(
        "mcp",
        "List, add, remove, inspect, reconnect, sign in to or out of the MCP servers",
        "[lend [off] | import [user] <json> | remove [user] <server> | show <server> | \
         reconnect <server> | login <server> | logout <server|all> | reload]",
    ),
    // bound to a key.
    command(
        "thinking",
        "Show or hide the thinking blocks in the transcript",
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
    fn the_command_list_has_stable_order() {
        let names: Vec<&str> = BUILTIN_SLASH_COMMANDS
            .iter()
            .map(|command| command.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "settings",
                "model",
                "subagent-model",
                "scoped-models",
                "index",
                "leases",
                "bash-filter",
                "init",
                "btw",
                "goal",
                "mcp",
                "thinking",
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
        // v0.1.6 through v0.1.22).
        assert_eq!(
            with_hint,
            vec![
                "model",
                "subagent-model",
                "index",
                "leases",
                "bash-filter",
                "btw",
                "goal",
                "mcp",
                "login"
            ]
        );
    }
}
