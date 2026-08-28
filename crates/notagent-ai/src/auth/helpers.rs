use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthPrompt, AuthPromptKind,
    AuthResult, BoxFuture, ModelAuth, ProviderAuthInteraction,
};

/// `envApiKeyAuth(name, envVars)` — stored key wins, otherwise the first set env var.
pub struct EnvApiKeyAuth {
    name: String,
    env_vars: Vec<String>,
}

impl EnvApiKeyAuth {
    pub fn new(
        name: impl Into<String>,
        env_vars: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        EnvApiKeyAuth {
            name: name.into(),
            env_vars: env_vars.into_iter().map(Into::into).collect(),
        }
    }
}

impl ApiKeyAuth for EnvApiKeyAuth {
    fn name(&self) -> &str {
        &self.name
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            let key = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Secret {
                        message: format!("Enter {}", self.name),
                        placeholder: None,
                    },
                })
                .await?;
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            Ok(ApiKeyCredential {
                key: Some(key),
                env: None,
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            if input.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            if let Some(credential) = &input.credential
                && let Some(key) = credential.key.as_ref().filter(|key| !key.is_empty())
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key.clone()),
                        ..Default::default()
                    },
                    env: credential.env.clone(),
                    source: Some("stored credential".to_string()),
                }));
            }
            for env_var in &self.env_vars {
                let value = input.ctx.env(env_var).await;
                if input.signal.is_cancelled() {
                    return Err(AuthError("The operation was aborted".to_string()));
                }
                if let Some(value) = value.filter(|value| !value.is_empty()) {
                    return Ok(Some(AuthResult {
                        auth: ModelAuth {
                            api_key: Some(value),
                            ..Default::default()
                        },
                        env: None,
                        source: Some(env_var.clone()),
                    }));
                }
            }
            Ok(None)
        })
    }
}
