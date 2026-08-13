//! Port von `packages/protocol/test/framing.test.ts`.

use notagent_protocol::{
    DEFAULT_MAX_FRAME_LENGTH, FrameDecoder, FrameDecoderOptions, assert_complete_frame,
    encode_frame,
};

fn frame(payload: &[u8]) -> Vec<u8> {
    encode_frame(payload).expect("encodes")
}

fn decoder(max_frame_length: Option<u64>) -> FrameDecoder {
    FrameDecoder::new(max_frame_length.map(FrameDecoderOptions::with_max_frame_length))
        .expect("valid options")
}

fn concatenate(chunks: &[Vec<u8>]) -> Vec<u8> {
    chunks.iter().flatten().copied().collect()
}

#[test]
fn prefixes_payloads_with_a_four_byte_big_endian_length() {
    assert_eq!(
        frame(&[0xaa, 0xbb, 0xcc]),
        vec![0x00, 0x00, 0x00, 0x03, 0xaa, 0xbb, 0xcc]
    );
    assert_eq!(frame(&[]), vec![0, 0, 0, 0]);
}

#[test]
fn validates_one_complete_bounded_frame_without_accepting_trailing_or_partial_bytes() {
    assert!(
        assert_complete_frame(
            &[0, 0, 0, 2, 1, 2],
            Some(FrameDecoderOptions::with_max_frame_length(2))
        )
        .is_ok()
    );
    let error = assert_complete_frame(&[0, 0, 0, 2, 1], None).expect_err("partial payload");
    assert!(
        error.message().to_lowercase().contains("complete"),
        "{}",
        error.message()
    );
    let error = assert_complete_frame(&[0, 0, 0, 1, 1, 2], None).expect_err("trailing bytes");
    assert!(
        error.message().to_lowercase().contains("exactly"),
        "{}",
        error.message()
    );
    let error = assert_complete_frame(
        &[0, 0, 0, 3, 1, 2, 3],
        Some(FrameDecoderOptions::with_max_frame_length(2)),
    )
    .expect_err("oversized");
    assert!(
        error.message().to_lowercase().contains("limit"),
        "{}",
        error.message()
    );
}

#[test]
fn decodes_fragmented_coalesced_and_empty_frames_in_order() {
    let wire = concatenate(&[frame(&[1, 2, 3]), frame(&[]), frame(&[4])]);
    let mut decoder = decoder(None);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    for byte in &wire {
        frames.extend(decoder.push(&[*byte]).expect("pushes"));
    }
    decoder.end().expect("ends");
    assert_eq!(frames, vec![vec![1, 2, 3], vec![], vec![4]]);

    let mut coalesced = self::decoder(None);
    assert_eq!(coalesced.push(&wire).expect("pushes"), frames);
    coalesced.end().expect("ends");
}

#[test]
fn assembles_payloads_spanning_multiple_internal_blocks() {
    let payload: Vec<u8> = (0..70_000u32).map(|index| (index % 251) as u8).collect();
    let wire = frame(&payload);
    let mut decoder = decoder(None);
    let mut frames = decoder.push(&wire[0..101]).expect("pushes");
    frames.extend(decoder.push(&wire[101..65_541]).expect("pushes"));
    frames.extend(decoder.push(&wire[65_541..]).expect("pushes"));
    decoder.end().expect("ends");
    assert_eq!(frames, vec![payload]);
}

#[test]
fn handles_every_split_point_across_a_frame() {
    let wire = frame(&[10, 20, 30, 40]);
    for split in 0..=wire.len() {
        let mut decoder = decoder(None);
        let mut frames = decoder.push(&wire[0..split]).expect("pushes");
        frames.extend(decoder.push(&wire[split..]).expect("pushes"));
        decoder.end().expect("ends");
        assert_eq!(frames, vec![vec![10, 20, 30, 40]], "split at {split}");
    }
}

#[test]
fn copies_payload_bytes_instead_of_retaining_or_aliasing_input_chunks() {
    let mut chunk = frame(&[1, 2, 3]);
    let mut decoder = decoder(None);
    let frames = decoder.push(&chunk).expect("pushes");
    chunk.fill(9);
    assert_eq!(frames, vec![vec![1, 2, 3]]);
}

#[test]
fn accepts_empty_chunks_and_a_clean_empty_stream() {
    let mut decoder = decoder(None);
    assert_eq!(decoder.push(&[]).expect("pushes"), Vec::<Vec<u8>>::new());
    assert!(decoder.end().is_ok());
}

#[test]
fn rejects_a_truncated_stream_at_end() {
    for (label, wire) in [
        ("partial header", vec![0u8, 0, 0]),
        ("partial payload", vec![0, 0, 0, 2, 1]),
    ] {
        let mut decoder = decoder(None);
        assert_eq!(
            decoder.push(&wire).expect("pushes"),
            Vec::<Vec<u8>>::new(),
            "{label}"
        );
        assert!(decoder.end().is_err(), "{label}");
    }
}

#[test]
fn rejects_an_oversized_declared_length_as_soon_as_its_header_is_complete() {
    let mut decoder = decoder(Some(3));
    let error = decoder.push(&[0, 0, 0, 4]).expect_err("oversized");
    assert!(
        error.message().to_lowercase().contains("limit"),
        "{}",
        error.message()
    );
    let error = decoder.push(&[1]).expect_err("failed decoder");
    assert!(
        error.message().to_lowercase().contains("failed"),
        "{}",
        error.message()
    );
}

#[test]
fn accepts_a_frame_exactly_at_the_configured_maximum() {
    let mut decoder = decoder(Some(3));
    assert_eq!(
        decoder.push(&frame(&[1, 2, 3])).expect("pushes"),
        vec![vec![1, 2, 3]]
    );
    decoder.end().expect("ends");
}

#[test]
fn cannot_be_pushed_after_end() {
    let mut decoder = decoder(None);
    decoder.end().expect("ends");
    let error = decoder.push(&[]).expect_err("push after end");
    assert!(
        error.message().to_lowercase().contains("ended"),
        "{}",
        error.message()
    );
    let error = decoder.end().expect_err("end after end");
    assert!(
        error.message().to_lowercase().contains("ended"),
        "{}",
        error.message()
    );
}

#[test]
fn rejects_invalid_maximum_frame_length() {
    // -1, 1.5 und NaN sind als u64 nicht darstellbar (Abweichung Klasse 1).
    let error = FrameDecoder::new(Some(FrameDecoderOptions::with_max_frame_length(
        DEFAULT_MAX_FRAME_LENGTH * 1_000,
    )))
    .expect_err("oversized limit");
    assert!(
        matches!(error, notagent_protocol::FrameError::Range(_)),
        "{error:?}"
    );
}
