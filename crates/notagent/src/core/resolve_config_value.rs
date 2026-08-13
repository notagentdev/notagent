//! Port of `packages/coding-agent/src/core/resolve-config-value.ts`.
//!
//! Resolve configuration values that may be shell commands, environment
//! variables or literals. Used by auth storage and the model registry.

use std::collections::{BTreeMap, HashMap};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

#[cfg(any(windows, test))]
use crate::utils::shell::{CommandTransport, get_shell_config};

/// Cache for shell command results (persists for the process lifetime).
static COMMAND_RESULT_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplatePart {
    Literal(String),
    Env(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigValueReference {
    Command(String),
    Template(Vec<TemplatePart>),
}

fn is_env_var_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Longest prefix of `value` that is a valid environment variable name.
fn env_var_name_prefix(value: &str) -> Option<&str> {
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let valid = if index == 0 {
            character.is_ascii_alphabetic() || character == '_'
        } else {
            character.is_ascii_alphanumeric() || character == '_'
        };
        if !valid {
            break;
        }
        end = index + character.len_utf8();
    }
    if end == 0 { None } else { Some(&value[..end]) }
}

fn append_literal(parts: &mut Vec<TemplatePart>, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(TemplatePart::Literal(previous)) = parts.last_mut() {
        previous.push_str(value);
        return;
    }
    parts.push(TemplatePart::Literal(value.to_owned()));
}

fn parse_config_value_template(config: &str) -> Vec<TemplatePart> {
    let mut parts: Vec<TemplatePart> = Vec::new();
    let mut index = 0usize;

    while index < config.len() {
        let Some(relative) = config[index..].find('$') else {
            append_literal(&mut parts, &config[index..]);
            break;
        };
        let dollar_index = index + relative;
        append_literal(&mut parts, &config[index..dollar_index]);
        let rest = &config[dollar_index + 1..];
        let next_char = rest.chars().next();

        match next_char {
            Some('$') | Some('!') => {
                let character = next_char.expect("matched");
                append_literal(&mut parts, &character.to_string());
                index = dollar_index + 1 + character.len_utf8();
            }
            Some('{') => {
                let after_brace = &config[dollar_index + 2..];
                match after_brace.find('}') {
                    None => {
                        append_literal(&mut parts, "$");
                        index = dollar_index + 1;
                    }
                    Some(relative_end) => {
                        let end_index = dollar_index + 2 + relative_end;
                        let name = &config[dollar_index + 2..end_index];
                        if is_env_var_name(name) {
                            parts.push(TemplatePart::Env(name.to_owned()));
                        } else {
                            append_literal(&mut parts, &config[dollar_index..=end_index]);
                        }
                        index = end_index + 1;
                    }
                }
            }
            _ => match env_var_name_prefix(rest) {
                Some(name) => {
                    let length = name.len();
                    parts.push(TemplatePart::Env(name.to_owned()));
                    index = dollar_index + 1 + length;
                }
                None => {
                    append_literal(&mut parts, "$");
                    index = dollar_index + 1;
                }
            },
        }
    }

    parts
}

fn parse_config_value_reference(config: &str) -> ConfigValueReference {
    if config.starts_with('!') {
        return ConfigValueReference::Command(config.to_owned());
    }
    ConfigValueReference::Template(parse_config_value_template(config))
}

fn resolve_env_config_value(name: &str, env: Option<&BTreeMap<String, String>>) -> Option<String> {
    // TS uses `||`, so an empty override falls through to the process env.
    if let Some(value) = env
        .and_then(|env| env.get(name))
        .filter(|value| !value.is_empty())
    {
        return Some(value.clone());
    }
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn template_env_var_names(parts: &[TemplatePart]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in parts {
        if let TemplatePart::Env(name) = part
            && !names.contains(name)
        {
            names.push(name.clone());
        }
    }
    names
}

fn resolve_template(
    parts: &[TemplatePart],
    env: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    let mut resolved = String::new();
    for part in parts {
        match part {
            TemplatePart::Literal(value) => resolved.push_str(value),
            TemplatePart::Env(name) => resolved.push_str(&resolve_env_config_value(name, env)?),
        }
    }
    Some(resolved)
}

/// The single environment variable a config value consists of, if any.
pub fn get_config_value_env_var_name(config: &str) -> Option<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Template(parts) if parts.len() == 1 => match &parts[0] {
            TemplatePart::Env(name) => Some(name.clone()),
            TemplatePart::Literal(_) => None,
        },
        _ => None,
    }
}

pub fn get_config_value_env_var_names(config: &str) -> Vec<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Template(parts) => template_env_var_names(&parts),
        ConfigValueReference::Command(_) => Vec::new(),
    }
}

pub fn get_missing_config_value_env_var_names(
    config: &str,
    env: Option<&BTreeMap<String, String>>,
) -> Vec<String> {
    get_config_value_env_var_names(config)
        .into_iter()
        .filter(|name| resolve_env_config_value(name, env).is_none())
        .collect()
}

pub fn is_command_config_value(config: &str) -> bool {
    matches!(
        parse_config_value_reference(config),
        ConfigValueReference::Command(_)
    )
}

pub fn is_config_value_configured(config: &str, env: Option<&BTreeMap<String, String>>) -> bool {
    get_missing_config_value_env_var_names(config, env).is_empty()
}

/// Resolve a config value (API key, header value, …) to an actual value.
///
/// - `!command` executes the rest as a shell command and uses stdout (cached)
/// - `$ENV_VAR` and `${ENV_VAR}` interpolate the named environment variable
/// - in non-command values `$$` escapes a literal `$` and `$!` a literal `!`
/// - anything else is a literal
pub fn resolve_config_value(
    config: &str,
    env: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Command(command) => execute_command(&command),
        ConfigValueReference::Template(parts) => resolve_template(&parts, env),
    }
}

pub fn resolve_config_value_uncached(
    config: &str,
    env: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    match parse_config_value_reference(config) {
        ConfigValueReference::Command(command) => execute_command_uncached(&command),
        ConfigValueReference::Template(parts) => resolve_template(&parts, env),
    }
}

pub fn resolve_config_value_or_throw(
    config: &str,
    description: &str,
    env: Option<&BTreeMap<String, String>>,
) -> Result<String, String> {
    if let Some(resolved) = resolve_config_value_uncached(config, env) {
        return Ok(resolved);
    }
    match parse_config_value_reference(config) {
        ConfigValueReference::Command(command) => Err(format!(
            "Failed to resolve {description} from shell command: {}",
            &command[1..]
        )),
        ConfigValueReference::Template(_) => {
            let missing = get_missing_config_value_env_var_names(config, env);
            match missing.len() {
                1 => Err(format!(
                    "Failed to resolve {description} from environment variable: {}",
                    missing[0]
                )),
                count if count > 1 => Err(format!(
                    "Failed to resolve {description} from environment variables: {}",
                    missing.join(", ")
                )),
                _ => Err(format!("Failed to resolve {description}")),
            }
        }
    }
}

/// Resolve all header values with the same logic as API keys.
pub fn resolve_headers(
    headers: Option<&BTreeMap<String, String>>,
    env: Option<&BTreeMap<String, String>>,
) -> Option<BTreeMap<String, String>> {
    let headers = headers?;
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in headers {
        // TS keeps only truthy values, so an empty result drops the header.
        if let Some(value) = resolve_config_value(value, env).filter(|value| !value.is_empty()) {
            resolved.insert(key.clone(), value);
        }
    }
    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

pub fn resolve_headers_or_throw(
    headers: Option<&BTreeMap<String, String>>,
    description: &str,
    env: Option<&BTreeMap<String, String>>,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let Some(headers) = headers else {
        return Ok(None);
    };
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in headers {
        let value =
            resolve_config_value_or_throw(value, &format!("{description} header \"{key}\""), env)?;
        resolved.insert(key.clone(), value);
    }
    Ok(if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    })
}

/// Clear the config value command cache. Exported for testing.
pub fn clear_config_value_cache() {
    COMMAND_RESULT_CACHE
        .lock()
        .expect("command cache mutex")
        .clear();
}

fn execute_command(command_config: &str) -> Option<String> {
    if let Some(cached) = COMMAND_RESULT_CACHE
        .lock()
        .expect("command cache mutex")
        .get(command_config)
    {
        return cached.clone();
    }
    let result = execute_command_uncached(command_config);
    COMMAND_RESULT_CACHE
        .lock()
        .expect("command cache mutex")
        .insert(command_config.to_owned(), result.clone());
    result
}

fn execute_command_uncached(command_config: &str) -> Option<String> {
    let command = &command_config[1..];
    #[cfg(windows)]
    {
        let configured = execute_with_configured_shell(command);
        if configured.executed {
            return configured.value;
        }
        execute_with_default_shell(command)
    }
    #[cfg(not(windows))]
    {
        execute_with_default_shell(command)
    }
}

#[cfg(any(windows, test))]
struct ConfiguredShellResult {
    executed: bool,
    value: Option<String>,
}

/// TS only takes this path on Windows, where `execSync`'s `cmd.exe` cannot run
/// the configured bash. Compiled in tests everywhere so it stays covered.
#[cfg(any(windows, test))]
fn execute_with_configured_shell(command: &str) -> ConfiguredShellResult {
    let Ok(config) = get_shell_config(None) else {
        return ConfiguredShellResult {
            executed: false,
            value: None,
        };
    };
    use std::io::Write;
    let command_from_stdin = config.command_transport == Some(CommandTransport::Stdin);
    let mut process = Command::new(&config.shell);
    process.args(&config.args);
    if !command_from_stdin {
        process.arg(command);
    }
    process
        .stdin(if command_from_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = match process.spawn() {
        Ok(child) => child,
        // A missing shell is TS's ENOENT: not executed, so the caller falls back.
        Err(_) => {
            return ConfiguredShellResult {
                executed: false,
                value: None,
            };
        }
    };
    if command_from_stdin && let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(command.as_bytes());
    }
    match wait_with_timeout(child) {
        Some((status, stdout)) if status.success() => {
            let value = stdout.trim().to_owned();
            ConfiguredShellResult {
                executed: true,
                value: (!value.is_empty()).then_some(value),
            }
        }
        Some(_) | None => ConfiguredShellResult {
            executed: true,
            value: None,
        },
    }
}

fn execute_with_default_shell(command: &str) -> Option<String> {
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("/bin/sh", "-c")
    };
    let child = Command::new(shell)
        .arg(flag)
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let (status, stdout) = wait_with_timeout(child)?;
    if !status.success() {
        return None;
    }
    let value = stdout.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// Port of the `timeout: 10000` option: kill the child when it overruns.
fn wait_with_timeout(mut child: std::process::Child) -> Option<(std::process::ExitStatus, String)> {
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut stdout) = stdout {
            use std::io::Read;
            let _ = stdout.read_to_string(&mut buffer);
        }
        buffer
    });

    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break None,
        }
    };
    let output = reader.join().unwrap_or_default();
    status.map(|status| (status, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn treats_plain_values_as_literals() {
        assert_eq!(
            resolve_config_value("sk-literal", None),
            Some("sk-literal".to_owned())
        );
        assert_eq!(resolve_config_value("", None), Some(String::new()));
    }

    #[test]
    fn interpolates_environment_variables_in_both_syntaxes() {
        let env = env(&[("TOKEN", "abc"), ("SUFFIX", "xyz")]);
        assert_eq!(
            resolve_config_value("$TOKEN", Some(&env)),
            Some("abc".to_owned())
        );
        assert_eq!(
            resolve_config_value("${TOKEN}", Some(&env)),
            Some("abc".to_owned())
        );
        assert_eq!(
            resolve_config_value("pre-$TOKEN-$SUFFIX", Some(&env)),
            Some("pre-abc-xyz".to_owned())
        );
        assert_eq!(
            resolve_config_value("${TOKEN}${SUFFIX}", Some(&env)),
            Some("abcxyz".to_owned())
        );
    }

    #[test]
    fn returns_none_when_a_referenced_variable_is_missing() {
        let env = env(&[("TOKEN", "abc")]);
        assert_eq!(
            resolve_config_value("$TOKEN/$NOTAGENT_MISSING_TEST_VAR", Some(&env)),
            None
        );
        assert_eq!(
            get_missing_config_value_env_var_names("$TOKEN/$NOTAGENT_MISSING_TEST_VAR", Some(&env)),
            vec!["NOTAGENT_MISSING_TEST_VAR".to_owned()]
        );
        assert!(!is_config_value_configured(
            "$NOTAGENT_MISSING_TEST_VAR",
            Some(&env)
        ));
        assert!(is_config_value_configured("$TOKEN", Some(&env)));
    }

    #[test]
    fn escapes_dollar_and_bang() {
        assert_eq!(
            resolve_config_value("$$HOME", None),
            Some("$HOME".to_owned())
        );
        assert_eq!(
            resolve_config_value("$!echo hi", None),
            Some("!echo hi".to_owned())
        );
        // A lone dollar that starts no valid name stays literal.
        assert_eq!(
            resolve_config_value("100$ and 5$", None),
            Some("100$ and 5$".to_owned())
        );
        // An unterminated brace stays literal too.
        assert_eq!(
            resolve_config_value("${UNTERMINATED", None),
            Some("${UNTERMINATED".to_owned())
        );
        // An invalid name inside braces is kept verbatim.
        assert_eq!(
            resolve_config_value("${not-a-name}", None),
            Some("${not-a-name}".to_owned())
        );
    }

    #[test]
    fn reports_the_single_environment_variable_of_a_value() {
        assert_eq!(
            get_config_value_env_var_name("$TOKEN"),
            Some("TOKEN".to_owned())
        );
        assert_eq!(
            get_config_value_env_var_name("${TOKEN}"),
            Some("TOKEN".to_owned())
        );
        assert_eq!(get_config_value_env_var_name("pre-$TOKEN"), None);
        assert_eq!(get_config_value_env_var_name("!echo hi"), None);
        assert_eq!(
            get_config_value_env_var_names("$A-$B-$A"),
            vec!["A".to_owned(), "B".to_owned()]
        );
    }

    #[test]
    fn executes_shell_commands_and_caches_them() {
        clear_config_value_cache();
        assert!(is_command_config_value("!echo hello"));
        assert_eq!(
            resolve_config_value("!echo hello", None),
            Some("hello".to_owned())
        );
        // Trailing whitespace is trimmed and empty output becomes None.
        assert_eq!(
            resolve_config_value_uncached("!printf '  spaced  '", None),
            Some("spaced".to_owned())
        );
        assert_eq!(resolve_config_value_uncached("!true", None), None);
        // A failing command yields no value.
        assert_eq!(resolve_config_value_uncached("!exit 3", None), None);
        clear_config_value_cache();
    }

    #[test]
    fn caches_command_results_for_the_process_lifetime() {
        clear_config_value_cache();
        let directory = tempfile::Builder::new()
            .prefix("notagent-config-value-")
            .tempdir()
            .expect("temp dir");
        let marker = directory.path().join("count");
        let command = format!(
            "!printf x >> {} && cat {}",
            marker.display(),
            marker.display()
        );
        let first = resolve_config_value(&command, None);
        let second = resolve_config_value(&command, None);
        assert_eq!(first, second);
        assert_eq!(
            first,
            Some("x".to_owned()),
            "the cached call must not run the command again"
        );
        clear_config_value_cache();
    }

    #[test]
    fn the_configured_shell_path_runs_commands_and_reports_failures() {
        let result = execute_with_configured_shell("echo configured");
        assert!(result.executed);
        assert_eq!(result.value, Some("configured".to_owned()));
        let result = execute_with_configured_shell("exit 7");
        assert!(result.executed, "a non-zero exit still counts as executed");
        assert_eq!(result.value, None);
    }

    #[test]
    fn or_throw_names_the_missing_source() {
        let error = resolve_config_value_or_throw("$NOTAGENT_MISSING_TEST_VAR", "API key", None)
            .expect_err("fails");
        assert_eq!(
            error,
            "Failed to resolve API key from environment variable: NOTAGENT_MISSING_TEST_VAR"
        );
        let error = resolve_config_value_or_throw(
            "$NOTAGENT_MISSING_A/$NOTAGENT_MISSING_B",
            "API key",
            None,
        )
        .expect_err("fails");
        assert_eq!(
            error,
            "Failed to resolve API key from environment variables: NOTAGENT_MISSING_A, NOTAGENT_MISSING_B"
        );
        let error = resolve_config_value_or_throw("!exit 1", "API key", None).expect_err("fails");
        assert_eq!(
            error,
            "Failed to resolve API key from shell command: exit 1"
        );
    }

    #[test]
    fn resolves_headers_and_drops_unresolvable_ones() {
        let overrides = env(&[("TOKEN", "abc")]);
        let headers = env(&[
            ("Authorization", "Bearer $TOKEN"),
            ("X-Missing", "$NOTAGENT_MISSING_TEST_VAR"),
        ]);
        let resolved = resolve_headers(Some(&headers), Some(&overrides)).expect("headers");
        assert_eq!(
            resolved.get("Authorization"),
            Some(&"Bearer abc".to_owned())
        );
        assert!(!resolved.contains_key("X-Missing"));

        let error = resolve_headers_or_throw(Some(&headers), "provider", Some(&overrides))
            .expect_err("fails");
        assert_eq!(
            error,
            "Failed to resolve provider header \"X-Missing\" from environment variable: NOTAGENT_MISSING_TEST_VAR"
        );
        assert_eq!(resolve_headers(None, None), None);
    }
}
