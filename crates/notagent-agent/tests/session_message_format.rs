use std::collections::BTreeSet;

use notagent_agent::types::AgentMessage;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/session-messages.jsonl");

fn fixture_messages() -> Vec<Value> {
    FIXTURE
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("fixture line is valid JSON"))
        .collect()
}

#[test]
fn every_fixture_message_roundtrips_without_change() {
    let messages = fixture_messages();
    assert_eq!(messages.len(), 44, "unexpected fixture size");

    for (index, original) in messages.iter().enumerate() {
        let parsed: AgentMessage = serde_json::from_value(original.clone())
            .unwrap_or_else(|error| panic!("line {index}: deserialization failed: {error}"));
        let reserialized = serde_json::to_value(&parsed)
            .unwrap_or_else(|error| panic!("line {index}: serialization failed: {error}"));
        assert_eq!(
            &reserialized, original,
            "line {index}: roundtrip changed the JSON"
        );
    }
}

#[test]
fn fixture_covers_all_message_roles_and_content_kinds() {
    let mut roles = BTreeSet::new();
    let mut content_kinds = BTreeSet::new();
    for message in fixture_messages() {
        roles.insert(message["role"].as_str().expect("role").to_string());
        if let Some(blocks) = message["content"].as_array() {
            for block in blocks {
                content_kinds.insert(block["type"].as_str().expect("content type").to_string());
            }
        }
    }
    assert_eq!(
        roles,
        ["assistant", "bashExecution", "toolResult", "user"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    assert_eq!(
        content_kinds,
        ["text", "thinking", "toolCall"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
}

#[test]
fn llm_messages_convert_without_loss() {
    for message in fixture_messages() {
        let parsed: AgentMessage =
            serde_json::from_value(message.clone()).expect("deserialization");
        match &parsed {
            AgentMessage::User(_) | AgentMessage::Assistant(_) | AgentMessage::ToolResult(_) => {
                let llm = parsed.as_llm_message().expect("LLM role");
                assert_eq!(serde_json::to_value(&llm).expect("serialization"), message);
            }
            // Custom roles are converted at the LLM boundary.
            _ => assert!(parsed.as_llm_message().is_none()),
        }
    }
}
