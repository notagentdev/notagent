//! Fast deterministic hash used to shorten long strings.
//!
//! 1:1 port of `packages/ai/src/utils/hash.ts` (13 LOC). The TS implementation works on
//! UTF-16 code units (`charCodeAt`), which this port reproduces via `encode_utf16`.

/// `shortHash(str)`
pub fn short_hash(text: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    for code_unit in text.encode_utf16() {
        let character = u32::from(code_unit);
        h1 = (h1 ^ character).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ character).wrapping_mul(1_597_334_677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h2 ^ (h2 >> 13)).wrapping_mul(3_266_489_909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h1 ^ (h1 >> 13)).wrapping_mul(3_266_489_909);
    format!("{}{}", to_base36(h2), to_base36(h1))
}

/// `Number.prototype.toString(36)` for unsigned 32-bit values.
fn to_base36(mut value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buffer = Vec::new();
    while value > 0 {
        buffer.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    buffer.reverse();
    String::from_utf8(buffer).expect("base36 digits are ASCII")
}
