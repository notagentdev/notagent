use std::sync::Arc;

use crate::api::streams::BedrockConverseStreamApi;
use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthEvent, AuthInfoLink, AuthPrompt,
    AuthPromptKind, AuthPromptOption, AuthResult, BoxFuture, ModelAuth, ProviderAuth,
    ProviderAuthInteraction,
};
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::types::ProviderEnv;

/// `bedrockAuth`
pub struct BedrockAuth;

impl ApiKeyAuth for BedrockAuth {
    fn name(&self) -> &str {
        "AWS credentials or bearer token"
    }

    fn login<'a>(
        &'a self,
        interaction: &'a ProviderAuthInteraction<'a>,
    ) -> Option<BoxFuture<'a, Result<ApiKeyCredential, AuthError>>> {
        Some(Box::pin(async move {
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            let method = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Select {
                        message: "Select Amazon Bedrock authentication method:".to_string(),
                        options: vec![
                            AuthPromptOption {
                                id: "bearer-token".to_string(),
                                label: "Bearer token".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "aws-profile".to_string(),
                                label: "AWS profile".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "credential-chain".to_string(),
                                label: "Existing AWS credential chain".to_string(),
                                description: None,
                            },
                        ],
                    },
                })
                .await?;
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            if method == "bearer-token" {
                return Ok(ApiKeyCredential {
                    key: Some(
                        interaction
                            .prompt(AuthPrompt {
                                signal: None,
                                kind: AuthPromptKind::Secret {
                                    message: "Enter Amazon Bedrock bearer token".to_string(),
                                    placeholder: None,
                                },
                            })
                            .await?,
                    ),
                    env: None,
                });
            }
            interaction.notify(AuthEvent::Info {
                message: "Amazon Bedrock supports AWS profiles, IAM credentials, and role-based credentials."
                    .to_string(),
                links: vec![AuthInfoLink {
                    url: "https://docs.aws.amazon.com/sdkref/latest/guide/standardized-credentials.html"
                        .to_string(),
                    label: Some("AWS credential provider chain".to_string()),
                }],
            });
            if method == "aws-profile" {
                let profile = interaction
                    .prompt(AuthPrompt {
                        signal: None,
                        kind: AuthPromptKind::Text {
                            message: "Enter AWS profile name".to_string(),
                            placeholder: None,
                        },
                    })
                    .await?;
                let mut env = ProviderEnv::new();
                env.insert("AWS_PROFILE".to_string(), profile);
                return Ok(ApiKeyCredential {
                    key: None,
                    env: Some(env),
                });
            }
            if method != "credential-chain" {
                return Err(AuthError(format!(
                    "Unknown Amazon Bedrock auth method: {method}"
                )));
            }
            interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Configure AWS credentials, then press Enter to continue"
                            .to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            Ok(ApiKeyCredential {
                key: None,
                env: None,
            })
        }))
    }

    fn resolve<'a>(
        &'a self,
        input: ApiKeyAuthInput<'a>,
    ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
        Box::pin(async move {
            let env = async |name: &str| -> Result<Option<String>, AuthError> {
                if input.signal.is_cancelled() {
                    return Err(AuthError("The operation was aborted".to_string()));
                }
                let value = input.ctx.env(name).await;
                if input.signal.is_cancelled() {
                    return Err(AuthError("The operation was aborted".to_string()));
                }
                Ok(value.filter(|value| !value.is_empty()))
            };

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
            if env("AWS_BEARER_TOKEN_BEDROCK").await?.is_some() {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: None,
                    source: Some("AWS_BEARER_TOKEN_BEDROCK".to_string()),
                }));
            }
            let credential_profile = input
                .credential
                .as_ref()
                .and_then(|credential| credential.env.as_ref())
                .and_then(|env| env.get("AWS_PROFILE"))
                .filter(|profile| !profile.is_empty())
                .cloned();
            if credential_profile.is_some() || env("AWS_PROFILE").await?.is_some() {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: input
                        .credential
                        .as_ref()
                        .and_then(|credential| credential.env.clone()),
                    source: Some(
                        if credential_profile.is_some() {
                            "stored credential"
                        } else {
                            "AWS_PROFILE"
                        }
                        .to_string(),
                    ),
                }));
            }
            if env("AWS_ACCESS_KEY_ID").await?.is_some()
                && env("AWS_SECRET_ACCESS_KEY").await?.is_some()
            {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: None,
                    source: Some("AWS access keys".to_string()),
                }));
            }
            for (name, source) in [
                ("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "ECS task role"),
                ("AWS_CONTAINER_CREDENTIALS_FULL_URI", "ECS task role"),
                ("AWS_WEB_IDENTITY_TOKEN_FILE", "web identity token"),
            ] {
                if env(name).await?.is_some() {
                    return Ok(Some(AuthResult {
                        auth: ModelAuth::default(),
                        env: None,
                        source: Some(source.to_string()),
                    }));
                }
            }
            Ok(None)
        })
    }
}

/// `amazonBedrockProvider()`
pub fn amazon_bedrock_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "amazon-bedrock".to_string(),
        name: Some("Amazon Bedrock".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(BedrockAuth)),
            oauth: None,
        },
        models: get_builtin_models("amazon-bedrock"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(BedrockConverseStreamApi)),
    })
}
