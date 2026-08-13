//! Differential test of the SSE decoder against the TypeScript original.
//!
//! The fixture was generated in the TS repo from the decoder functions of
//! `packages/ai/src/api/anthropic-messages.ts` (`/tmp/gen-sse-fixture.cjs`): 16
//! payloads — plain events, multi-line data, comments, CR/LF/CRLF, missing colons,
//! trailing events without a blank line — each fed as a whole and split at every
//! possible position (355 cases).

use notagent_ai::api::sse::{ServerSentEvent, SseDecoder};

const FIXTURE: &str = include_str!("fixtures/sse-decoder.jsonl");

#[derive(serde::Deserialize)]
struct Case {
    chunks: Vec<String>,
    events: Vec<ExpectedEvent>,
}

#[derive(serde::Deserialize)]
struct ExpectedEvent {
    event: Option<String>,
    data: String,
    raw: Vec<String>,
}

fn decode(chunks: &[String]) -> Vec<ServerSentEvent> {
    let mut decoder = SseDecoder::new();
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.feed(chunk));
    }
    events.extend(decoder.finish());
    events
}

#[test]
fn matches_typescript_for_every_fixture_case() {
    let cases: Vec<Case> = FIXTURE
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(cases.len(), 355, "unexpected fixture size");

    for case in &cases {
        let actual = decode(&case.chunks);
        assert_eq!(
            actual.len(),
            case.events.len(),
            "event count for chunks {:?}: got {:?}",
            case.chunks,
            actual
        );
        for (actual, expected) in actual.iter().zip(&case.events) {
            assert_eq!(
                actual.event, expected.event,
                "event name for chunks {:?}",
                case.chunks
            );
            assert_eq!(
                actual.data, expected.data,
                "data for chunks {:?}",
                case.chunks
            );
            assert_eq!(
                actual.raw, expected.raw,
                "raw lines for chunks {:?}",
                case.chunks
            );
        }
    }
}
