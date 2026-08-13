//! Zeitgeordnete UUIDv7.
//!
//! 1:1-Port von `packages/ai/src/utils/uuid.ts` (48 LOC) — inklusive des
//! monotonen Sequenzzählers und des Timestamp-Vorschubs bei Sequenzüberlauf.

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

/// `uuidv7()` — erzeugt eine zeitgeordnete UUIDv7 in kanonischer Textform.
pub fn uuidv7() -> String {
    let mut random = [0u8; 16];
    rand::rng().fill(&mut random);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Systemzeit vor der Unix-Epoche")
        .as_millis() as i64;

    let mut state = STATE.lock().expect("UUIDv7-Zustand vergiftet");
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
    drop(state);

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
