//! Port of `packages/tui/test/editor.test.ts` (4152 LOC).
//!
//! Deviation class 1: cursor columns are byte offsets instead of UTF-16 code
//! units (see `PARITY.md`); the ASCII cases keep the TS numbers, the Unicode
//! cases carry the byte values.

use std::rc::Rc;

use notagent_tui::components::editor::{
    Editor, EditorOptions, EditorTheme, Segment, TextChunk, word_wrap_line,
};
use notagent_tui::components::select_list::SelectListTheme;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, TuiCore};
use notagent_tui::tui_main_screen::TuiMainScreen;

/// `defaultSelectListTheme` of `test/test-themes.ts` (chalk level 3).
fn default_select_list_theme() -> SelectListTheme {
    SelectListTheme {
        selected_prefix: Rc::new(|text| format!("\x1b[34m{text}\x1b[39m")),
        selected_text: Rc::new(|text| format!("\x1b[1m{text}\x1b[22m")),
        description: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
        scroll_info: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
        no_match: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
    }
}

/// `defaultEditorTheme` of `test/test-themes.ts`.
fn default_editor_theme() -> EditorTheme {
    EditorTheme {
        border_color: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
        select_list: Rc::new(default_select_list_theme),
    }
}

/// `createTestTUI()`; the renderer is kept alive by the returned handle.
fn create_test_tui(columns: usize, rows: usize) -> (TuiMainScreen, TuiCore) {
    let tui = TuiMainScreen::new(Box::new(VirtualTerminal::new(columns, rows)));
    let core = tui.core().clone();
    (tui, core)
}

/// An editor on an 80x24 terminal.
fn editor() -> (TuiMainScreen, Editor) {
    let (tui, core) = create_test_tui(80, 24);
    (
        tui,
        Editor::new(core, default_editor_theme(), EditorOptions::default()),
    )
}

// === Prompt history navigation ===

#[test]
fn does_nothing_on_up_arrow_when_history_is_empty() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn shows_most_recent_history_entry_on_up_arrow_when_editor_is_empty() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("first prompt");
    editor.add_to_history("second prompt");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "second prompt");
}

#[test]
fn cycles_through_history_entries_on_repeated_up_arrow() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("first");
    editor.add_to_history("second");
    editor.add_to_history("third");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "third");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "second");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "first");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "first");
}

#[test]
fn jumps_to_start_before_entering_history_from_a_non_empty_draft() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("prompt");
    editor.set_text("draft");
    editor.handle_input("\x1b[D");
    editor.handle_input("\x1b[D");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "draft");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "prompt");

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "draft");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn navigates_forward_through_history_with_down_arrow() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("first");
    editor.add_to_history("second");
    editor.add_to_history("third");
    editor.set_text("draft");

    for _ in 0..4 {
        editor.handle_input("\x1b[A");
    }

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "second");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "third");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "draft");
}

#[test]
fn exits_history_mode_when_typing_a_character() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("old prompt");
    editor.handle_input("\x1b[A");
    editor.handle_input("x");
    assert_eq!(editor.get_text(), "xold prompt");
}

#[test]
fn exits_history_mode_on_set_text() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("first");
    editor.add_to_history("second");
    editor.handle_input("\x1b[A");
    editor.set_text("");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "second");
}

#[test]
fn does_not_add_empty_strings_to_history() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("");
    editor.add_to_history("   ");
    editor.add_to_history("valid");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "valid");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "valid");
}

#[test]
fn does_not_add_consecutive_duplicates_to_history() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("same");
    editor.add_to_history("same");
    editor.add_to_history("same");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "same");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "same");
}

#[test]
fn allows_non_consecutive_duplicates_in_history() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("first");
    editor.add_to_history("second");
    editor.add_to_history("first");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "first");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "second");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "first");
}

#[test]
fn uses_cursor_movement_instead_of_history_when_the_editor_has_content() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("history item");
    editor.set_text("line1\nline2");

    editor.handle_input("\x1b[A");
    editor.handle_input("X");
    assert_eq!(editor.get_text(), "line1X\nline2");
}

#[test]
fn limits_history_to_100_entries() {
    let (_tui, mut editor) = editor();
    for index in 0..105 {
        editor.add_to_history(&format!("prompt {index}"));
    }

    for _ in 0..100 {
        editor.handle_input("\x1b[A");
    }
    assert_eq!(editor.get_text(), "prompt 5");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "prompt 5");
}

#[test]
fn places_cursor_at_start_after_browsing_history_upward() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("older entry");
    editor.add_to_history("line1\nline2\nline3");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "line1\nline2\nline3");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "older entry");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn places_cursor_at_end_after_browsing_history_downward() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("older entry");
    editor.add_to_history("line1\nline2\nline3");
    editor.add_to_history("newer entry");

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "line1\nline2\nline3");
    assert_eq!(editor.get_cursor(), (2, 5));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "newer entry");
}

#[test]
fn allows_opposite_direction_cursor_movement_within_a_multiline_history_entry() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("line1\nline2\nline3");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_text(), "line1\nline2\nline3");
    assert_eq!(editor.get_cursor(), (1, 0));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "line1\nline2\nline3");
    assert_eq!(editor.get_cursor(), (0, 0));
}

// === public state accessors ===

#[test]
fn returns_cursor_position() {
    let (_tui, mut editor) = editor();
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("a");
    editor.handle_input("b");
    editor.handle_input("c");
    assert_eq!(editor.get_cursor(), (0, 3));

    editor.handle_input("\x1b[D");
    assert_eq!(editor.get_cursor(), (0, 2));
}

#[test]
fn returns_lines_as_a_defensive_copy() {
    let (_tui, mut editor) = editor();
    editor.set_text("a\nb");

    let mut lines = editor.get_lines();
    assert_eq!(lines, ["a", "b"]);

    lines[0] = "mutated".to_string();
    assert_eq!(editor.get_lines(), ["a", "b"]);
}

// === Backslash+Enter newline workaround ===

#[test]
fn inserts_backslash_immediately() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\\");
    assert_eq!(editor.get_text(), "\\");
}

#[test]
fn converts_standalone_backslash_to_newline_on_enter() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\\");
    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "\n");
}

#[test]
fn inserts_backslash_normally_when_followed_by_other_characters() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\\");
    editor.handle_input("x");
    assert_eq!(editor.get_text(), "\\x");
}

#[test]
fn does_not_trigger_newline_when_the_backslash_is_not_before_the_cursor() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\\");
    editor.handle_input("x");
    editor.handle_input("\r");
    assert_eq!(editor.take_submitted(), ["\\x"]);
}

#[test]
fn only_removes_one_backslash_when_multiple_are_present() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\\");
    editor.handle_input("\\");
    editor.handle_input("\\");
    assert_eq!(editor.get_text(), "\\\\\\");

    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "\\\\\n");
}

// === Kitty CSI-u handling ===

#[test]
fn ignores_printable_csi_u_sequences_with_unsupported_modifiers() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\x1b[99;9u");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn inserts_shifted_csi_u_letters_as_text() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\x1b[69;2u");
    assert_eq!(editor.get_text(), "E");
}

#[test]
fn inserts_shifted_modify_other_keys_letters_as_text() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\x1b[27;2;69~");
    assert_eq!(editor.get_text(), "E");
}

// === Unicode text editing behavior ===

#[test]
fn inserts_mixed_ascii_umlauts_and_emojis_as_literal_text() {
    let (_tui, mut editor) = editor();
    for character in ["H", "e", "l", "l", "o", " ", "ä", "ö", "ü", " ", "😀"] {
        editor.handle_input(character);
    }
    assert_eq!(editor.get_text(), "Hello äöü 😀");
}

#[test]
fn deletes_single_code_unit_unicode_characters_with_backspace() {
    let (_tui, mut editor) = editor();
    for character in ["ä", "ö", "ü"] {
        editor.handle_input(character);
    }
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "äö");
}

#[test]
fn deletes_multi_code_unit_emojis_with_a_single_backspace() {
    let (_tui, mut editor) = editor();
    editor.handle_input("😀");
    editor.handle_input("👍");
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "😀");
}

#[test]
fn inserts_characters_at_the_right_position_after_moving_over_umlauts() {
    let (_tui, mut editor) = editor();
    for character in ["ä", "ö", "ü"] {
        editor.handle_input(character);
    }
    editor.handle_input("\x1b[D");
    editor.handle_input("\x1b[D");
    editor.handle_input("x");
    assert_eq!(editor.get_text(), "äxöü");
}

#[test]
fn moves_the_cursor_across_multi_code_unit_emojis_with_a_single_arrow_key() {
    let (_tui, mut editor) = editor();
    for character in ["😀", "👍", "🎉"] {
        editor.handle_input(character);
    }
    editor.handle_input("\x1b[D");
    editor.handle_input("\x1b[D");
    editor.handle_input("x");
    assert_eq!(editor.get_text(), "😀x👍🎉");
}

#[test]
fn preserves_umlauts_across_line_breaks() {
    let (_tui, mut editor) = editor();
    for character in ["ä", "ö", "ü"] {
        editor.handle_input(character);
    }
    editor.handle_input("\n");
    for character in ["Ä", "Ö", "Ü"] {
        editor.handle_input(character);
    }
    assert_eq!(editor.get_text(), "äöü\nÄÖÜ");
}

#[test]
fn replaces_the_entire_document_with_unicode_text_via_set_text() {
    let (_tui, mut editor) = editor();
    editor.set_text("Hällö Wörld! 😀 äöüÄÖÜß");
    assert_eq!(editor.get_text(), "Hällö Wörld! 😀 äöüÄÖÜß");
}

#[test]
fn moves_to_document_start_on_ctrl_a_and_inserts_at_the_beginning() {
    let (_tui, mut editor) = editor();
    editor.handle_input("a");
    editor.handle_input("b");
    editor.handle_input("\x01");
    editor.handle_input("x");
    assert_eq!(editor.get_text(), "xab");
}

#[test]
fn deletes_words_with_ctrl_w_and_alt_backspace() {
    let (_tui, mut editor) = editor();

    editor.set_text("foo bar baz");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo bar ");

    editor.set_text("foo bar   ");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo ");

    editor.set_text("foo bar...");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo bar");

    editor.set_text("foo.bar");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo.");

    editor.set_text("foo:bar");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo:");

    editor.set_text("line one\nline two");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "line one\nline ");

    editor.set_text("line one\n");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "line one");

    editor.set_text("foo 😀😀 bar");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo 😀😀 ");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo ");

    editor.set_text("foo bar");
    editor.handle_input("\x1b\x7f");
    assert_eq!(editor.get_text(), "foo ");
}

#[test]
fn navigates_words_with_ctrl_left_and_ctrl_right() {
    let (_tui, mut editor) = editor();

    editor.set_text("foo bar... baz");
    editor.handle_input("\x1b[1;5D");
    assert_eq!(editor.get_cursor(), (0, 11));
    editor.handle_input("\x1b[1;5D");
    assert_eq!(editor.get_cursor(), (0, 7));
    editor.handle_input("\x1b[1;5D");
    assert_eq!(editor.get_cursor(), (0, 4));

    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 7));
    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 10));
    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 14));

    editor.set_text("   foo bar");
    editor.handle_input("\x01");
    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 6));

    editor.set_text("foo.bar baz");
    editor.handle_input("\x1b[1;5D");
    assert_eq!(editor.get_cursor(), (0, 8));
    editor.handle_input("\x1b[1;5D");
    assert_eq!(editor.get_cursor(), (0, 4));
    editor.handle_input("\x1b[1;5D");
    assert_eq!(editor.get_cursor(), (0, 3));

    editor.handle_input("\x01");
    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 3));
    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 4));
    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 7));
}

/// The TS case expects ICU's dictionary segmentation, which treats 你好 and
/// 世界 as one word each. `unicode-segmentation` implements UAX#29, which
/// breaks between Han characters — the documented residual difference of the
/// sanctioned `Intl.Segmenter` substitution (see PARITY.md). The stops at the
/// fullwidth comma, which the case is about, are identical.
#[test]
fn stops_at_fullwidth_chinese_punctuation() {
    let (_tui, mut editor) = editor();
    // 你(0-3) 好(3-6) ，(6-9) 世(9-12) 界(12-15)
    editor.set_text("你好，世界");

    let mut backward = vec![editor.get_cursor().1];
    for _ in 0..5 {
        editor.handle_input("\x1b[1;5D");
        backward.push(editor.get_cursor().1);
    }
    assert_eq!(backward, [15, 12, 9, 6, 3, 0]);

    let mut forward = vec![editor.get_cursor().1];
    for _ in 0..5 {
        editor.handle_input("\x1b[1;5C");
        forward.push(editor.get_cursor().1);
    }
    assert_eq!(forward, [0, 3, 6, 9, 12, 15]);
}

/// Same residual difference as
/// [`stops_at_fullwidth_chinese_punctuation`]: the ASCII words move exactly as
/// in TS, the Han runs step per character.
#[test]
fn handles_mixed_cjk_and_ascii_word_movement() {
    let (_tui, mut editor) = editor();
    // hello(0-5) 你(5-8) 好(8-11) ，(11-14) world(14-19) 世(19-22) 界(22-25)
    editor.set_text("hello你好，world世界");

    let mut backward = vec![editor.get_cursor().1];
    for _ in 0..6 {
        editor.handle_input("\x1b[1;5D");
        backward.push(editor.get_cursor().1);
    }
    assert_eq!(backward, [25, 22, 19, 14, 11, 8, 5]);

    let mut forward = vec![editor.get_cursor().1];
    for _ in 0..6 {
        editor.handle_input("\x1b[1;5C");
        forward.push(editor.get_cursor().1);
    }
    assert_eq!(forward, [5, 8, 11, 14, 19, 22, 25]);
}

// === Scroll indicators ===

/// `stripVTControlCharacters` of `node:util`.
fn strip_vt(text: &str) -> String {
    notagent_tui::utils::strip_terminal_sequences(text)
}

#[test]
fn keeps_truncated_scroll_indicators_within_width_and_preserves_their_color() {
    let width = 10;
    let (_tui, core) = create_test_tui(width, 24);
    let mut editor = Editor::new(
        core,
        EditorTheme {
            border_color: Rc::new(|text| format!("\x1b[35m{text}\x1b[39m")),
            select_list: Rc::new(default_select_list_theme),
        },
        EditorOptions::default(),
    );
    editor.set_text(
        &(0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    // Render once to establish the wrapping, then scroll so content stays
    // above and below the viewport.
    editor.render(width);
    for _ in 0..10 {
        editor.handle_input("\x1b[A");
    }

    let lines = editor.render(width);
    let top_border = lines[0].clone();
    let bottom_border = lines[lines.len() - 1].clone();

    assert!(strip_vt(&top_border).starts_with("─── ↑"));
    assert!(strip_vt(&bottom_border).starts_with("─── ↓"));
    assert_eq!(
        top_border,
        format!("\x1b[35m{}\x1b[39m", strip_vt(&top_border))
    );
    assert_eq!(
        bottom_border,
        format!("\x1b[35m{}\x1b[39m", strip_vt(&bottom_border))
    );
    for line in &lines {
        assert_eq!(
            notagent_tui::visible_width(line),
            width,
            "line exceeds width {width}: {line:?}"
        );
    }
}

// === Grapheme-aware text wrapping ===

#[test]
fn wraps_lines_correctly_when_text_contains_wide_emojis() {
    let (_tui, mut editor) = editor();
    let width = 20;
    editor.set_text("Hello ✅ World");
    let lines = editor.render(width);
    for line in &lines[1..lines.len() - 1] {
        assert_eq!(notagent_tui::visible_width(line), width);
    }
}

#[test]
fn wraps_long_text_with_emojis_at_the_right_positions() {
    let (_tui, mut editor) = editor();
    let width = 10;
    editor.set_text("✅✅✅✅✅✅");
    let lines = editor.render(width);
    for line in &lines[1..lines.len() - 1] {
        assert_eq!(notagent_tui::visible_width(line), width);
    }
}

#[test]
fn renders_isolated_thai_and_lao_am_clusters_without_width_drift() {
    for text in ["ำabc", "ຳabc"] {
        let (_tui, mut editor) = editor();
        let width = 8;
        editor.set_text(text);
        for line in editor.render(width) {
            assert_eq!(
                notagent_tui::visible_width(&line),
                width,
                "line width drift for {text:?}: {line}"
            );
        }
    }
}

#[test]
fn wraps_cjk_characters_correctly() {
    let (_tui, mut editor) = editor();
    let width = 10 + 1; // one column stays reserved for the cursor
    editor.set_text("日本語テスト");
    let lines = editor.render(width);

    for line in &lines[1..lines.len() - 1] {
        assert_eq!(notagent_tui::visible_width(line), width);
    }

    let content_lines: Vec<String> = lines[1..lines.len() - 1]
        .iter()
        .map(|line| strip_vt(line).trim().to_string())
        .collect();
    assert_eq!(content_lines.len(), 2);
    assert_eq!(content_lines[0], "日本語テス");
    assert_eq!(content_lines[1], "ト");
}

#[test]
fn handles_mixed_ascii_and_wide_characters_in_wrapping() {
    let (_tui, mut editor) = editor();
    let width = 15 + 1;
    editor.set_text("Test ✅ OK 日本");
    let lines = editor.render(width);

    let content_lines = &lines[1..lines.len() - 1];
    assert_eq!(content_lines.len(), 1);
    assert_eq!(notagent_tui::visible_width(&content_lines[0]), width);
}

#[test]
fn renders_the_cursor_correctly_on_wide_characters() {
    let (_tui, mut editor) = editor();
    let width = 20;
    editor.set_text("A✅B");
    let lines = editor.render(width);

    let content_line = &lines[1];
    assert!(content_line.contains("\x1b[7m"));
    assert_eq!(notagent_tui::visible_width(content_line), width);
}

#[test]
fn does_not_exceed_terminal_width_with_an_emoji_at_the_wrap_boundary() {
    let (_tui, mut editor) = editor();
    let width = 11;
    editor.set_text("0123456789✅");
    let lines = editor.render(width);
    for line in &lines[1..lines.len() - 1] {
        assert!(notagent_tui::visible_width(line) <= width);
    }
}

#[test]
fn shows_the_cursor_at_the_end_of_the_line_before_wrapping() {
    let width = 10;
    for padding_x in [0, 1] {
        let (_tui, core) = create_test_tui(width + padding_x, 24);
        let mut editor = Editor::new(
            core,
            default_editor_theme(),
            EditorOptions {
                padding_x: Some(padding_x),
                ..EditorOptions::default()
            },
        );

        // Nine characters fill the layout width exactly.
        for _ in 0..9 {
            editor.handle_input("a");
        }
        let lines = editor.render(width + padding_x);
        let content_lines = &lines[1..lines.len() - 1];
        assert_eq!(content_lines.len(), 1, "one content line before the wrap");
        assert!(
            content_lines[0].ends_with("\x1b[7m \x1b[0m"),
            "cursor at the end of the line"
        );

        editor.handle_input("a");
        let lines = editor.render(width + padding_x);
        let content_lines = &lines[1..lines.len() - 1];
        assert_eq!(content_lines.len(), 2, "wraps to two content lines");
    }
}

// === Word wrapping ===

#[test]
fn wraps_at_word_boundaries_instead_of_mid_word() {
    let (_tui, mut editor) = editor();
    let width = 40;
    editor.set_text("Hello world this is a test of word wrapping functionality");
    let lines = editor.render(width);

    let content_lines: Vec<String> = lines[1..lines.len() - 1]
        .iter()
        .map(|line| strip_vt(line).trim().to_string())
        .collect();
    assert!(!content_lines[0].ends_with('-'));
    for line in &content_lines {
        let last_char = line.trim_end().chars().next_back();
        assert!(
            last_char.is_none_or(|character| character.is_alphanumeric()
                || character == '_'
                || ".,!?;:".contains(character)),
            "line ends unexpectedly with: {last_char:?}"
        );
    }
}

#[test]
fn does_not_start_lines_with_leading_whitespace_after_word_wrap() {
    let (_tui, mut editor) = editor();
    let width = 20;
    editor.set_text("Word1 Word2 Word3 Word4 Word5 Word6");
    let lines = editor.render(width);

    for line in &lines[1..lines.len() - 1] {
        let stripped = strip_vt(line);
        if !stripped.trim_start().is_empty() {
            let trimmed_end = stripped.trim_end();
            assert!(
                !(trimmed_end.starts_with(char::is_whitespace)
                    && trimmed_end.trim_start().len() < trimmed_end.len()
                    && !trimmed_end.trim_start().is_empty()
                    && trimmed_end.starts_with(' ')),
                "line starts with unexpected whitespace: {stripped:?}"
            );
        }
    }
}

#[test]
fn breaks_long_words_at_character_level() {
    let (_tui, mut editor) = editor();
    let width = 30;
    editor.set_text("Check https://example.com/very/long/path/that/exceeds/width here");
    let lines = editor.render(width);
    for line in &lines[1..lines.len() - 1] {
        assert_eq!(notagent_tui::visible_width(line), width);
    }
}

#[test]
fn preserves_multiple_spaces_within_words_on_the_same_line() {
    let (_tui, mut editor) = editor();
    let width = 50;
    editor.set_text("Word1   Word2    Word3");
    let lines = editor.render(width);
    let content_line = strip_vt(&lines[1]).trim().to_string();
    assert!(content_line.contains("Word1   Word2"));
}

#[test]
fn handles_the_empty_string() {
    let (_tui, mut editor) = editor();
    editor.set_text("");
    assert_eq!(editor.render(40).len(), 3);
}

#[test]
fn handles_a_single_word_that_fits_exactly() {
    let (_tui, mut editor) = editor();
    let width = 10 + 1;
    editor.set_text("1234567890");
    let lines = editor.render(width);
    assert_eq!(lines.len(), 3);
    assert!(strip_vt(&lines[1]).contains("1234567890"));
}

// === wordWrapLine ===

fn chunk_texts(chunks: &[TextChunk]) -> Vec<String> {
    chunks.iter().map(|chunk| chunk.text.clone()).collect()
}

/// Build the `Intl.SegmentData[]` the TS cases pass in explicitly.
fn segments(line: &str, parts: &[&str]) -> Vec<Segment> {
    let mut index = 0;
    parts
        .iter()
        .map(|part| {
            let segment = Segment {
                index,
                segment: (*part).to_string(),
            };
            index += part.len();
            assert!(line[segment.index..].starts_with(part));
            segment
        })
        .collect()
}

#[test]
fn wraps_a_word_to_the_next_line_when_it_ends_exactly_at_the_terminal_width() {
    let chunks = word_wrap_line("hello world test", 11, None);
    assert_eq!(chunk_texts(&chunks), ["hello ", "world test"]);
}

#[test]
fn keeps_whitespace_at_the_terminal_width_boundary_on_the_same_line() {
    let chunks = word_wrap_line("hello world test", 12, None);
    assert_eq!(chunk_texts(&chunks), ["hello world ", "test"]);
}

#[test]
fn handles_an_unbreakable_word_filling_the_width_exactly_followed_by_a_space() {
    let chunks = word_wrap_line("aaaaaaaaaaaa aaaa", 12, None);
    assert_eq!(chunk_texts(&chunks), ["aaaaaaaaaaaa", " aaaa"]);
}

#[test]
fn wraps_a_word_that_fits_the_width_but_not_the_remaining_space() {
    let chunks = word_wrap_line("      aaaaaaaaaaaa", 12, None);
    assert_eq!(chunk_texts(&chunks), ["      ", "aaaaaaaaaaaa"]);
}

#[test]
fn keeps_a_word_with_multi_space_and_the_following_word_together_when_they_fit() {
    let chunks = word_wrap_line("Lorem ipsum dolor sit amet,    consectetur", 30, None);
    assert_eq!(
        chunk_texts(&chunks),
        ["Lorem ipsum dolor sit ", "amet,    consectetur"]
    );
}

#[test]
fn keeps_them_together_when_they_fill_the_width_exactly() {
    let chunks = word_wrap_line(
        "Lorem ipsum dolor sit amet,              consectetur",
        30,
        None,
    );
    assert_eq!(
        chunk_texts(&chunks),
        ["Lorem ipsum dolor sit ", "amet,              consectetur"]
    );
}

#[test]
fn splits_when_word_plus_multi_space_plus_word_exceeds_the_width() {
    let chunks = word_wrap_line(
        "Lorem ipsum dolor sit amet,               consectetur",
        30,
        None,
    );
    assert_eq!(
        chunk_texts(&chunks),
        [
            "Lorem ipsum dolor sit ",
            "amet,               ",
            "consectetur"
        ]
    );
}

#[test]
fn breaks_long_whitespace_at_the_line_boundary() {
    let chunks = word_wrap_line(
        "Lorem ipsum dolor sit amet,                         consectetur",
        30,
        None,
    );
    assert_eq!(
        chunk_texts(&chunks),
        [
            "Lorem ipsum dolor sit ",
            "amet,                         ",
            "consectetur"
        ]
    );
}

#[test]
fn breaks_long_whitespace_at_the_line_boundary_2() {
    let chunks = word_wrap_line(
        "Lorem ipsum dolor sit amet,                          consectetur",
        30,
        None,
    );
    assert_eq!(
        chunk_texts(&chunks),
        [
            "Lorem ipsum dolor sit ",
            "amet,                         ",
            " consectetur"
        ]
    );
}

#[test]
fn breaks_whitespace_spanning_full_lines() {
    let chunks = word_wrap_line(
        "Lorem ipsum dolor sit amet,                                     consectetur",
        30,
        None,
    );
    assert_eq!(
        chunk_texts(&chunks),
        [
            "Lorem ipsum dolor sit ",
            "amet,                         ",
            "            consectetur"
        ]
    );
}

#[test]
fn force_breaks_when_a_wide_char_after_a_word_boundary_wrap_still_overflows() {
    let line = format!(" {}你", "a".repeat(186));
    let chunks = word_wrap_line(&line, 187, None);

    for chunk in &chunks {
        assert!(
            notagent_tui::visible_width(&chunk.text) <= 187,
            "chunk width {} exceeds 187",
            notagent_tui::visible_width(&chunk.text)
        );
    }
    let reconstructed: String = chunks
        .iter()
        .map(|chunk| line[chunk.start_index..chunk.end_index].to_string())
        .collect();
    assert_eq!(reconstructed, line);
}

#[test]
fn splits_an_oversized_atomic_segment_across_multiple_chunks() {
    let marker = "[paste #1 +20 lines]";
    let line = format!("A{marker}B");
    let segments = segments(&line, &["A", marker, "B"]);
    let chunks = word_wrap_line(&line, 10, Some(&segments));

    for chunk in &chunks {
        assert!(notagent_tui::visible_width(&chunk.text) <= 10);
    }
    let reconstructed: String = chunks
        .iter()
        .map(|chunk| line[chunk.start_index..chunk.end_index].to_string())
        .collect();
    assert_eq!(reconstructed, line);
}

#[test]
fn splits_an_oversized_atomic_segment_at_the_start_of_the_line() {
    let marker = "[paste #1 +20 lines]";
    let line = format!("{marker}B");
    let segments = segments(&line, &[marker, "B"]);
    let chunks = word_wrap_line(&line, 10, Some(&segments));

    for chunk in &chunks {
        assert!(notagent_tui::visible_width(&chunk.text) <= 10);
    }
    assert!(chunks[chunks.len() - 1].text.contains('B'));
    let reconstructed: String = chunks
        .iter()
        .map(|chunk| line[chunk.start_index..chunk.end_index].to_string())
        .collect();
    assert_eq!(reconstructed, line);
}

#[test]
fn splits_an_oversized_atomic_segment_at_the_end_of_the_line() {
    let marker = "[paste #1 +20 lines]";
    let line = format!("A{marker}");
    let segments = segments(&line, &["A", marker]);
    let chunks = word_wrap_line(&line, 10, Some(&segments));

    for chunk in &chunks {
        assert!(notagent_tui::visible_width(&chunk.text) <= 10);
    }
    assert_eq!(chunks[0].text, "A");
    let reconstructed: String = chunks
        .iter()
        .map(|chunk| line[chunk.start_index..chunk.end_index].to_string())
        .collect();
    assert_eq!(reconstructed, line);
}

#[test]
fn splits_consecutive_oversized_atomic_segments() {
    let first = "[paste #1 +20 lines]";
    let second = "[paste #2 +30 lines]";
    let line = format!("{first}{second}");
    let segments = segments(&line, &[first, second]);
    let chunks = word_wrap_line(&line, 10, Some(&segments));

    for chunk in &chunks {
        assert!(notagent_tui::visible_width(&chunk.text) <= 10);
    }
    let reconstructed: String = chunks
        .iter()
        .map(|chunk| line[chunk.start_index..chunk.end_index].to_string())
        .collect();
    assert_eq!(reconstructed, line);
}

#[test]
fn wraps_normally_after_an_oversized_atomic_segment() {
    let marker = "[paste #1 +20 lines]";
    let line = format!("{marker} hello world");
    let segments = segments(
        &line,
        &[
            marker, " ", "h", "e", "l", "l", "o", " ", "w", "o", "r", "l", "d",
        ],
    );
    let chunks = word_wrap_line(&line, 10, Some(&segments));

    for chunk in &chunks {
        assert!(
            notagent_tui::visible_width(&chunk.text) <= 10,
            "chunk {:?} exceeds 10",
            chunk.text
        );
    }
    assert_eq!(chunks[chunks.len() - 1].text, "world");
    let reconstructed: String = chunks
        .iter()
        .map(|chunk| line[chunk.start_index..chunk.end_index].to_string())
        .collect();
    assert_eq!(reconstructed, line);
}
