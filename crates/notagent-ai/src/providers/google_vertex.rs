use std::sync::Arc;

use crate::api::streams::GoogleVertexApi;
use crate::auth::types::{
    ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthError, AuthEvent, AuthInfoLink, AuthPrompt,
    AuthPromptKind, AuthPromptOption, AuthResult, BoxFuture, ModelAuth, ProviderAuth,
    ProviderAuthInteraction,
};
use crate::model_catalog::get_builtin_models;
use crate::models::{BuiltProvider, CreateProviderOptions, ProviderApis, create_provider};
use crate::types::ProviderEnv;

const VERTEX_ADC_PATH: &str = "~/.config/gcloud/application_default_credentials.json";

/// `vertexAuth`
pub struct VertexAuth;

impl ApiKeyAuth for VertexAuth {
    fn name(&self) -> &str {
        "Google Cloud credentials"
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
                        message: "Select Google Vertex AI authentication method:".to_string(),
                        options: vec![
                            AuthPromptOption {
                                id: "api-key".to_string(),
                                label: "Google Cloud API key".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "adc".to_string(),
                                label: "Application Default Credentials".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: "service-account".to_string(),
                                label: "Service account credentials file".to_string(),
                                description: None,
                            },
                        ],
                    },
                })
                .await?;
            if interaction.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            if method == "api-key" {
                return Ok(ApiKeyCredential {
                    key: Some(
                        interaction
                            .prompt(AuthPrompt {
                                signal: None,
                                kind: AuthPromptKind::Secret {
                                    message: "Enter Google Cloud API key".to_string(),
                                    placeholder: None,
                                },
                            })
                            .await?,
                    ),
                    env: None,
                });
            }
            if method != "adc" && method != "service-account" {
                return Err(AuthError(format!(
                    "Unknown Google Vertex AI auth method: {method}"
                )));
            }
            interaction.notify(AuthEvent::Info {
                message: if method == "adc" {
                    "Run `gcloud auth application-default login`, then provide the project and location."
                } else {
                    "Provide a service account credentials file, project, and location."
                }
                .to_string(),
                links: vec![AuthInfoLink {
                    url: "https://cloud.google.com/docs/authentication/provide-credentials-adc"
                        .to_string(),
                    label: Some("Application Default Credentials".to_string()),
                }],
            });
            let project = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Enter Google Cloud project ID".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let location = interaction
                .prompt(AuthPrompt {
                    signal: None,
                    kind: AuthPromptKind::Text {
                        message: "Enter Google Cloud location".to_string(),
                        placeholder: None,
                    },
                })
                .await?;
            let credentials_path = if method == "service-account" {
                Some(
                    interaction
                        .prompt(AuthPrompt {
                            signal: None,
                            kind: AuthPromptKind::Text {
                                message: "Enter service account credentials file path".to_string(),
                                placeholder: None,
                            },
                        })
                        .await?,
                )
            } else {
                None
            };
            let mut env = ProviderEnv::new();
            env.insert("GOOGLE_CLOUD_PROJECT".to_string(), project);
            env.insert("GOOGLE_CLOUD_LOCATION".to_string(), location);
            if let Some(credentials_path) = credentials_path.filter(|path| !path.is_empty()) {
                env.insert(
                    "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
                    credentials_path,
                );
            }
            Ok(ApiKeyCredential {
                key: None,
                env: Some(env),
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
            let credential_env = |name: &str| -> Option<String> {
                input
                    .credential
                    .as_ref()
                    .and_then(|credential| credential.env.as_ref())
                    .and_then(|env| env.get(name))
                    .filter(|value| !value.is_empty())
                    .cloned()
            };
            let credential_key = input
                .credential
                .as_ref()
                .and_then(|credential| credential.key.clone())
                .filter(|key| !key.is_empty());

            let key = match credential_key.clone() {
                Some(key) => Some(key),
                None => env("GOOGLE_CLOUD_API_KEY").await?,
            };
            if let Some(key) = key {
                return Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key),
                        ..Default::default()
                    },
                    env: None,
                    source: Some(
                        if credential_key.is_some() {
                            "stored credential"
                        } else {
                            "GOOGLE_CLOUD_API_KEY"
                        }
                        .to_string(),
                    ),
                }));
            }

            let adc_path = match credential_env("GOOGLE_APPLICATION_CREDENTIALS") {
                Some(path) => Some(path),
                None => env("GOOGLE_APPLICATION_CREDENTIALS").await?,
            };
            if input.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            let has_credentials = input
                .ctx
                .file_exists(adc_path.as_deref().unwrap_or(VERTEX_ADC_PATH))
                .await;
            if input.signal.is_cancelled() {
                return Err(AuthError("The operation was aborted".to_string()));
            }
            let project = match credential_env("GOOGLE_CLOUD_PROJECT") {
                Some(project) => Some(project),
                None => match env("GOOGLE_CLOUD_PROJECT").await? {
                    Some(project) => Some(project),
                    None => env("GCLOUD_PROJECT").await?,
                },
            };
            let location = match credential_env("GOOGLE_CLOUD_LOCATION") {
                Some(location) => Some(location),
                None => env("GOOGLE_CLOUD_LOCATION").await?,
            };
            if has_credentials && project.is_some() && location.is_some() {
                return Ok(Some(AuthResult {
                    auth: ModelAuth::default(),
                    env: input
                        .credential
                        .as_ref()
                        .and_then(|credential| credential.env.clone()),
                    source: Some(
                        if input.credential.is_some() {
                            "stored credential"
                        } else {
                            "gcloud application default credentials"
                        }
                        .to_string(),
                    ),
                }));
            }
            Ok(None)
        })
    }
}

/// `googleVertexProvider()`
pub fn google_vertex_provider() -> Arc<BuiltProvider> {
    create_provider(CreateProviderOptions {
        id: "google-vertex".to_string(),
        name: Some("Google Vertex AI".to_string()),
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(VertexAuth)),
            oauth: None,
        },
        models: get_builtin_models("google-vertex"),
        fetch_models: None,
        filter_models: None,
        api: ProviderApis::Single(Arc::new(GoogleVertexApi)),
    })
}
