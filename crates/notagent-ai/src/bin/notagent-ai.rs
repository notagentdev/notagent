//! `notagent-ai` — OAuth login for the built-in providers.
//!
//! 1:1 port of `packages/ai/src/cli.ts` (119 LOC). The npm bin is declared in
//! `package.json`; here the binary target is the crate's `src/bin/` entry (deviation
//! class 4, distribution mechanics).

use std::collections::BTreeMap;
use std::sync::Arc;

use notagent_ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, BoxFuture, OAuthAuth,
    OAuthCredential, ProviderAuthInteraction,
};
use notagent_ai::models::Provider;
use notagent_ai::providers::all::builtin_providers;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdin};
use tokio::sync::Mutex;

const AUTH_FILE: &str = "auth.json";

/// `PROVIDERS` — the built-in providers that advertise OAuth.
fn oauth_providers() -> Vec<Arc<dyn Provider>> {
    builtin_providers()
        .into_iter()
        .filter(|provider| provider.auth().oauth.is_some())
        .collect()
}

/// `prompt(rl, question)` — writes the question and reads one line.
struct Readline {
    lines: Mutex<BufReader<Stdin>>,
}

impl Readline {
    fn new() -> Readline {
        Readline {
            lines: Mutex::new(BufReader::new(tokio::io::stdin())),
        }
    }

    async fn prompt(&self, question: &str) -> Result<String, AuthError> {
        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(question.as_bytes())
            .await
            .map_err(|error| AuthError(error.to_string()))?;
        stdout
            .flush()
            .await
            .map_err(|error| AuthError(error.to_string()))?;
        let mut line = String::new();
        self.lines
            .lock()
            .await
            .read_line(&mut line)
            .await
            .map_err(|error| AuthError(error.to_string()))?;
        // `rl.question` hands over the line without its terminator.
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

/// `answerPrompt(rl, authPrompt)` plus the `notify` switch.
struct CliInteraction {
    readline: Readline,
}

impl AuthInteraction for CliInteraction {
    fn signal(&self) -> Option<tokio_util::sync::CancellationToken> {
        None
    }

    fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
        Box::pin(async move {
            match prompt.kind {
                AuthPromptKind::Select { message, options } => {
                    println!("\n{message}");
                    for (index, option) in options.iter().enumerate() {
                        println!("  {}. {}", index + 1, option.label);
                    }
                    let answer = self
                        .readline
                        .prompt(&format!("Enter number (1-{}): ", options.len()))
                        .await?;
                    let choice = answer.trim().parse::<i64>().unwrap_or(0) - 1;
                    let selected = usize::try_from(choice)
                        .ok()
                        .and_then(|choice| options.get(choice));
                    match selected {
                        Some(option) => Ok(option.id.clone()),
                        None => Err(AuthError("Invalid selection".to_string())),
                    }
                }
                AuthPromptKind::Text {
                    message,
                    placeholder,
                }
                | AuthPromptKind::Secret {
                    message,
                    placeholder,
                }
                | AuthPromptKind::ManualCode {
                    message,
                    placeholder,
                } => {
                    let suffix = placeholder
                        .filter(|placeholder| !placeholder.is_empty())
                        .map(|placeholder| format!(" ({placeholder})"))
                        .unwrap_or_default();
                    self.readline.prompt(&format!("{message}{suffix}: ")).await
                }
            }
        })
    }

    fn notify(&self, event: AuthEvent) {
        match event {
            AuthEvent::AuthUrl { url, instructions } => {
                println!("\nOpen this URL in your browser:\n{url}");
                if let Some(instructions) = instructions.filter(|value| !value.is_empty()) {
                    println!("{instructions}");
                }
            }
            AuthEvent::DeviceCode {
                user_code,
                verification_uri,
                ..
            } => {
                println!("\nOpen this URL in your browser:\n{verification_uri}");
                println!("Enter code: {user_code}");
            }
            AuthEvent::Info { message, .. } | AuthEvent::Progress { message } => {
                println!("{message}");
            }
        }
    }
}

/// `loadAuth()` — an unreadable or malformed file is treated as empty.
fn load_auth() -> BTreeMap<String, OAuthCredential> {
    let Ok(content) = std::fs::read_to_string(AUTH_FILE) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// `saveAuth(auth)`
fn save_auth(auth: &BTreeMap<String, OAuthCredential>) -> Result<(), String> {
    let content = serde_json::to_string_pretty(auth).map_err(|error| error.to_string())?;
    std::fs::write(AUTH_FILE, content).map_err(|error| error.to_string())
}

/// `login(providerId)`
async fn login(provider_id: &str) -> Result<(), String> {
    let providers = oauth_providers();
    let provider = providers
        .iter()
        .find(|provider| provider.id() == provider_id)
        .ok_or_else(|| format!("Unknown provider: {provider_id}"))?;
    let oauth: Arc<dyn OAuthAuth> = provider.auth().oauth.clone().expect("filtered for oauth");

    let interaction = CliInteraction {
        readline: Readline::new(),
    };
    let credential = oauth
        .login(&ProviderAuthInteraction {
            interaction: &interaction,
            signal: tokio_util::sync::CancellationToken::new(),
        })
        .await
        .map_err(|error| error.0)?;

    let mut auth = load_auth();
    auth.insert(provider_id.to_string(), credential);
    save_auth(&auth)?;
    println!("\nCredentials saved to {AUTH_FILE}");
    Ok(())
}

async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let providers = oauth_providers();
    let command = args.first().map(String::as_str);

    if matches!(command, None | Some("help") | Some("--help") | Some("-h")) {
        let provider_list = providers
            .iter()
            .map(|provider| format!("  {:<20} {}", provider.id(), provider.name()))
            .collect::<Vec<_>>()
            .join("\n");
        println!(
            "Usage: notagent-ai <command> [provider]\n\nCommands:\n  login [provider]  Login to an OAuth provider\n  list              List available providers\n\nProviders:\n{provider_list}"
        );
        return Ok(());
    }
    if command == Some("list") {
        for provider in &providers {
            println!("{:<20} {}", provider.id(), provider.name());
        }
        return Ok(());
    }
    if command == Some("login") {
        let mut provider_id = args.get(1).cloned();
        if provider_id.is_none() {
            let readline = Readline::new();
            for (index, provider) in providers.iter().enumerate() {
                println!("  {}. {}", index + 1, provider.name());
            }
            let answer = readline
                .prompt(&format!("Enter number (1-{}): ", providers.len()))
                .await
                .map_err(|error| error.0)?;
            let index = answer.trim().parse::<i64>().unwrap_or(0) - 1;
            provider_id = usize::try_from(index)
                .ok()
                .and_then(|index| providers.get(index))
                .map(|provider| provider.id().to_string());
        }
        let provider_id = provider_id.unwrap_or_default();
        if provider_id.is_empty()
            || !providers
                .iter()
                .any(|provider| provider.id() == provider_id)
        {
            return Err(format!("Unknown provider: {provider_id}"));
        }
        return login(&provider_id).await;
    }
    Err(format!("Unknown command: {}", command.unwrap_or_default()))
}

#[tokio::main]
async fn main() {
    notagent_ai::install_default_crypto_provider();
    if let Err(error) = run().await {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
