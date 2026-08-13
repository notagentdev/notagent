//! Time-ordered UUIDv7.
//!
//! 1:1 port of `packages/ai/src/utils/uuid.ts` (48 LOC) including the monotonic
//! sequence counter and the timestamp bump on sequence overflow.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngExt;

struct MonotonicState {
    /// TS: `let lastTimestamp = -Infinity`.
    last_timestamp: i64,
    sequence: u32,
    initialized: bool,
}

static STATE: Mutex<MonotonicState> = Mutex::new(MonotonicState {
    last_timestamp: 0,
    sequence: 0,
    initialized: false,
});

/// `uuidv7()` — generates a time-ordered UUIDv7 in canonical text form.
pub fn uuidv7() -> String {
    let mut random = [0u8; 16];
    rand::rng().fill(&mut random);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before the Unix epoch")
        .as_millis() as i64;
    let mut state = STATE.lock().expect("UUIDv7 state poisoned");
    next_uuidv7(&mut state, timestamp, random)
}

/// The algorithm itself, with clock and randomness injected so it can be tested the
/// same way `packages/ai/test/uuid.test.ts` stubs `Date.now` and `crypto.getRandomValues`.
fn next_uuidv7(state: &mut MonotonicState, timestamp: i64, random: [u8; 16]) -> String {
    if !state.initialized || timestamp > state.last_timestamp {
        state.sequence = (random[6] as u32) << 24
            | (random[7] as u32) << 16
            | (random[8] as u32) << 8
            | (random[9] as u32);
        state.last_timestamp = timestamp;
        state.initialized = true;
    } else {
        state.sequence = state.sequence.wrapping_add(1);
        if state.sequence == 0 {
            state.last_timestamp += 1;
        }
    }
    let last_timestamp = state.last_timestamp;
    let sequence = state.sequence;

    let mut bytes = [0u8; 16];
    bytes[0] = ((last_timestamp / 0x100_0000_0000) & 0xff) as u8;
    bytes[1] = ((last_timestamp / 0x1_0000_0000) & 0xff) as u8;
    bytes[2] = ((last_timestamp / 0x100_0000) & 0xff) as u8;
    bytes[3] = ((last_timestamp / 0x1_0000) & 0xff) as u8;
    bytes[4] = ((last_timestamp / 0x100) & 0xff) as u8;
    bytes[5] = (last_timestamp & 0xff) as u8;
    bytes[6] = 0x70 | ((sequence >> 28) & 0x0f) as u8;
    bytes[7] = ((sequence >> 20) & 0xff) as u8;
    bytes[8] = 0x80 | ((sequence >> 14) & 0x3f) as u8;
    bytes[9] = ((sequence >> 6) & 0xff) as u8;
    bytes[10] = (((sequence & 0x3f) << 2) as u8) | (random[10] & 0x03);
    bytes[11] = random[11];
    bytes[12] = random[12];
    bytes[13] = random[13];
    bytes[14] = random[14];
    bytes[15] = random[15];

    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `packages/ai/test/uuid.test.ts`: the RFC 9562 layout and monotonic order,
    /// driven by the same stubbed randomness and clock value.
    #[test]
    fn uses_rfc_9562_layout_and_preserves_monotonic_order() {
        const TIMESTAMP: i64 = 0x0123_4567_89ab;
        let mut state = MonotonicState {
            last_timestamp: 0,
            sequence: 0,
            initialized: false,
        };
        let random_values = [
            [
                0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xfe, 0x01, 0x11, 0x22, 0x33, 0x44, 0x55,
            ],
            [0u8; 16],
            [0u8; 16],
        ];

        let first = next_uuidv7(&mut state, TIMESTAMP, random_values[0]);
        let second = next_uuidv7(&mut state, TIMESTAMP, random_values[1]);
        let third = next_uuidv7(&mut state, TIMESTAMP, random_values[2]);

        assert_eq!(first, "01234567-89ab-7fff-bfff-f91122334455");
        assert_eq!(second, "01234567-89ab-7fff-bfff-fc0000000000");
        assert_eq!(third, "01234567-89ac-7000-8000-000000000000");
        for uuid in [&first, &second, &third] {
            assert!(
                matches_uuid_v7_layout(uuid),
                "{uuid} does not match the v7 layout"
            );
        }
        assert_eq!(parse_timestamp(&first), TIMESTAMP);
        assert_eq!(parse_timestamp(&second), TIMESTAMP);
        assert_eq!(parse_timestamp(&third), TIMESTAMP + 1);
        assert!(first < second);
        assert!(second < third);
    }

    #[test]
    fn generates_unique_increasing_ids() {
        let ids: Vec<String> = (0..100).map(|_| uuidv7()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "UUIDv7 values must be monotonically increasing"
        );
        sorted.dedup();
        assert_eq!(sorted.len(), 100, "UUIDv7 values must be unique");
        assert!(ids.iter().all(|id| matches_uuid_v7_layout(id)));
    }

    /// `/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/`
    fn matches_uuid_v7_layout(uuid: &str) -> bool {
        let groups: Vec<&str> = uuid.split('-').collect();
        if groups.len() != 5 {
            return false;
        }
        let lengths = [8, 4, 4, 4, 12];
        if groups
            .iter()
            .zip(lengths)
            .any(|(group, length)| group.len() != length)
        {
            return false;
        }
        if !uuid
            .chars()
            .all(|character| character == '-' || character.is_ascii_hexdigit())
        {
            return false;
        }
        if !uuid
            .chars()
            .all(|character| !character.is_ascii_uppercase())
        {
            return false;
        }
        groups[2].starts_with('7')
            && matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b'))
    }

    fn parse_timestamp(uuid: &str) -> i64 {
        i64::from_str_radix(&uuid.replace('-', "")[..12], 16).expect("hex timestamp")
    }
}
