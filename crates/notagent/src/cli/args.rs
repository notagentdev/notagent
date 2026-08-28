use std::collections::BTreeMap;

use notagent_agent::types::ThinkingLevel;

use crate::config::{APP_NAME, USER_CONFIG_DIR_NAME, env_agent_dir, env_session_dir};
use crate::core::settings_manager::TuiMode;

/// `--mode <mode>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Text,
    Json,
    Rpc,
}

impl OutputMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(OutputMode::Text),
            "json" => Some(OutputMode::Json),
            "rpc" => Some(OutputMode::Rpc),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OutputMode::Text => "text",
            OutputMode::Json => "json",
            OutputMode::Rpc => "rpc",
        }
    }
}

/// `--auto` / `--yolo`: start in a mode that suppresses approval prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    Auto,
    Yolo,
}

impl StartMode {
    pub fn as_str(self) -> &'static str {
        match self {
            StartMode::Auto => "auto",
            StartMode::Yolo => "yolo",
        }
    }
}

/// `--list-models [search]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListModels {
    All,
    Search(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgDiagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

impl ArgDiagnostic {
    fn warning(message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            message: message.into(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<Vec<String>>,
    pub thinking: Option<ThinkingLevel>,
    pub continue_session: bool,
    pub resume: bool,
    pub help: bool,
    pub version: bool,
    pub mode: Option<OutputMode>,
    pub name: Option<String>,
    pub no_session: bool,
    pub session: Option<String>,
    pub session_id: Option<String>,
    pub fork: Option<String>,
    pub session_dir: Option<String>,
    pub models: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
    pub no_tools: bool,
    pub no_builtin_tools: bool,
    pub print: bool,
    /// Start in a prompt-suppressing mode. Required when there is no TTY.
    pub start_mode: Option<StartMode>,
    pub export: Option<String>,
    pub no_skills: bool,
    pub skills: Option<Vec<String>>,
    pub prompt_templates: Option<Vec<String>>,
    pub no_prompt_templates: bool,
    pub themes: Option<Vec<String>>,
    pub no_themes: bool,
    pub no_context_files: bool,
    pub list_models: Option<ListModels>,
    pub offline: bool,
    pub tui_mode: Option<TuiMode>,
    pub verbose: bool,
    pub project_trust_override: Option<bool>,
    pub messages: Vec<String>,
    pub file_args: Vec<String>,
    /// Long flags nothing claimed. Kept as a map so the error can name them all.
    pub unknown_flags: BTreeMap<String, UnknownFlagValue>,
    pub diagnostics: Vec<ArgDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownFlagValue {
    Present,
    Value(String),
}

const VALID_THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub fn is_valid_thinking_level(level: &str) -> bool {
    VALID_THINKING_LEVELS.contains(&level)
}

fn parse_thinking_level(level: &str) -> Option<ThinkingLevel> {
    match level {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::Xhigh),
        "max" => Some(ThinkingLevel::Max),
        _ => None,
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|part| part.trim().to_string())
        .collect()
}

fn split_non_empty_list(value: &str) -> Vec<String> {
    split_list(value)
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect()
}

pub fn parse_args(args: &[String]) -> Args {
    let mut result = Args::default();
    let mut index = 0usize;

    while index < args.len() {
        let arg = args[index].as_str();
        let next = |offset: usize| args.get(index + offset).map(String::as_str);

        match arg {
            "--help" | "-h" => result.help = true,
            "--version" | "-v" => result.version = true,
            "--mode" if index + 1 < args.len() => {
                index += 1;
                if let Some(mode) = OutputMode::parse(&args[index]) {
                    result.mode = Some(mode);
                }
            }
            "--continue" | "-c" => result.continue_session = true,
            "--resume" | "-r" => result.resume = true,
            "--provider" if index + 1 < args.len() => {
                index += 1;
                result.provider = Some(args[index].clone());
            }
            "--model" if index + 1 < args.len() => {
                index += 1;
                result.model = Some(args[index].clone());
            }
            "--api-key" if index + 1 < args.len() => {
                index += 1;
                result.api_key = Some(args[index].clone());
            }
            "--system-prompt" if index + 1 < args.len() => {
                index += 1;
                result.system_prompt = Some(args[index].clone());
            }
            "--append-system-prompt" if index + 1 < args.len() => {
                index += 1;
                result
                    .append_system_prompt
                    .get_or_insert_with(Vec::new)
                    .push(args[index].clone());
            }
            "--name" | "-n" => {
                if index + 1 < args.len() {
                    index += 1;
                    result.name = Some(args[index].clone());
                } else {
                    result
                        .diagnostics
                        .push(ArgDiagnostic::error("--name requires a value"));
                }
            }
            "--no-session" => result.no_session = true,
            "--session" if index + 1 < args.len() => {
                index += 1;
                result.session = Some(args[index].clone());
            }
            "--session-id" if index + 1 < args.len() => {
                index += 1;
                result.session_id = Some(args[index].clone());
            }
            "--fork" if index + 1 < args.len() => {
                index += 1;
                result.fork = Some(args[index].clone());
            }
            "--session-dir" if index + 1 < args.len() => {
                index += 1;
                result.session_dir = Some(args[index].clone());
            }
            "--models" if index + 1 < args.len() => {
                index += 1;
                result.models = Some(split_list(&args[index]));
            }
            "--no-tools" | "-nt" => result.no_tools = true,
            "--no-builtin-tools" | "-nbt" => result.no_builtin_tools = true,
            "--tools" | "-t" if index + 1 < args.len() => {
                index += 1;
                result.tools = Some(split_non_empty_list(&args[index]));
            }
            "--exclude-tools" | "-xt" if index + 1 < args.len() => {
                index += 1;
                result.exclude_tools = Some(split_non_empty_list(&args[index]));
            }
            "--thinking" if index + 1 < args.len() => {
                index += 1;
                let level = args[index].clone();
                match parse_thinking_level(&level) {
                    Some(level) => result.thinking = Some(level),
                    None => result.diagnostics.push(ArgDiagnostic::warning(format!(
                        "Invalid thinking level \"{level}\". Valid values: {}",
                        VALID_THINKING_LEVELS.join(", ")
                    ))),
                }
            }
            "--auto" | "--yolo" => {
                let requested = if arg == "--auto" {
                    StartMode::Auto
                } else {
                    StartMode::Yolo
                };
                // A second, different flag is a contradiction the caller must
                // resolve. Silently letting the last one win is the wrong
                // kindness for a flag that decides how much runs unsupervised.
                match result.start_mode {
                    Some(existing) if existing != requested => {
                        result.diagnostics.push(ArgDiagnostic::error(format!(
                            "Conflicting start modes: --{} and {arg}. Pass only one.",
                            existing.as_str()
                        )));
                    }
                    _ => result.start_mode = Some(requested),
                }
            }
            "--print" | "-p" => {
                result.print = true;
                // The prompt may follow directly. `---` is not a flag: it is how
                // a message beginning with YAML frontmatter starts.
                if let Some(value) = next(1)
                    && !value.starts_with('@')
                    && (!value.starts_with('-') || value.starts_with("---"))
                {
                    result.messages.push(value.to_string());
                    index += 1;
                }
            }
            "--export" if index + 1 < args.len() => {
                index += 1;
                result.export = Some(args[index].clone());
            }
            "--skill" if index + 1 < args.len() => {
                index += 1;
                result
                    .skills
                    .get_or_insert_with(Vec::new)
                    .push(args[index].clone());
            }
            "--prompt-template" if index + 1 < args.len() => {
                index += 1;
                result
                    .prompt_templates
                    .get_or_insert_with(Vec::new)
                    .push(args[index].clone());
            }
            "--theme" if index + 1 < args.len() => {
                index += 1;
                result
                    .themes
                    .get_or_insert_with(Vec::new)
                    .push(args[index].clone());
            }
            "--no-skills" | "-ns" => result.no_skills = true,
            "--no-prompt-templates" | "-np" => result.no_prompt_templates = true,
            "--no-themes" => result.no_themes = true,
            "--no-context-files" | "-nc" => result.no_context_files = true,
            "--list-models" => {
                // A following word that is neither a flag nor a file argument is
                // the search pattern.
                match next(1) {
                    Some(value) if !value.starts_with('-') && !value.starts_with('@') => {
                        index += 1;
                        result.list_models = Some(ListModels::Search(value.to_string()));
                    }
                    _ => result.list_models = Some(ListModels::All),
                }
            }
            "--tui-mode" => match next(1) {
                Some("regular") => {
                    result.tui_mode = Some(TuiMode::Regular);
                    index += 1;
                }
                Some("fullscreen") => {
                    result.tui_mode = Some(TuiMode::Fullscreen);
                    index += 1;
                }
                None => result.diagnostics.push(ArgDiagnostic::error(
                    "--tui-mode requires regular or fullscreen",
                )),
                Some(mode) if mode.starts_with('-') => result.diagnostics.push(
                    ArgDiagnostic::error("--tui-mode requires regular or fullscreen"),
                ),
                Some(mode) => {
                    let mode = mode.to_string();
                    index += 1;
                    result.diagnostics.push(ArgDiagnostic::error(format!(
                        "Invalid TUI mode \"{mode}\". Valid values: regular, fullscreen"
                    )));
                }
            },
            "--verbose" => result.verbose = true,
            "--approve" | "-a" => result.project_trust_override = Some(true),
            "--no-approve" | "-na" => result.project_trust_override = Some(false),
            "--offline" => result.offline = true,
            _ if arg.starts_with('@') => result.file_args.push(arg[1..].to_string()),
            _ if arg.starts_with("--") => match arg.find('=') {
                Some(equals) => {
                    result.unknown_flags.insert(
                        arg[2..equals].to_string(),
                        UnknownFlagValue::Value(arg[equals + 1..].to_string()),
                    );
                }
                None => {
                    let name = arg[2..].to_string();
                    match next(1) {
                        Some(value) if !value.starts_with('-') && !value.starts_with('@') => {
                            result
                                .unknown_flags
                                .insert(name, UnknownFlagValue::Value(value.to_string()));
                            index += 1;
                        }
                        _ => {
                            result.unknown_flags.insert(name, UnknownFlagValue::Present);
                        }
                    }
                }
            },
            _ if arg.starts_with('-') => result
                .diagnostics
                .push(ArgDiagnostic::error(format!("Unknown option: {arg}"))),
            _ => result.messages.push(arg.to_string()),
        }

        index += 1;
    }

    result
}

/// The error for flags nothing claimed.
/// errors on the ones no extension registered. With the extension system gone
/// every unknown flag reaches this, which is why parsing keeps them instead of
/// erroring inline — the message names all of them at once, as it did before.
pub fn unknown_flags_error(args: &Args) -> Option<String> {
    if args.unknown_flags.is_empty() {
        return None;
    }
    let names: Vec<String> = args
        .unknown_flags
        .keys()
        .map(|name| format!("--{name}"))
        .collect();
    Some(format!(
        "Unknown option{}: {}",
        if names.len() == 1 { "" } else { "s" },
        names.join(", ")
    ))
}

/// The `--help` text.
pub fn help_text() -> String {
    let agent_dir_env = format!("{:<32}", env_agent_dir());
    let session_dir_env = format!("{:<32}", env_session_dir());
    format!(
        r#"{APP_NAME} - AI coding assistant with read, bash, edit, write tools

Usage:
  {APP_NAME} [options] [@files...] [messages...]

Commands:
  {APP_NAME} install <source> [-l]     Install a package source and add it to settings
  {APP_NAME} remove <source> [-l]      Remove a package source from settings
  {APP_NAME} uninstall <source> [-l]   Alias for remove
  {APP_NAME} update [source|self|notagent]   Update notagent, packages, or model catalogs
  {APP_NAME} list                      List installed packages from settings
  {APP_NAME} config [-l]               Open TUI to enable/disable package resources (Tab switches scope)
  {APP_NAME} auth <command>            Print credentials or check provider readiness
  {APP_NAME} <command> --help          Show help for install/remove/uninstall/update/list/config/auth

Options:
  --provider <name>              Provider name (default: google)
  --model <pattern>              Model pattern or ID (supports "provider/id" and optional ":<thinking>")
  --api-key <key>                API key (defaults to env vars)
  --system-prompt <text>         System prompt (default: coding assistant prompt)
  --append-system-prompt <text>  Append text or file contents to the system prompt (can be used multiple times)
  --mode <mode>                  Output mode: text (default), json, or rpc
  --auto                         Start in auto mode: tool use is approved automatically
  --yolo                         Start in yolo mode: nothing is confirmed
  --print, -p                    Non-interactive mode: process prompt and exit
  --continue, -c                 Continue previous session
  --resume, -r                   Select a session to resume
  --session <path|id>            Use specific session file or partial UUID
  --session-id <id>              Use exact project session ID, creating it if missing
  --fork <path|id>               Fork specific session file or partial UUID into a new session
  --session-dir <dir>            Directory for session storage and lookup
  --no-session                   Don't save session (ephemeral)
  --name, -n <name>              Set session display name
  --models <patterns>            Comma-separated model patterns for Ctrl+P cycling
                                 Supports globs (anthropic/*, *sonnet*) and fuzzy matching
  --no-tools, -nt                Disable all tools by default
  --no-builtin-tools, -nbt       Disable built-in tools by default
  --tools, -t <tools>            Comma-separated allowlist of tool names to enable
  --exclude-tools, -xt <tools>   Comma-separated denylist of tool names to disable
  --thinking <level>             Set thinking level: off, minimal, low, medium, high, xhigh, max
  --skill <path>                 Load a skill file or directory (can be used multiple times)
  --no-skills, -ns               Disable skills discovery and loading
  --prompt-template <path>       Load a prompt template file or directory (can be used multiple times)
  --no-prompt-templates, -np     Disable prompt template discovery and loading
  --theme <path>                 Load a theme file or directory (can be used multiple times)
  --no-themes                    Disable theme discovery and loading
  --no-context-files, -nc        Disable AGENTS.md and CLAUDE.md discovery and loading
  --export <file>                Export session file to HTML and exit
  --list-models [search]         List available models (with optional fuzzy search)
  --verbose                      Force verbose startup (overrides quietStartup setting)
  --tui-mode <mode>              TUI mode: regular (default) or fullscreen
  --approve, -a                  Trust project-local files for this run
  --no-approve, -na              Ignore project-local files for this run
  --offline                      Disable startup network operations (same as NOTAGENT_OFFLINE=1)
  --help, -h                     Show this help
  --version, -v                  Show version number

Examples:
  # Print a provider API key for an external client
  {APP_NAME} auth print-api-key --provider openai

  # Print an OAuth bearer token for an external client (refreshes if expired)
  {APP_NAME} auth print-bearer-token --provider openai-codex

  # Interactive mode
  {APP_NAME}

  # Interactive mode with initial prompt
  {APP_NAME} "List all .ts files in src/"

  # Include files in initial message
  {APP_NAME} @prompt.md @image.png "What color is the sky?"

  # Non-interactive mode (process and exit)
  {APP_NAME} -p "List all .ts files in src/"

  # Multiple messages (interactive)
  {APP_NAME} "Read package.json" "What dependencies do we have?"

  # Continue previous session
  {APP_NAME} --continue "What did we discuss?"

  # Start a named session
  {APP_NAME} --name "Refactor auth module"

  # Use different model
  {APP_NAME} --provider openai --model gpt-4o-mini "Help me refactor this code"

  # Use model with provider prefix (no --provider needed)
  {APP_NAME} --model openai/gpt-4o "Help me refactor this code"

  # Use model with thinking level shorthand
  {APP_NAME} --model sonnet:high "Solve this complex problem"

  # Limit model cycling to specific models
  {APP_NAME} --models claude-sonnet,claude-haiku,gpt-4o

  # Limit to a specific provider with glob pattern
  {APP_NAME} --models "github-copilot/*"

  # Cycle models with fixed thinking levels
  {APP_NAME} --models sonnet:high,haiku:low

  # Start with a specific thinking level
  {APP_NAME} --thinking high "Solve this complex problem"

  # Read-only mode (no file modifications possible)
  {APP_NAME} --tools read,grep,find,ls -p "Review the code in src/"

  # Disable one tool while keeping the rest available
  {APP_NAME} --exclude-tools ask_question

  # Export a session file to HTML
  {APP_NAME} --export ~/{USER_CONFIG_DIR_NAME}/agent/sessions/--path--/session.jsonl
  {APP_NAME} --export session.jsonl output.html

Environment Variables:
  ANTHROPIC_AUTH_TOKEN             - Anthropic bearer auth token
  ANTHROPIC_API_KEY                - Anthropic Claude API key
  ANTHROPIC_OAUTH_TOKEN            - Anthropic OAuth token (alternative to API key)
  ANT_LING_API_KEY                 - Ant Ling API key
  OPENAI_API_KEY                   - OpenAI GPT API key
  AZURE_OPENAI_API_KEY             - Azure OpenAI API key
  AZURE_OPENAI_BASE_URL            - Azure OpenAI/Cognitive Services base URL (e.g. https://{{resource}}.openai.azure.com)
  AZURE_OPENAI_RESOURCE_NAME       - Azure OpenAI resource name (alternative to base URL)
  AZURE_OPENAI_API_VERSION         - Azure OpenAI API version (default: v1)
  AZURE_OPENAI_DEPLOYMENT_NAME_MAP - Azure OpenAI model=deployment map (comma-separated)
  DEEPSEEK_API_KEY                 - DeepSeek API key
  NVIDIA_API_KEY                   - NVIDIA NIM API key
  GEMINI_API_KEY                   - Google Gemini API key
  GROQ_API_KEY                     - Groq API key
  CEREBRAS_API_KEY                 - Cerebras API key
  XAI_API_KEY                      - xAI Grok API key
  FIREWORKS_API_KEY                - Fireworks API key
  TOGETHER_API_KEY                 - Together AI API key
  BASETEN_API_KEY                  - Baseten API key
  OPENROUTER_API_KEY               - OpenRouter API key
  AI_GATEWAY_API_KEY               - Vercel AI Gateway API key
  ZAI_API_KEY                      - ZAI Coding Plan API key (Global)
  ZAI_CODING_CN_API_KEY            - ZAI Coding Plan API key (China)
  MISTRAL_API_KEY                  - Mistral API key
  MINIMAX_API_KEY                  - MiniMax API key
  MOONSHOT_API_KEY                 - Moonshot AI API key
  OPENCODE_API_KEY                 - OpenCode Zen/OpenCode Go API key
  KIMI_API_KEY                     - Kimi For Coding API key
  CLOUDFLARE_API_KEY               - Cloudflare API token (Workers AI and AI Gateway)
  CLOUDFLARE_ACCOUNT_ID            - Cloudflare account id (required for both)
  CLOUDFLARE_GATEWAY_ID            - Cloudflare AI Gateway slug (required for AI Gateway)
  QWEN_TOKEN_PLAN_API_KEY          - Qwen Token Plan API key (international region)
  QWEN_TOKEN_PLAN_CN_API_KEY       - Qwen Token Plan API key (China region)
  XIAOMI_API_KEY                   - Xiaomi MiMo API key (api.xiaomimimo.com billing)
  XIAOMI_TOKEN_PLAN_CN_API_KEY     - Xiaomi MiMo Token Plan API key (China region)
  XIAOMI_TOKEN_PLAN_AMS_API_KEY    - Xiaomi MiMo Token Plan API key (Amsterdam region)
  XIAOMI_TOKEN_PLAN_SGP_API_KEY    - Xiaomi MiMo Token Plan API key (Singapore region)
  AWS_PROFILE                      - AWS profile for Amazon Bedrock
  AWS_ACCESS_KEY_ID                - AWS access key for Amazon Bedrock
  AWS_SECRET_ACCESS_KEY            - AWS secret key for Amazon Bedrock
  AWS_BEARER_TOKEN_BEDROCK         - Bedrock API key (bearer token)
  AWS_REGION                       - AWS region for Amazon Bedrock (e.g., us-east-1)
  {agent_dir_env} - Config directory (default: ~/{USER_CONFIG_DIR_NAME}/agent)
  {session_dir_env} - Session storage directory (overridden by --session-dir)
  NOTAGENT_PACKAGE_DIR                   - Override package directory (for Nix/Guix store paths)
  NOTAGENT_OFFLINE                       - Disable startup network operations when set to 1/true/yes
  NOTAGENT_TELEMETRY                     - Override install telemetry when set to 1/true/yes or 0/false/no
  NOTAGENT_SHARE_VIEWER_URL              - Base URL for /share command (default: https://notagent.dev/session/)

Built-in Tool Names:
  read   - Read file contents
  bash   - Execute bash commands
  edit   - Edit files with find/replace
  write  - Write files (creates/overwrites)
  grep   - Search file contents (read-only, off by default)
  find   - Find files by glob pattern (read-only, off by default)
  ls     - List directory contents (read-only, off by default)
"#
    )
}
