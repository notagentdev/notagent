use notagent::cli::args::{
    ArgDiagnostic, DiagnosticLevel, ListModels, OutputMode, StartMode, UnknownFlagValue,
    parse_args, unknown_flags_error,
};
use notagent::core::settings_manager::TuiMode;
use notagent_agent::types::ThinkingLevel;

fn parse(args: &[&str]) -> notagent::cli::args::Args {
    parse_args(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn parses_the_version_flag() {
    assert!(parse(&["--version"]).version);
    assert!(parse(&["-v"]).version);
    let result = parse(&["--version", "--model", "gpt-4"]);
    assert!(result.version);
    assert_eq!(result.model.as_deref(), Some("gpt-4"));
}

#[test]
fn parses_the_help_flag() {
    assert!(parse(&["--help"]).help);
    assert!(parse(&["-h"]).help);
}

#[test]
fn parses_the_print_flag_and_the_prompt_after_it() {
    assert!(parse(&["--print"]).print);
    assert!(parse(&["-p"]).print);

    // A message that opens with YAML frontmatter is not a flag.
    let result = parse(&["-p", "---\ntitle: x\n---\nbody"]);
    assert!(result.print);
    assert_eq!(result.messages, strings(&["---\ntitle: x\n---\nbody"]));

    // Options after -p stay options.
    let result = parse(&["-p", "--model", "gpt-4"]);
    assert!(result.print);
    assert!(result.messages.is_empty());
    assert_eq!(result.model.as_deref(), Some("gpt-4"));
}

#[test]
fn parses_the_continue_and_resume_flags() {
    assert!(parse(&["--continue"]).continue_session);
    assert!(parse(&["-c"]).continue_session);
    assert!(parse(&["--resume"]).resume);
    assert!(parse(&["-r"]).resume);
}

#[test]
fn parses_the_flags_that_take_a_value() {
    assert_eq!(
        parse(&["--provider", "openai"]).provider.as_deref(),
        Some("openai")
    );
    assert_eq!(parse(&["--model", "gpt-4"]).model.as_deref(), Some("gpt-4"));
    assert_eq!(
        parse(&["--api-key", "sk-1"]).api_key.as_deref(),
        Some("sk-1")
    );
    assert_eq!(
        parse(&["--system-prompt", "be terse"])
            .system_prompt
            .as_deref(),
        Some("be terse")
    );
    assert_eq!(
        parse(&["--append-system-prompt", "extra"]).append_system_prompt,
        Some(strings(&["extra"]))
    );
    assert_eq!(
        parse(&[
            "--append-system-prompt",
            "one",
            "--append-system-prompt",
            "two"
        ])
        .append_system_prompt,
        Some(strings(&["one", "two"]))
    );
    assert_eq!(parse(&["--mode", "json"]).mode, Some(OutputMode::Json));
    assert_eq!(parse(&["--mode", "rpc"]).mode, Some(OutputMode::Rpc));
    assert_eq!(parse(&["--session", "abc"]).session.as_deref(), Some("abc"));
    assert_eq!(
        parse(&["--session-id", "abc"]).session_id.as_deref(),
        Some("abc")
    );
    assert_eq!(parse(&["--fork", "abc"]).fork.as_deref(), Some("abc"));
    assert_eq!(
        parse(&["--export", "out.html"]).export.as_deref(),
        Some("out.html")
    );
    assert_eq!(
        parse(&["--thinking", "high"]).thinking,
        Some(ThinkingLevel::High)
    );
    assert_eq!(
        parse(&["--models", "a, b ,c"]).models,
        Some(strings(&["a", "b", "c"]))
    );
}

#[test]
fn the_name_flag_reports_a_missing_value() {
    assert_eq!(
        parse(&["--name", "session"]).name.as_deref(),
        Some("session")
    );
    assert_eq!(parse(&["-n", "session"]).name.as_deref(), Some("session"));
    // An empty value is kept, so the caller can validate it.
    assert_eq!(parse(&["--name", ""]).name.as_deref(), Some(""));

    let result = parse(&["--name"]);
    assert_eq!(
        result.diagnostics,
        vec![ArgDiagnostic {
            level: DiagnosticLevel::Error,
            message: "--name requires a value".to_string(),
        }]
    );

    let result = parse(&["--name", "x", "hello"]);
    assert_eq!(result.name.as_deref(), Some("x"));
    assert_eq!(result.messages, strings(&["hello"]));
}

#[test]
fn parses_the_resource_flags() {
    assert!(parse(&["--no-session"]).no_session);
    assert_eq!(parse(&["--skill", "./a"]).skills, Some(strings(&["./a"])));
    assert_eq!(
        parse(&["--skill", "./a", "--skill", "./b"]).skills,
        Some(strings(&["./a", "./b"]))
    );
    assert_eq!(
        parse(&["--prompt-template", "./p"]).prompt_templates,
        Some(strings(&["./p"]))
    );
    assert_eq!(
        parse(&["--prompt-template", "./one", "--prompt-template", "./two"]).prompt_templates,
        Some(strings(&["./one", "./two"]))
    );
    assert_eq!(
        parse(&["--theme", "./t.json"]).themes,
        Some(strings(&["./t.json"]))
    );
    assert_eq!(
        parse(&["--theme", "./dark.json", "--theme", "./light.json"]).themes,
        Some(strings(&["./dark.json", "./light.json"]))
    );
    assert!(parse(&["--no-skills"]).no_skills);
    assert!(parse(&["--no-prompt-templates"]).no_prompt_templates);
    assert!(parse(&["--no-themes"]).no_themes);
    assert!(parse(&["--no-context-files"]).no_context_files);
    assert!(parse(&["-nc"]).no_context_files);
}

#[test]
fn parses_the_project_approval_flags() {
    assert_eq!(parse(&["--approve"]).project_trust_override, Some(true));
    assert_eq!(parse(&["-a"]).project_trust_override, Some(true));
    assert_eq!(parse(&["--no-approve"]).project_trust_override, Some(false));
    assert_eq!(parse(&["-na"]).project_trust_override, Some(false));
}

#[test]
fn parses_the_verbose_and_offline_flags() {
    assert!(parse(&["--verbose"]).verbose);
    assert!(parse(&["--offline"]).offline);
}

#[test]
fn parses_the_tui_mode_flag() {
    assert_eq!(
        parse(&["--tui-mode", "regular"]).tui_mode,
        Some(TuiMode::Regular)
    );
    assert_eq!(
        parse(&["--tui-mode", "fullscreen"]).tui_mode,
        Some(TuiMode::Fullscreen)
    );

    assert_eq!(
        parse(&["--tui-mode", "other"]).diagnostics,
        vec![ArgDiagnostic {
            level: DiagnosticLevel::Error,
            message: "Invalid TUI mode \"other\". Valid values: regular, fullscreen".to_string(),
        }]
    );
    assert_eq!(
        parse(&["--tui-mode"]).diagnostics,
        vec![ArgDiagnostic {
            level: DiagnosticLevel::Error,
            message: "--tui-mode requires regular or fullscreen".to_string(),
        }]
    );

    // The old spelling is not recognized; it lands with the unknown flags.
    let result = parse(&["--ui-mode", "fullscreen"]);
    assert_eq!(result.tui_mode, None);
    assert_eq!(
        result.unknown_flags.get("ui-mode"),
        Some(&UnknownFlagValue::Value("fullscreen".to_string()))
    );
}

#[test]
fn parses_the_tool_flags() {
    assert!(parse(&["--no-tools"]).no_tools);
    assert!(parse(&["-nt"]).no_tools);
    assert!(parse(&["--no-builtin-tools"]).no_builtin_tools);
    assert!(parse(&["-nbt"]).no_builtin_tools);
    assert_eq!(
        parse(&["--tools", "read,bash"]).tools,
        Some(strings(&["read", "bash"]))
    );
    assert_eq!(
        parse(&["-t", "read,bash"]).tools,
        Some(strings(&["read", "bash"]))
    );
    assert_eq!(
        parse(&["--exclude-tools", "read,bash"]).exclude_tools,
        Some(strings(&["read", "bash"]))
    );
    assert_eq!(
        parse(&["-xt", "read,bash"]).exclude_tools,
        Some(strings(&["read", "bash"]))
    );

    let result = parse(&["--no-tools", "--tools", "read,bash"]);
    assert!(result.no_tools);
    assert_eq!(result.tools, Some(strings(&["read", "bash"])));

    let result = parse(&["--no-builtin-tools", "--tools", "read,bash"]);
    assert!(result.no_builtin_tools);
    assert_eq!(result.tools, Some(strings(&["read", "bash"])));
}

#[test]
fn warns_about_an_invalid_thinking_level() {
    let result = parse(&["--thinking", "sideways"]);
    assert_eq!(result.thinking, None);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].level, DiagnosticLevel::Warning);
    assert!(
        result.diagnostics[0]
            .message
            .contains("Invalid thinking level")
    );
}

#[test]
fn parses_the_start_mode_flags_and_refuses_a_contradiction() {
    assert_eq!(parse(&["--auto"]).start_mode, Some(StartMode::Auto));
    assert_eq!(parse(&["--yolo"]).start_mode, Some(StartMode::Yolo));
    // Repeating the same one is not a contradiction.
    let result = parse(&["--auto", "--auto"]);
    assert_eq!(result.start_mode, Some(StartMode::Auto));
    assert!(result.diagnostics.is_empty());

    let result = parse(&["--auto", "--yolo"]);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].level, DiagnosticLevel::Error);
    assert!(
        result.diagnostics[0]
            .message
            .contains("Conflicting start modes")
    );
}

#[test]
fn parses_the_list_models_flag_with_and_without_a_search() {
    assert_eq!(parse(&["--list-models"]).list_models, Some(ListModels::All));
    assert_eq!(
        parse(&["--list-models", "sonnet"]).list_models,
        Some(ListModels::Search("sonnet".to_string()))
    );
    // A flag after it is not the search pattern.
    let result = parse(&["--list-models", "--offline"]);
    assert_eq!(result.list_models, Some(ListModels::All));
    assert!(result.offline);
}

#[test]
fn parses_messages_and_file_arguments() {
    assert_eq!(
        parse(&["hello", "world"]).messages,
        strings(&["hello", "world"])
    );
    assert_eq!(
        parse(&["@README.md", "@src/main.ts"]).file_args,
        strings(&["README.md", "src/main.ts"])
    );
    let result = parse(&["@file.txt", "explain this", "@image.png"]);
    assert_eq!(result.file_args, strings(&["file.txt", "image.png"]));
    assert_eq!(result.messages, strings(&["explain this"]));
}

#[test]
fn collects_unknown_long_flags_in_all_three_shapes() {
    let result = parse(&["--unknown-flag", "message"]);
    assert!(result.messages.is_empty());
    assert_eq!(
        result.unknown_flags.get("unknown-flag"),
        Some(&UnknownFlagValue::Value("message".to_string()))
    );

    assert_eq!(
        parse(&["--unknown-flag"]).unknown_flags.get("unknown-flag"),
        Some(&UnknownFlagValue::Present)
    );
    assert_eq!(
        parse(&["--unknown-flag=value"])
            .unknown_flags
            .get("unknown-flag"),
        Some(&UnknownFlagValue::Value("value".to_string()))
    );
}

#[test]
fn reports_an_unknown_short_flag_immediately() {
    let result = parse(&["-z"]);
    assert_eq!(
        result.diagnostics,
        vec![ArgDiagnostic {
            level: DiagnosticLevel::Error,
            message: "Unknown option: -z".to_string(),
        }]
    );
}

/// left to claim an unknown long flag, so it becomes an error naming all of them.
#[test]
fn unknown_long_flags_become_one_error() {
    assert_eq!(unknown_flags_error(&parse(&["hello"])), None);
    assert_eq!(
        unknown_flags_error(&parse(&["--plan"])).as_deref(),
        Some("Unknown option: --plan")
    );
    assert_eq!(
        unknown_flags_error(&parse(&["--plan", "--sandbox=strict"])).as_deref(),
        Some("Unknown options: --plan, --sandbox")
    );
}

#[test]
fn parses_several_flags_together() {
    let result = parse(&[
        "--provider",
        "anthropic",
        "--model",
        "claude-sonnet",
        "--print",
        "--thinking",
        "high",
        "@prompt.md",
        "Do the task",
    ]);
    assert_eq!(result.provider.as_deref(), Some("anthropic"));
    assert_eq!(result.model.as_deref(), Some("claude-sonnet"));
    assert!(result.print);
    assert_eq!(result.thinking, Some(ThinkingLevel::High));
    assert_eq!(result.file_args, strings(&["prompt.md"]));
    assert_eq!(result.messages, strings(&["Do the task"]));
}

#[test]
fn the_help_text_names_the_environment_variables_and_the_tools() {
    let help = notagent::cli::args::help_text();
    assert!(help.contains("ANTHROPIC_API_KEY"));
    assert!(help.contains("NOTAGENT_CODING_AGENT_DIR"));
    assert!(help.contains("NOTAGENT_CODING_AGENT_SESSION_DIR"));
    assert!(help.contains("Built-in Tool Names:"));
    // The extension flags are gone with the extension system.
    assert!(!help.contains("--extension"));
    assert!(!help.contains("--no-extensions"));
}
