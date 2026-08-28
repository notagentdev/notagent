use notagent_protocol::{
    CborOptions, CborValue, DEFAULT_MAX_CBOR_BYTE_LENGTH, DEFAULT_MAX_CBOR_CONTAINER_LENGTH,
    DEFAULT_MAX_CBOR_DEPTH, decode_cbor, encode_cbor,
};

fn from_hex(hex: &str) -> Vec<u8> {
    assert!(
        hex.len().is_multiple_of(2),
        "Hex fixture must contain whole bytes"
    );
    (0..hex.len() / 2)
        .map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn encode(value: &CborValue) -> Vec<u8> {
    encode_cbor(value, None).expect("encodes")
}

fn known_vectors() -> Vec<(CborValue, &'static str)> {
    vec![
        (CborValue::Null, "f6"),
        (CborValue::Bool(false), "f4"),
        (CborValue::Bool(true), "f5"),
        (CborValue::Number(0.0), "00"),
        (CborValue::Number(1.0), "01"),
        (CborValue::Number(10.0), "0a"),
        (CborValue::Number(23.0), "17"),
        (CborValue::Number(24.0), "1818"),
        (CborValue::Number(25.0), "1819"),
        (CborValue::Number(100.0), "1864"),
        (CborValue::Number(1000.0), "1903e8"),
        (CborValue::Number(1_000_000.0), "1a000f4240"),
        (CborValue::Number(1_000_000_000_000.0), "1b000000e8d4a51000"),
        (
            CborValue::Number(9_007_199_254_740_991.0),
            "1b001fffffffffffff",
        ),
        (CborValue::Number(-1.0), "20"),
        (CborValue::Number(-10.0), "29"),
        (CborValue::Number(-24.0), "37"),
        (CborValue::Number(-25.0), "3818"),
        (CborValue::Number(-100.0), "3863"),
        (CborValue::Number(-1000.0), "3903e7"),
        (CborValue::Number(-1_000_000.0), "3a000f423f"),
        (
            CborValue::Number(-9_007_199_254_740_991.0),
            "3b001ffffffffffffe",
        ),
        (CborValue::Number(1.1), "fb3ff199999999999a"),
        (CborValue::Number(-0.0), "fb8000000000000000"),
        (CborValue::Bytes(vec![1, 2, 3, 4]), "4401020304"),
        (CborValue::text(""), "60"),
        (CborValue::text("IETF"), "6449455446"),
        (CborValue::text("ü"), "62c3bc"),
        (CborValue::text("水"), "63e6b0b4"),
        (CborValue::text("𐅑"), "64f0908591"),
        (CborValue::Array(vec![]), "80"),
        (
            CborValue::Array(vec![
                CborValue::Number(1.0),
                CborValue::Number(2.0),
                CborValue::Number(3.0),
            ]),
            "83010203",
        ),
        (
            CborValue::Array(vec![
                CborValue::Number(1.0),
                CborValue::Array(vec![CborValue::Number(2.0), CborValue::Number(3.0)]),
                CborValue::Array(vec![CborValue::Number(4.0), CborValue::Number(5.0)]),
            ]),
            "8301820203820405",
        ),
        (
            CborValue::map([
                ("a", CborValue::Number(1.0)),
                (
                    "b",
                    CborValue::Array(vec![CborValue::Number(2.0), CborValue::Number(3.0)]),
                ),
            ]),
            "a26161016162820203",
        ),
    ]
}

#[test]
fn encodes_and_decodes_rfc_8949_vectors() {
    for (index, (value, wire)) in known_vectors().into_iter().enumerate() {
        assert_eq!(to_hex(&encode(&value)), wire, "vector {index} encodes");
        let decoded = decode_cbor(&from_hex(wire), None).expect("decodes");
        assert_eq!(decoded, value, "vector {index} decodes");
        // `Object.is(decoded, -0)` — the sign of zero is preserved.
        if let (CborValue::Number(decoded), CborValue::Number(expected)) = (&decoded, &value) {
            assert_eq!(
                decoded.is_sign_negative(),
                expected.is_sign_negative(),
                "vector {index} keeps the number sign"
            );
        }
    }
}

// has no Rust equivalent (there is no `undefined`); omitting optional fields is
// covered by `tests/protocol.rs` at the codec level.

#[test]
fn preserves_a_leading_unicode_bom_and_treats_proto_as_data() {
    assert_eq!(
        decode_cbor(&from_hex("63efbbbf"), None).unwrap(),
        CborValue::text("\u{feff}")
    );
    let value = CborValue::map([("__proto__", CborValue::text("safe"))]);
    assert_eq!(decode_cbor(&encode(&value), None).unwrap(), value);
}

#[test]
fn rejects_unsupported_encoder_values() {
    // Not representable in Rust: undefined, array holes, bigint, symbol,
    for (label, value) in [
        ("NaN", CborValue::Number(f64::NAN)),
        ("positive infinity", CborValue::Number(f64::INFINITY)),
        ("negative infinity", CborValue::Number(f64::NEG_INFINITY)),
        (
            "unsafe positive integer",
            CborValue::Number(9_007_199_254_740_992.0),
        ),
        (
            "unsafe negative integer",
            CborValue::Number(-9_007_199_254_740_992.0),
        ),
    ] {
        assert!(encode_cbor(&value, None).is_err(), "rejects {label}");
    }
}

#[test]
fn rejects_excessive_encoder_depth() {
    // Lossy strings and cycles cannot be represented in Rust.
    let mut too_deep = CborValue::Null;
    for _ in 0..=DEFAULT_MAX_CBOR_DEPTH {
        too_deep = CborValue::Array(vec![too_deep]);
    }
    let error = encode_cbor(&too_deep, None).expect_err("rejects excessive depth");
    assert!(
        error.message().to_lowercase().contains("depth"),
        "{}",
        error.message()
    );
}

#[test]
fn rejects_invalid_decoder_input() {
    for (label, wire) in [
        ("empty input", ""),
        ("truncated integer", "18"),
        ("reserved additional information", "1c"),
        ("indefinite byte string", "5f"),
        ("indefinite text string", "7f"),
        ("indefinite array", "9f"),
        ("indefinite map", "bf"),
        ("tag", "c000"),
        ("undefined", "f7"),
        ("unsupported simple value", "e0"),
        ("break outside an indefinite item", "ff"),
        ("float16", "f93c00"),
        ("float32", "fa3f800000"),
        ("positive infinity", "fb7ff0000000000000"),
        ("NaN", "fb7ff8000000000000"),
        ("truncated float64", "fb3ff00000"),
        ("truncated byte string", "44010203"),
        ("truncated text string", "636162"),
        ("truncated array", "8201"),
        ("truncated map", "a16161"),
        ("trailing data", "0000"),
        ("non-string map key", "a10102"),
        ("duplicate map key", "a2616101616102"),
        ("invalid UTF-8 byte", "61ff"),
        ("overlong UTF-8", "62c080"),
        ("UTF-8 surrogate", "63eda080"),
        ("unsafe positive integer", "1b0020000000000000"),
        ("unsafe negative integer", "3b001fffffffffffff"),
        ("unsafe integer encoded as float64", "fb4340000000000000"),
    ] {
        assert!(
            decode_cbor(&from_hex(wire), None).is_err(),
            "rejects {label}"
        );
    }
}

#[test]
fn enforces_depth_and_declared_length_limits_before_traversing_values() {
    let mut too_deep = vec![0x81u8; (DEFAULT_MAX_CBOR_DEPTH + 2) as usize];
    *too_deep.last_mut().unwrap() = 0xf6;
    let error = decode_cbor(&too_deep, None).expect_err("rejects excessive depth");
    assert!(
        error.message().to_lowercase().contains("depth"),
        "{}",
        error.message()
    );

    let oversized_bytes = from_hex(&format!("5a{:08x}", DEFAULT_MAX_CBOR_BYTE_LENGTH + 1));
    let oversized_text = from_hex(&format!("7a{:08x}", DEFAULT_MAX_CBOR_BYTE_LENGTH + 1));
    let oversized_array = from_hex(&format!("9a{:08x}", DEFAULT_MAX_CBOR_CONTAINER_LENGTH + 1));
    let oversized_map = from_hex(&format!("ba{:08x}", DEFAULT_MAX_CBOR_CONTAINER_LENGTH + 1));
    for wire in [
        oversized_bytes,
        oversized_text,
        oversized_array,
        oversized_map,
    ] {
        let error = decode_cbor(&wire, None).expect_err("rejects oversized declared length");
        assert!(
            error.message().to_lowercase().contains("limit"),
            "{}",
            error.message()
        );
    }
}

#[test]
fn supports_stricter_caller_provided_limits() {
    let container = Some(CborOptions::with_max_container_length(2));
    let bytes = Some(CborOptions::with_max_byte_length(2));
    for error in [
        decode_cbor(&from_hex("83010203"), container).expect_err("array limit"),
        decode_cbor(&from_hex("626162"), bytes).expect_err("text limit"),
        encode_cbor(
            &CborValue::Array(vec![
                CborValue::Number(1.0),
                CborValue::Number(2.0),
                CborValue::Number(3.0),
            ]),
            container,
        )
        .expect_err("encoded array limit"),
        encode_cbor(&CborValue::text("ab"), bytes).expect_err("encoded text limit"),
    ] {
        assert!(
            error.message().to_lowercase().contains("limit"),
            "{}",
            error.message()
        );
    }
}
