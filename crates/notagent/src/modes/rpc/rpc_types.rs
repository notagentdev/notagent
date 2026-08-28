use serde::{Deserialize, Serialize};

use notagent_agent::types::ThinkingLevel;
use notagent_ai::types::{ImageContent, Model};

use crate::core::agent_session::QueueBehavior;
use crate::core::source_info::SourceInfo;

/// The envelope every command shares.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcCommandEnvelope {
    pub id: Option<String>,
    pub command_type: String,
    pub payload: serde_json::Value,
}

impl RpcCommandEnvelope {
    /// Splits a parsed line into `id`, `type` and the rest.
    pub fn from_value(value: serde_json::Value) -> Self {
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let command_type = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        RpcCommandEnvelope {
            id,
            command_type,
            payload: value,
        }
    }

    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_value(self.payload.clone()).map_err(|error| error.to_string())
    }
}

/// `streamingBehavior` of the `prompt` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RpcStreamingBehavior {
    Steer,
    FollowUp,
}

impl From<RpcStreamingBehavior> for QueueBehavior {
    fn from(behavior: RpcStreamingBehavior) -> Self {
        match behavior {
            RpcStreamingBehavior::Steer => QueueBehavior::Steer,
            RpcStreamingBehavior::FollowUp => QueueBehavior::FollowUp,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcPromptCommand {
    pub message: String,
    #[serde(default)]
    pub images: Option<Vec<ImageContent>>,
    #[serde(default)]
    pub streaming_behavior: Option<RpcStreamingBehavior>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcMessageCommand {
    pub message: String,
    #[serde(default)]
    pub images: Option<Vec<ImageContent>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcNewSessionCommand {
    #[serde(default)]
    pub parent_session: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSetModelCommand {
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSetThinkingLevelCommand {
    pub level: ThinkingLevel,
}

/// `"all" | "one-at-a-time"` of the two queue-mode commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpcQueueMode {
    All,
    OneAtATime,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSetQueueModeCommand {
    pub mode: RpcQueueMode,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcCompactCommand {
    #[serde(default)]
    pub custom_instructions: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSetEnabledCommand {
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcBashCommand {
    pub command: String,
    #[serde(default)]
    pub exclude_from_context: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcExportHtmlCommand {
    #[serde(default)]
    pub output_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSwitchSessionCommand {
    pub session_path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcForkCommand {
    pub entry_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcGetEntriesCommand {
    #[serde(default)]
    pub since: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSetSessionNameCommand {
    pub name: String,
}

/// A command available for invocation via prompt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSlashCommand {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `"prompt"` or `"skill"`; the `"extension"` source went with the
    /// extension system.
    pub source: &'static str,
    pub source_info: SourceInfo,
}

/// The state `get_state` answers with.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSessionState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    pub thinking_level: ThinkingLevel,
    pub is_streaming: bool,
    pub is_compacting: bool,
    pub steering_mode: RpcQueueMode,
    pub follow_up_mode: RpcQueueMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub auto_compaction_enabled: bool,
    pub message_count: usize,
    pub pending_message_count: usize,
}

/// One response record.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcResponse {
    pub id: Option<String>,
    pub command: String,
    pub outcome: Result<Option<serde_json::Value>, String>,
}

impl RpcResponse {
    pub fn success(id: Option<String>, command: &str, data: Option<serde_json::Value>) -> Self {
        RpcResponse {
            id,
            command: command.to_owned(),
            outcome: Ok(data),
        }
    }

    pub fn error(id: Option<String>, command: &str, message: impl Into<String>) -> Self {
        RpcResponse {
            id,
            command: command.to_owned(),
            outcome: Err(message.into()),
        }
    }

    /// The wire record. `id` is absent rather than null when there was none, as
    /// `JSON.stringify` drops an undefined property.
    pub fn to_value(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        if let Some(id) = &self.id {
            map.insert("id".to_owned(), serde_json::Value::from(id.clone()));
        }
        map.insert("type".to_owned(), serde_json::Value::from("response"));
        map.insert(
            "command".to_owned(),
            serde_json::Value::from(self.command.clone()),
        );
        match &self.outcome {
            Ok(data) => {
                map.insert("success".to_owned(), serde_json::Value::from(true));
                if let Some(data) = data {
                    map.insert("data".to_owned(), data.clone());
                }
            }
            Err(message) => {
                map.insert("success".to_owned(), serde_json::Value::from(false));
                map.insert("error".to_owned(), serde_json::Value::from(message.clone()));
            }
        }
        serde_json::Value::Object(map)
    }
}
