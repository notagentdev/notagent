use notagent_tui::keys::matches_key;
use notagent_tui::stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};

fn new_buffer() -> StdinBuffer {
    StdinBuffer::new(StdinBufferOptions {
        timeout: Some(10),
        escape_timeout: None,
    })
}

fn data_events(events: &[StdinEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            StdinEvent::Data(data) => Some(data.clone()),
            StdinEvent::Paste(_) => None,
        })
        .collect()
}

fn paste_events(events: &[StdinEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            StdinEvent::Paste(data) => Some(data.clone()),
            StdinEvent::Data(_) => None,
        })
        .collect()
}

#[derive(Default)]
struct Collector {
    events: Vec<StdinEvent>,
}

impl Collector {
    fn feed(&mut self, buffer: &mut StdinBuffer, data: &str) {
        self.events.extend(buffer.process(data));
    }

    fn timeout(&mut self, buffer: &mut StdinBuffer) {
        self.events.extend(buffer.flush_timeout());
    }

    fn data(&self) -> Vec<String> {
        data_events(&self.events)
    }

    fn pastes(&self) -> Vec<String> {
        paste_events(&self.events)
    }
}

// describe("Regular Characters")

#[test]
fn should_pass_through_regular_characters_immediately() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("a")), ["a"]);
}

#[test]
fn should_pass_through_multiple_regular_characters() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("abc")), ["a", "b", "c"]);
}

#[test]
fn should_handle_unicode_characters() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("hello 世界")),
        ["h", "e", "l", "l", "o", " ", "世", "界"]
    );
}

// describe("Complete Escape Sequences")

#[test]
fn should_pass_through_complete_escape_sequences() {
    for sequence in ["\x1b[<35;20;5m", "\x1b[A", "\x1b[11~", "\x1ba", "\x1bOA"] {
        let mut buffer = new_buffer();
        assert_eq!(data_events(&buffer.process(sequence)), [sequence]);
    }
}

// describe("Partial Escape Sequences")

#[test]
fn should_buffer_incomplete_mouse_sgr_sequence() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b");
    assert!(collector.data().is_empty());
    assert_eq!(buffer.get_buffer(), "\x1b");

    collector.feed(&mut buffer, "[<35");
    assert!(collector.data().is_empty());
    assert_eq!(buffer.get_buffer(), "\x1b[<35");

    collector.feed(&mut buffer, ";20;5m");
    assert_eq!(collector.data(), ["\x1b[<35;20;5m"]);
    assert_eq!(buffer.get_buffer(), "");
}

#[test]
fn should_buffer_incomplete_csi_sequence() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b[");
    assert!(collector.data().is_empty());
    collector.feed(&mut buffer, "1;");
    assert!(collector.data().is_empty());
    collector.feed(&mut buffer, "5H");
    assert_eq!(collector.data(), ["\x1b[1;5H"]);
}

#[test]
fn should_buffer_split_across_many_chunks() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    for chunk in ["\x1b", "[", "<", "3", "5", ";", "2", "0", ";", "5", "m"] {
        collector.feed(&mut buffer, chunk);
    }
    assert_eq!(collector.data(), ["\x1b[<35;20;5m"]);
}

#[test]
fn should_flush_incomplete_sequence_after_timeout() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b[<35");
    assert!(collector.data().is_empty());
    assert_eq!(buffer.pending_timeout_ms(), Some(10));

    collector.timeout(&mut buffer);
    assert_eq!(collector.data(), ["\x1b[<35"]);
}

#[test]
fn should_flush_a_lone_esc_as_escape_when_cr_arrives_after_the_timeout() {
    // Legacy-mode Alt+Enter is ESC + CR; if the transport splits the bytes
    // further apart than the timeout, ESC is flushed alone and the host sees
    // Escape instead of Alt+Enter.
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b");
    assert_eq!(buffer.pending_timeout_ms(), Some(10));
    collector.timeout(&mut buffer);
    collector.feed(&mut buffer, "\r");

    assert_eq!(collector.data(), ["\x1b", "\r"]);
    assert!(matches_key(&collector.data()[0], "escape"));
}

#[test]
fn should_merge_esc_and_cr_split_across_chunks_within_a_larger_timeout() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions {
        timeout: None,
        escape_timeout: Some(100),
    });
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b");
    // 20 ms elapse: more than the 10 ms default, less than the configured 100 ms.
    assert_eq!(buffer.pending_timeout_ms(), Some(100));
    collector.feed(&mut buffer, "\r");

    assert_eq!(collector.data(), ["\x1b\r"]);
    assert!(matches_key(&collector.data()[0], "alt+enter"));
}

#[test]
fn does_not_apply_the_sequence_timeout_to_a_lone_esc() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions {
        timeout: Some(100),
        escape_timeout: None,
    });
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b");
    // The lone ESC uses the escape timeout (10 ms), not the 100 ms sequence timeout.
    assert_eq!(buffer.pending_timeout_ms(), Some(10));
    collector.timeout(&mut buffer);
    collector.feed(&mut buffer, "\r");

    assert_eq!(collector.data(), ["\x1b", "\r"]);
    assert!(matches_key(&collector.data()[0], "escape"));
}

#[test]
fn keeps_fragmented_mouse_sequences_buffered_across_delayed_chunks_by_default() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions::default());
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b[");
    // 20 ms elapse, below the 50 ms default sequence timeout.
    assert_eq!(buffer.pending_timeout_ms(), Some(50));
    assert!(collector.data().is_empty());
    collector.feed(&mut buffer, "<65;48;39M");
    assert_eq!(collector.data(), ["\x1b[<65;48;39M"]);
    buffer.destroy();
}

// describe("Mixed Content")

#[test]
fn should_handle_characters_followed_by_escape_sequence() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("abc\x1b[A")),
        ["a", "b", "c", "\x1b[A"]
    );
}

#[test]
fn should_handle_escape_sequence_followed_by_characters() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[Aabc")),
        ["\x1b[A", "a", "b", "c"]
    );
}

#[test]
fn should_handle_multiple_complete_sequences() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[A\x1b[B\x1b[C")),
        ["\x1b[A", "\x1b[B", "\x1b[C"]
    );
}

#[test]
fn should_handle_partial_sequence_with_preceding_characters() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "abc\x1b[<35");
    assert_eq!(collector.data(), ["a", "b", "c"]);
    assert_eq!(buffer.get_buffer(), "\x1b[<35");

    collector.feed(&mut buffer, ";20;5m");
    assert_eq!(collector.data(), ["a", "b", "c", "\x1b[<35;20;5m"]);
}

// describe("Kitty Keyboard Protocol")

#[test]
fn should_handle_kitty_csi_u_press_and_release_events() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("\x1b[97u")), ["\x1b[97u"]);

    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[97;1:3u")),
        ["\x1b[97;1:3u"]
    );
}

#[test]
fn should_handle_batched_kitty_press_and_release() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[97u\x1b[97;1:3u")),
        ["\x1b[97u", "\x1b[97;1:3u"]
    );
}

#[test]
fn should_handle_multiple_batched_kitty_events() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[97u\x1b[97;1:3u\x1b[98u\x1b[98;1:3u")),
        ["\x1b[97u", "\x1b[97;1:3u", "\x1b[98u", "\x1b[98;1:3u"]
    );
}

#[test]
fn should_handle_kitty_arrow_and_functional_keys_with_event_type() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("\x1b[1;1:1A")), ["\x1b[1;1:1A"]);

    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("\x1b[3;1:3~")), ["\x1b[3;1:3~"]);
}

#[test]
fn should_split_esc_esc_csi_into_standalone_esc_and_the_csi_sequence() {
    // WezTerm regression: Escape press as raw \x1b, release as Kitty CSI-u.
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b\x1b[27;129:3u")),
        ["\x1b", "\x1b[27;129:3u"]
    );

    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b\x1b[27;1:3u")),
        ["\x1b", "\x1b[27;1:3u"]
    );
}

#[test]
fn should_still_emit_esc_esc_as_a_single_sequence_when_not_followed_by_a_new_escape() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("\x1b\x1b")), ["\x1b\x1b"]);
}

#[test]
fn should_handle_plain_characters_mixed_with_kitty_sequences() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("a\x1b[97;1:3u")),
        ["a", "\x1b[97;1:3u"]
    );
}

#[test]
fn should_drop_raw_duplicate_character_after_matching_kitty_printable_sequence() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("\x1b[224uà")), ["\x1b[224u"]);
}

#[test]
fn should_drop_raw_duplicate_character_after_matching_kitty_printable_sequence_across_chunks() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b[64u");
    collector.feed(&mut buffer, "@");
    assert_eq!(collector.data(), ["\x1b[64u"]);
}

#[test]
fn should_keep_non_matching_plain_character_after_kitty_printable_sequence() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process("\x1b[97ub")), ["\x1b[97u", "b"]);
}

#[test]
fn should_keep_raw_character_after_modified_kitty_printable_sequence() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[64;3u@")),
        ["\x1b[64;3u", "@"]
    );
}

#[test]
fn should_handle_rapid_typing_simulation_with_kitty_protocol() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[104u\x1b[104;1:3u\x1b[105u\x1b[105;1:3u")),
        ["\x1b[104u", "\x1b[104;1:3u", "\x1b[105u", "\x1b[105;1:3u"]
    );
}

// describe("Mouse Events")

#[test]
fn should_handle_mouse_press_release_and_move_events() {
    for sequence in ["\x1b[<0;10;5M", "\x1b[<0;10;5m", "\x1b[<35;20;5m"] {
        let mut buffer = new_buffer();
        assert_eq!(data_events(&buffer.process(sequence)), [sequence]);
    }
}

#[test]
fn should_handle_split_mouse_events() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    for chunk in ["\x1b[<3", "5;1", "5;", "10m"] {
        collector.feed(&mut buffer, chunk);
    }
    assert_eq!(collector.data(), ["\x1b[<35;15;10m"]);
}

#[test]
fn should_handle_multiple_mouse_events() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[<35;1;1m\x1b[<35;2;2m\x1b[<35;3;3m")),
        ["\x1b[<35;1;1m", "\x1b[<35;2;2m", "\x1b[<35;3;3m"]
    );
}

#[test]
fn should_handle_old_style_mouse_sequence() {
    let mut buffer = new_buffer();
    assert_eq!(
        data_events(&buffer.process("\x1b[M abc")),
        ["\x1b[M ab", "c"]
    );
}

#[test]
fn should_buffer_incomplete_old_style_mouse_sequence() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b[M");
    assert_eq!(buffer.get_buffer(), "\x1b[M");

    collector.feed(&mut buffer, " a");
    assert_eq!(buffer.get_buffer(), "\x1b[M a");

    collector.feed(&mut buffer, "b");
    assert_eq!(collector.data(), ["\x1b[M ab"]);
}

// describe("Edge Cases")

#[test]
fn should_handle_empty_input() {
    let mut buffer = new_buffer();
    // An empty string emits an empty data event.
    assert_eq!(data_events(&buffer.process("")), [""]);
}

#[test]
fn should_handle_lone_escape_character_with_timeout() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b");
    assert!(collector.data().is_empty());

    collector.timeout(&mut buffer);
    assert_eq!(collector.data(), ["\x1b"]);
}

#[test]
fn flushes_a_lone_escape_promptly_with_the_longer_default_sequence_timeout() {
    let mut buffer = StdinBuffer::new(StdinBufferOptions::default());
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b");
    assert_eq!(buffer.pending_timeout_ms(), Some(10));
    collector.timeout(&mut buffer);
    assert_eq!(collector.data(), ["\x1b"]);
    buffer.destroy();
}

#[test]
fn should_handle_lone_escape_character_with_explicit_flush() {
    let mut buffer = new_buffer();
    assert!(data_events(&buffer.process("\x1b")).is_empty());
    assert_eq!(buffer.flush(), ["\x1b"]);
}

#[test]
fn should_handle_buffer_input() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process_bytes(b"\x1b[A")), ["\x1b[A"]);
}

#[test]
fn a_single_high_byte_that_cannot_start_a_character_is_a_meta_key_at_once() {
    let mut buffer = new_buffer();
    assert_eq!(data_events(&buffer.process_bytes(&[0xfa])), ["\x1bz"]);
}

#[test]
fn a_single_high_byte_that_can_start_a_character_is_a_meta_key_once_nothing_continues_it() {
    let mut buffer = new_buffer();
    assert!(data_events(&buffer.process_bytes(&[0xe1])).is_empty());
    assert_eq!(buffer.pending_timeout_ms(), Some(10));
    assert_eq!(data_events(&buffer.flush_timeout()), ["\x1ba"]);

    assert!(data_events(&buffer.process_bytes(&[0xe1])).is_empty());
    assert_eq!(
        data_events(&buffer.process_bytes(b"x")),
        ["\x1ba", "x"],
        "input that does not continue the character ends the wait"
    );
}

#[test]
fn a_character_split_between_two_reads_arrives_whole() {
    let text = "Grüße 🙈 世界";
    let bytes = text.as_bytes();
    for split in 1..bytes.len() {
        let mut buffer = new_buffer();
        let mut received: Vec<String> = data_events(&buffer.process_bytes(&bytes[..split]));
        received.extend(data_events(&buffer.process_bytes(&bytes[split..])));
        received.extend(data_events(&buffer.flush_timeout()));
        assert_eq!(received.concat(), text, "split at byte {split}");
    }
}

#[test]
fn a_bracketed_paste_split_inside_a_character_keeps_the_character() {
    let pasted = "ä".repeat(3000);
    let payload = format!("\x1b[200~{pasted}\x1b[201~");
    let bytes = payload.as_bytes();
    let mut buffer = new_buffer();
    let mut events = Vec::new();
    for chunk in bytes.chunks(4095) {
        events.extend(buffer.process_bytes(chunk));
    }
    assert_eq!(paste_events(&events), [pasted]);
}

#[test]
fn should_handle_very_long_sequences() {
    let mut buffer = new_buffer();
    let long_seq = format!("\x1b[{}H", "1;".repeat(50));
    assert_eq!(
        data_events(&buffer.process(&long_seq)),
        std::slice::from_ref(&long_seq)
    );
}

// describe("Flush")

#[test]
fn should_flush_incomplete_sequences() {
    let mut buffer = new_buffer();
    buffer.process("\x1b[<35");
    assert_eq!(buffer.flush(), ["\x1b[<35"]);
    assert_eq!(buffer.get_buffer(), "");
}

#[test]
fn should_return_empty_array_if_nothing_to_flush() {
    let mut buffer = new_buffer();
    assert!(buffer.flush().is_empty());
}

// describe("Clear")

#[test]
fn should_clear_buffered_content_without_emitting() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b[<35");
    assert_eq!(buffer.get_buffer(), "\x1b[<35");

    buffer.clear();
    assert_eq!(buffer.get_buffer(), "");
    assert!(collector.data().is_empty());
}

// describe("Bracketed Paste")

#[test]
fn should_emit_paste_event_for_complete_bracketed_paste() {
    let mut buffer = new_buffer();
    let events = buffer.process("\x1b[200~hello world\x1b[201~");

    assert_eq!(paste_events(&events), ["hello world"]);
    assert!(data_events(&events).is_empty());
}

#[test]
fn should_handle_paste_arriving_in_chunks() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "\x1b[200~");
    assert!(collector.pastes().is_empty());

    collector.feed(&mut buffer, "hello ");
    assert!(collector.pastes().is_empty());

    collector.feed(&mut buffer, "world\x1b[201~");
    assert_eq!(collector.pastes(), ["hello world"]);
    assert!(collector.data().is_empty());
}

#[test]
fn should_handle_paste_with_input_before_and_after() {
    let mut buffer = new_buffer();
    let mut collector = Collector::default();

    collector.feed(&mut buffer, "a");
    collector.feed(&mut buffer, "\x1b[200~pasted\x1b[201~");
    collector.feed(&mut buffer, "b");

    assert_eq!(collector.data(), ["a", "b"]);
    assert_eq!(collector.pastes(), ["pasted"]);
}

#[test]
fn should_handle_paste_with_newlines_and_unicode() {
    let mut buffer = new_buffer();
    let events = buffer.process("\x1b[200~line1\nline2\nline3\x1b[201~");
    assert_eq!(paste_events(&events), ["line1\nline2\nline3"]);
    assert!(data_events(&events).is_empty());

    let mut buffer = new_buffer();
    let events = buffer.process("\x1b[200~Hello 世界 🎉\x1b[201~");
    assert_eq!(paste_events(&events), ["Hello 世界 🎉"]);
    assert!(data_events(&events).is_empty());
}

// describe("Destroy")

#[test]
fn should_clear_buffer_on_destroy() {
    let mut buffer = new_buffer();
    buffer.process("\x1b[<35");
    assert_eq!(buffer.get_buffer(), "\x1b[<35");

    buffer.destroy();
    assert_eq!(buffer.get_buffer(), "");
    // No pending timeout remains after destroy.
    assert_eq!(buffer.pending_timeout_ms(), None);
}
