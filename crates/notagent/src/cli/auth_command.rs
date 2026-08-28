use std::sync::OnceLock;

use notagent_ai::auth::types::AuthResult;
use regex::Regex;

use crate::cli::args::Args;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthCommandKind {
    Check,
    ApiKey,
    BearerToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCommand {
    pub kind: AuthCommandKind,
    pub args: Vec<String>,
    pub json: bool,
    pub credentials: bool,
    pub no_refresh: bool,
    pub min_expiry_ms: Option<u64>,
}

/// Every failure of the auth commands, as one message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AuthCommandError(pub String);

impl AuthCommandError {
    fn new(message: impl Into<String>) -> Self {
        AuthCommandError(message.into())
    }
}

pub fn get_auth_command_name(kind: AuthCommandKind) -> &'static str {
    match kind {
        AuthCommandKind::Check => "auth check",
        AuthCommandKind::ApiKey => "auth print-api-key",
        AuthCommandKind::BearerToken => "auth print-bearer-token",
    }
}

pub fn get_auth_command_usage(kind: AuthCommandKind) -> &'static str {
    match kind {
        AuthCommandKind::Check => {
            "notagent auth check --provider <provider> [--json] [--credentials] [--no-refresh]"
        }
        AuthCommandKind::ApiKey => {
            "notagent auth print-api-key --provider <provider> [--model <model>]"
        }
        AuthCommandKind::BearerToken => {
            "notagent auth print-bearer-token --provider <provider> [--model <model>] [--min-expiry <duration>]"
        }
    }
}

pub fn is_auth_command_help(args: &[String]) -> bool {
    args.first().map(String::as_str) == Some("auth")
        && (args.get(1).is_none()
            || args.get(1).map(String::as_str) == Some("help")
            || args.iter().any(|arg| arg == "--help" || arg == "-h"))
}

pub fn auth_command_help_text() -> &'static str {
    "Usage:\n  notagent auth print-api-key [--provider <provider>] [--model <model>]\n  notagent auth print-bearer-token [--provider <provider>] [--model <model>] [--min-expiry <duration>]\n  notagent auth check [--provider <provider>] [--model <model>] [--json] [--credentials] [--no-refresh]\n\nAuth commands require at least one of --provider or --model. Checks refresh expired OAuth credentials by default; --no-refresh prevents this. --credentials emits the credential, or includes it in JSON output."
}

pub fn print_auth_command_help() {
    crate::core::output_guard::console_log(auth_command_help_text());
}

/// Parses a duration such as `30m` or `1h` into milliseconds.
fn parse_min_expiry(value: Option<&str>) -> Option<u64> {
    let value = value?;
    let digits_end = value.find(|character: char| !character.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let amount: u64 = value[..digits_end].parse().ok()?;
    let unit = value[digits_end..].to_ascii_lowercase();
    let factor = match unit.as_str() {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    };
    Some(amount * factor)
}

/// `None` means "not an auth command"; the error means "an auth command that
/// cannot run".
pub fn parse_auth_command(args: &[String]) -> Result<Option<AuthCommand>, AuthCommandError> {
    if args.first().map(String::as_str) != Some("auth") {
        return Ok(None);
    }

    let kind = match args.get(1).map(String::as_str) {
        Some("check") => AuthCommandKind::Check,
        Some("print-api-key") => AuthCommandKind::ApiKey,
        Some("print-bearer-token") => AuthCommandKind::BearerToken,
        other => {
            return Err(AuthCommandError::new(format!(
                "Unknown auth command \"{}\". Use \"notagent auth print-api-key\", \"notagent auth print-bearer-token\", or \"notagent auth check\".",
                other.unwrap_or("")
            )));
        }
    };

    let mut command_args: Vec<String> = Vec::new();
    let mut json = false;
    let mut credentials = false;
    let mut no_refresh = false;
    let mut min_expiry_ms: Option<u64> = None;
    let mut index = 2usize;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--min-expiry" {
            if kind != AuthCommandKind::BearerToken {
                return Err(AuthCommandError::new(
                    "--min-expiry is only supported by print-bearer-token",
                ));
            }
            index += 1;
            let value = args.get(index).map(String::as_str);
            match parse_min_expiry(value) {
                Some(milliseconds) => min_expiry_ms = Some(milliseconds),
                None => {
                    return Err(AuthCommandError::new(
                        "--min-expiry must use a duration such as 30m or 1h",
                    ));
                }
            }
            index += 1;
            continue;
        }
        if arg == "--json" || arg == "--credentials" || arg == "--no-refresh" {
            if kind != AuthCommandKind::Check {
                return Err(AuthCommandError::new(format!(
                    "{arg} is only supported by auth check"
                )));
            }
            match arg {
                "--json" => json = true,
                "--credentials" => credentials = true,
                _ => no_refresh = true,
            }
            index += 1;
            continue;
        }
        command_args.push(arg.to_owned());
        index += 1;
    }

    Ok(Some(AuthCommand {
        kind,
        args: command_args,
        json,
        credentials,
        no_refresh,
        min_expiry_ms,
    }))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthCommandTarget {
    pub provider: Option<String>,
    pub model: Option<String>,
}

pub fn validate_auth_command_args(
    args: &Args,
    kind: AuthCommandKind,
) -> Result<AuthCommandTarget, AuthCommandError> {
    let provider = args
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let model = args
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some((option, _)) = args.unknown_flags.iter().next() {
        return Err(AuthCommandError::new(format!(
            "Unknown option --{option} for \"{}\".",
            get_auth_command_name(kind)
        )));
    }
    if args.api_key.is_some() || !args.messages.is_empty() || !args.file_args.is_empty() {
        return Err(AuthCommandError::new(
            "Auth commands only accept --provider and --model",
        ));
    }
    if provider.is_none() && model.is_none() {
        return Err(AuthCommandError::new(if kind == AuthCommandKind::Check {
            "Auth checks require --provider <provider> or --model <model>"
        } else {
            "Credential printing requires --provider <provider> or --model <model>"
        }));
    }
    Ok(AuthCommandTarget { provider, model })
}

/// The credential inside an auth result: the API key, or the bearer token of an
/// `Authorization` header.
pub fn get_auth_credential(auth: Option<&AuthResult>) -> Option<String> {
    static BEARER: OnceLock<Regex> = OnceLock::new();
    let auth = auth?;
    if let Some(api_key) = auth.auth.api_key.as_ref().filter(|key| !key.is_empty()) {
        return Some(api_key.clone());
    }
    let authorization = auth.auth.headers.as_ref().and_then(|headers| {
        headers
            .iter()
            .find(|(name, _)| name.to_lowercase() == "authorization")
            .and_then(|(_, value)| value.clone())
    })?;
    let bearer = BEARER.get_or_init(|| Regex::new(r"(?i)^Bearer\s+(.+)$").expect("valid regex"));
    bearer
        .captures(&authorization)
        .map(|captures| captures[1].to_owned())
}
