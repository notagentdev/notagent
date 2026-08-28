use std::rc::Rc;

use notagent_tui::autocomplete::{
    AppliedCompletion, AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions,
    CombinedAutocompleteProvider, CommandEntry, SlashCommand, SuggestionOptions,
};
use notagent_tui::components::editor::{
    Editor, EditorOptions, EditorTheme, Segment, TextChunk, word_wrap_line,
};
use notagent_tui::components::select_list::SelectListTheme;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, TuiCore};
use notagent_tui::tui_main_screen::TuiMainScreen;

fn default_select_list_theme() -> SelectListTheme {
    SelectListTheme {
        selected_prefix: Rc::new(|text| format!("\x1b[34m{text}\x1b[39m")),
        selected_text: Rc::new(|text| format!("\x1b[1m{text}\x1b[22m")),
        description: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
        scroll_info: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
        no_match: Rc::new(|text| format!("\x1b[2m{text}\x1b[22m")),
    }
}

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

/// 世界 as one word each. `unicode-segmentation` implements UAX#29, which
/// breaks between Han characters — the documented residual difference of the
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
        top_border.as_ref(),
        format!("\x1b[35m{}\x1b[39m", strip_vt(&top_border))
    );
    assert_eq!(
        bottom_border.as_ref(),
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

// === Kill ring ===

#[test]
fn ctrl_w_saves_deleted_text_and_ctrl_y_yanks_it() {
    let (_tui, mut editor) = editor();
    editor.set_text("foo bar baz");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo bar ");

    editor.handle_input("\x01");
    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "bazfoo bar ");
}

#[test]
fn ctrl_u_saves_deleted_text_to_the_kill_ring() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x15");
    assert_eq!(editor.get_text(), "world");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "hello world");
}

#[test]
fn ctrl_k_saves_deleted_text_to_the_kill_ring() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    editor.handle_input("\x0b");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "hello world");
}

#[test]
fn ctrl_y_does_nothing_when_the_kill_ring_is_empty() {
    let (_tui, mut editor) = editor();
    editor.set_text("test");
    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "test");
}

#[test]
fn alt_y_cycles_through_the_kill_ring_after_ctrl_y() {
    let (_tui, mut editor) = editor();
    for text in ["first", "second", "third"] {
        editor.set_text(text);
        editor.handle_input("\x17");
    }
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "third");
    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "second");
    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "first");
    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "third");
}

#[test]
fn alt_y_does_nothing_if_not_preceded_by_a_yank() {
    let (_tui, mut editor) = editor();
    editor.set_text("test");
    editor.handle_input("\x17");
    editor.set_text("other");

    editor.handle_input("x");
    assert_eq!(editor.get_text(), "otherx");

    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "otherx");
}

#[test]
fn alt_y_does_nothing_with_at_most_one_entry() {
    let (_tui, mut editor) = editor();
    editor.set_text("only");
    editor.handle_input("\x17");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "only");

    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "only");
}

#[test]
fn consecutive_ctrl_w_accumulates_into_one_entry() {
    let (_tui, mut editor) = editor();
    editor.set_text("one two three");
    for _ in 0..3 {
        editor.handle_input("\x17");
    }
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "one two three");
}

#[test]
fn ctrl_u_accumulates_multiline_deletes_including_newlines() {
    let (_tui, mut editor) = editor();
    editor.set_text("line1\nline2\nline3");

    editor.handle_input("\x15");
    assert_eq!(editor.get_text(), "line1\nline2\n");
    editor.handle_input("\x15");
    assert_eq!(editor.get_text(), "line1\nline2");
    editor.handle_input("\x15");
    assert_eq!(editor.get_text(), "line1\n");
    editor.handle_input("\x15");
    assert_eq!(editor.get_text(), "line1");
    editor.handle_input("\x15");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "line1\nline2\nline3");
}

#[test]
fn backward_deletions_prepend_and_forward_deletions_append() {
    let (_tui, mut editor) = editor();
    editor.set_text("prefix|suffix");
    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x0b");
    editor.handle_input("\x0b");
    assert_eq!(editor.get_text(), "prefix");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "prefix|suffix");
}

#[test]
fn non_delete_actions_break_kill_accumulation() {
    let (_tui, mut editor) = editor();
    editor.set_text("foo bar baz");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo bar ");

    editor.handle_input("x");
    assert_eq!(editor.get_text(), "foo bar x");

    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "foo bar ");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "foo bar x");

    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "foo bar baz");
}

#[test]
fn non_yank_actions_break_the_alt_y_chain() {
    let (_tui, mut editor) = editor();
    editor.set_text("first");
    editor.handle_input("\x17");
    editor.set_text("second");
    editor.handle_input("\x17");
    editor.set_text("");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "second");

    editor.handle_input("x");
    assert_eq!(editor.get_text(), "secondx");

    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "secondx");
}

#[test]
fn kill_ring_rotation_persists_after_cycling() {
    let (_tui, mut editor) = editor();
    for text in ["first", "second", "third"] {
        editor.set_text(text);
        editor.handle_input("\x17");
    }
    editor.set_text("");

    editor.handle_input("\x19");
    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "second");

    editor.handle_input("x");
    editor.set_text("");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "second");
}

#[test]
fn consecutive_deletions_across_lines_coalesce_into_one_entry() {
    let (_tui, mut editor) = editor();
    editor.set_text("1\n2\n3");

    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "1\n2\n");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "1\n2");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "1\n");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "1");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "1\n2\n3");
}

#[test]
fn ctrl_k_at_line_end_deletes_the_newline_and_coalesces() {
    let (_tui, mut editor) = editor();
    editor.set_text("");
    for character in ["a", "b", "\n", "c", "d"] {
        editor.handle_input(character);
    }
    editor.handle_input("\x1b[A");
    editor.handle_input("\x05");

    editor.handle_input("\x0b");
    assert_eq!(editor.get_text(), "abcd");

    editor.handle_input("\x0b");
    assert_eq!(editor.get_text(), "ab");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "ab\ncd");
}

#[test]
fn handles_yank_in_the_middle_of_the_text() {
    let (_tui, mut editor) = editor();
    editor.set_text("word");
    editor.handle_input("\x17");
    editor.set_text("hello world");

    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "hello wordworld");
}

#[test]
fn handles_yank_pop_in_the_middle_of_the_text() {
    let (_tui, mut editor) = editor();
    editor.set_text("FIRST");
    editor.handle_input("\x17");
    editor.set_text("SECOND");
    editor.handle_input("\x17");

    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "hello SECONDworld");

    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "hello FIRSTworld");
}

#[test]
fn multiline_yank_and_yank_pop_in_the_middle_of_the_text() {
    let (_tui, mut editor) = editor();
    editor.set_text("SINGLE");
    editor.handle_input("\x17");

    editor.set_text("A\nB");
    for _ in 0..3 {
        editor.handle_input("\x15");
    }

    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "hello A\nBworld");

    editor.handle_input("\x1by");
    assert_eq!(editor.get_text(), "hello SINGLEworld");
}

#[test]
fn alt_d_deletes_a_word_forward_and_saves_it_to_the_kill_ring() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world test");
    editor.handle_input("\x01");

    editor.handle_input("\x1bd");
    assert_eq!(editor.get_text(), " world test");

    editor.handle_input("\x1bd");
    assert_eq!(editor.get_text(), " test");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "hello world test");
}

#[test]
fn alt_d_at_the_end_of_the_line_deletes_the_newline() {
    let (_tui, mut editor) = editor();
    editor.set_text("line1\nline2");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x05");

    editor.handle_input("\x1bd");
    assert_eq!(editor.get_text(), "line1line2");

    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "line1\nline2");
}

// === Undo ===

#[test]
fn does_nothing_when_the_undo_stack_is_empty() {
    let (_tui, mut editor) = editor();
    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");
}

/// Type the characters of `text` one by one.
fn type_text(editor: &mut Editor, text: &str) {
    for character in text.chars() {
        editor.handle_input(&character.to_string());
    }
}

#[test]
fn coalesces_consecutive_word_characters_into_one_undo_unit() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello world");
    assert_eq!(editor.get_text(), "hello world");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn undoes_spaces_one_at_a_time() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello  ");
    assert_eq!(editor.get_text(), "hello  ");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello ");
    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello");
    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn undoes_newlines_and_lets_the_next_word_capture_state() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello");
    editor.handle_input("\n");
    type_text(&mut editor, "world");
    assert_eq!(editor.get_text(), "hello\nworld");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello\n");
    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello");
    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn undoes_backspace() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello");
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "hell");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello");
}

#[test]
fn undoes_forward_delete() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello");
    editor.handle_input("\x01");
    editor.handle_input("\x1b[C");
    editor.handle_input("\x1b[3~");
    assert_eq!(editor.get_text(), "hllo");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello");
}

#[test]
fn undoes_ctrl_w() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello world");
    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "hello ");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");
}

#[test]
fn undoes_ctrl_k() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello world");
    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x0b");
    assert_eq!(editor.get_text(), "hello ");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");

    editor.handle_input("|");
    assert_eq!(editor.get_text(), "hello |world");
}

#[test]
fn undoes_ctrl_u() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello world");
    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x15");
    assert_eq!(editor.get_text(), "world");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");
}

#[test]
fn undoes_yank() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello ");
    editor.handle_input("\x17");
    editor.handle_input("\x19");
    assert_eq!(editor.get_text(), "hello ");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn undoes_a_single_line_paste_atomically() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..5 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x1b[200~beep boop\x1b[201~");
    assert_eq!(editor.get_text(), "hellobeep boop world");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");

    editor.handle_input("|");
    assert_eq!(editor.get_text(), "hello| world");
}

#[test]
fn decodes_csi_u_ctrl_letter_sequences_inside_bracketed_paste() {
    let (_tui, mut editor) = editor();
    // tmux popups with extended-keys-format=csi-u re-encode `\n` in pastes.
    editor.handle_input("\x1b[200~line1\x1b[106;5uline2\x1b[106;5uline3\x1b[201~");
    assert_eq!(editor.get_text(), "line1\nline2\nline3");
}

#[test]
fn undoes_a_multiline_paste_atomically() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..5 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x1b[200~line1\nline2\nline3\x1b[201~");
    assert_eq!(editor.get_text(), "helloline1\nline2\nline3 world");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");

    editor.handle_input("|");
    assert_eq!(editor.get_text(), "hello| world");
}

#[test]
fn undoes_insert_text_at_cursor_atomically() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..5 {
        editor.handle_input("\x1b[C");
    }

    editor.insert_text_at_cursor("/tmp/image.png");
    assert_eq!(editor.get_text(), "hello/tmp/image.png world");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");

    editor.handle_input("|");
    assert_eq!(editor.get_text(), "hello| world");
}

#[test]
fn insert_text_at_cursor_handles_multiline_text() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..5 {
        editor.handle_input("\x1b[C");
    }

    editor.insert_text_at_cursor("line1\nline2\nline3");
    assert_eq!(editor.get_text(), "helloline1\nline2\nline3 world");
    assert_eq!(editor.get_cursor(), (2, 5));

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");
}

#[test]
fn insert_text_at_cursor_normalizes_crlf_and_cr_line_endings() {
    let (_tui, mut editor) = editor();
    editor.set_text("");

    editor.insert_text_at_cursor("a\r\nb\r\nc");
    assert_eq!(editor.get_text(), "a\nb\nc");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");

    editor.insert_text_at_cursor("x\ry\rz");
    assert_eq!(editor.get_text(), "x\ny\nz");
}

#[test]
fn undoes_set_text_to_the_empty_string() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello world");
    assert_eq!(editor.get_text(), "hello world");

    editor.set_text("");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");
}

#[test]
fn clears_the_undo_stack_on_submit() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello");
    editor.handle_input("\r");

    assert_eq!(editor.take_submitted(), ["hello"]);
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");
}

#[test]
fn exits_history_browsing_mode_on_undo() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("hello");
    assert_eq!(editor.get_text(), "");

    type_text(&mut editor, "world");
    assert_eq!(editor.get_text(), "world");

    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "hello");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "world");
}

#[test]
fn undo_restores_the_pre_history_state_after_multiple_navigations() {
    let (_tui, mut editor) = editor();
    editor.add_to_history("first");
    editor.add_to_history("second");
    editor.add_to_history("third");

    type_text(&mut editor, "current");
    assert_eq!(editor.get_text(), "current");

    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "third");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "second");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_text(), "first");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "current");
}

#[test]
fn cursor_movement_starts_a_new_undo_unit() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello world");
    assert_eq!(editor.get_text(), "hello world");

    for _ in 0..5 {
        editor.handle_input("\x1b[D");
    }

    type_text(&mut editor, "lol");
    assert_eq!(editor.get_text(), "hello lolworld");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello world");

    editor.handle_input("|");
    assert_eq!(editor.get_text(), "hello |world");
}

#[test]
fn no_op_delete_operations_do_not_push_undo_snapshots() {
    let (_tui, mut editor) = editor();
    type_text(&mut editor, "hello");
    assert_eq!(editor.get_text(), "hello");

    editor.handle_input("\x17");
    assert_eq!(editor.get_text(), "");
    editor.handle_input("\x17");
    editor.handle_input("\x17");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "hello");
}

// === Autocomplete ===

fn apply_completion_helper(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
    item: &AutocompleteItem,
    prefix: &str,
) -> AppliedCompletion {
    let line = lines.get(cursor_line).cloned().unwrap_or_default();
    let before = &line[..cursor_col - prefix.len()];
    let after = &line[cursor_col..];
    let mut new_lines = lines.to_vec();
    new_lines[cursor_line] = format!("{before}{}{after}", item.value);
    AppliedCompletion {
        lines: new_lines,
        cursor_line,
        cursor_col: cursor_col - prefix.len() + item.value.len(),
    }
}

type SuggestFn = Box<dyn Fn(&[String], usize, usize, bool) -> Option<AutocompleteSuggestions>>;

struct MockProvider {
    trigger_characters: Vec<String>,
    calls: Rc<std::cell::Cell<usize>>,
    suggest: SuggestFn,
}

impl MockProvider {
    fn new(suggest: SuggestFn) -> (Rc<Self>, Rc<std::cell::Cell<usize>>) {
        let calls = Rc::new(std::cell::Cell::new(0));
        (
            Rc::new(Self {
                trigger_characters: Vec::new(),
                calls: calls.clone(),
                suggest,
            }),
            calls,
        )
    }

    fn with_triggers(
        trigger_characters: &[&str],
        suggest: SuggestFn,
    ) -> (Rc<Self>, Rc<std::cell::Cell<usize>>) {
        let calls = Rc::new(std::cell::Cell::new(0));
        (
            Rc::new(Self {
                trigger_characters: trigger_characters
                    .iter()
                    .map(|character| (*character).to_string())
                    .collect(),
                calls: calls.clone(),
                suggest,
            }),
            calls,
        )
    }
}

#[async_trait::async_trait(?Send)]
impl AutocompleteProvider for MockProvider {
    fn trigger_characters(&self) -> Vec<String> {
        self.trigger_characters.clone()
    }

    async fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        options: SuggestionOptions,
    ) -> Option<AutocompleteSuggestions> {
        self.calls.set(self.calls.get() + 1);
        (self.suggest)(lines, cursor_line, cursor_col, options.force)
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> AppliedCompletion {
        apply_completion_helper(lines, cursor_line, cursor_col, item, prefix)
    }
}

fn suggestion(items: &[(&str, &str)], prefix: &str) -> AutocompleteSuggestions {
    AutocompleteSuggestions {
        items: items
            .iter()
            .map(|(value, label)| AutocompleteItem::new(*value, *label))
            .collect(),
        prefix: prefix.to_string(),
    }
}

#[tokio::test]
async fn auto_applies_a_single_force_file_suggestion_without_showing_the_menu() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, force| {
        if !force {
            return None;
        }
        let prefix = &lines[0][..cursor_col];
        (prefix == "Work").then(|| suggestion(&[("Workspace/", "Workspace/")], "Work"))
    }));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "Work");
    assert_eq!(editor.get_text(), "Work");

    editor.handle_input("\t");
    editor.pump_autocomplete().await;
    assert_eq!(editor.get_text(), "Workspace/");
    assert!(!editor.is_showing_autocomplete());

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "Work");
}

#[tokio::test]
async fn shows_the_menu_when_force_file_has_multiple_suggestions() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, force| {
        if !force {
            return None;
        }
        let prefix = &lines[0][..cursor_col];
        (prefix == "src").then(|| suggestion(&[("src/", "src/"), ("src.txt", "src.txt")], "src"))
    }));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "src");
    assert_eq!(editor.get_text(), "src");

    editor.handle_input("\t");
    editor.pump_autocomplete().await;
    assert_eq!(editor.get_text(), "src");
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\t");
    assert_eq!(editor.get_text(), "src/");
    assert!(!editor.is_showing_autocomplete());
}

#[tokio::test]
async fn keeps_suggestions_open_when_typing_in_force_mode() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, force| {
        let all_files = [
            ("readme.md", "readme.md"),
            ("package.json", "package.json"),
            ("src/", "src/"),
            ("dist/", "dist/"),
        ];
        let prefix = &lines[0][..cursor_col];
        let should_match = force || prefix.contains('/') || prefix.starts_with('.');
        if !should_match {
            return None;
        }
        let filtered: Vec<(&str, &str)> = all_files
            .iter()
            .filter(|(value, _)| value.to_lowercase().starts_with(&prefix.to_lowercase()))
            .copied()
            .collect();
        (!filtered.is_empty()).then(|| suggestion(&filtered, prefix))
    }));
    editor.set_autocomplete_provider(provider);

    editor.handle_input("\t");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("r");
    editor.pump_autocomplete().await;
    assert_eq!(editor.get_text(), "r");
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("e");
    editor.pump_autocomplete().await;
    assert_eq!(editor.get_text(), "re");
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\t");
    assert_eq!(editor.get_text(), "readme.md");
    assert!(!editor.is_showing_autocomplete());
}

#[tokio::test]
async fn debounces_at_autocomplete_while_typing() {
    let (_tui, mut editor) = editor();
    let (provider, calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, _force| {
        Some(suggestion(
            &[("@main.ts", "main.ts")],
            &lines[0][..cursor_col],
        ))
    }));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "@mai");

    assert_eq!(calls.get(), 0);
    assert!(!editor.is_showing_autocomplete());

    editor.pump_autocomplete().await;

    assert_eq!(calls.get(), 1);
    assert!(editor.is_showing_autocomplete());
}

#[tokio::test]
async fn debounces_hash_autocomplete_while_typing() {
    let (_tui, mut editor) = editor();
    let (provider, calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, _force| {
        Some(suggestion(&[("#2983", "#2983")], &lines[0][..cursor_col]))
    }));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "#298");

    assert_eq!(calls.get(), 0);
    assert!(!editor.is_showing_autocomplete());

    editor.pump_autocomplete().await;

    assert_eq!(calls.get(), 1);
    assert!(editor.is_showing_autocomplete());
}

#[tokio::test]
async fn debounces_custom_trigger_characters_while_typing() {
    let (_tui, mut editor) = editor();
    let (provider, calls) = MockProvider::with_triggers(
        &["$"],
        Box::new(|lines, _line, cursor_col, _force| {
            Some(suggestion(
                &[("$skill-name", "skill-name")],
                &lines[0][..cursor_col],
            ))
        }),
    );
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "$sk");

    assert_eq!(calls.get(), 0);
    editor.pump_autocomplete().await;
    assert_eq!(calls.get(), 1);
    assert!(editor.is_showing_autocomplete());
}

#[tokio::test]
async fn re_queries_the_picker_when_the_cursor_moves_back_into_the_command_name() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, _force| {
        let before = &lines[0][..cursor_col];
        if !before.starts_with('/') {
            return None;
        }
        // Past the command name: offer arguments.
        if let Some(space_index) = before.find(' ') {
            return Some(suggestion(
                &[("repo", "repo"), ("message", "message"), ("help", "help")],
                &before[space_index + 1..],
            ));
        }
        Some(suggestion(&[("cmd", "cmd")], before))
    }));
    editor.set_autocomplete_provider(provider);

    for character in "/cmd ".chars() {
        editor.handle_input(&character.to_string());
        editor.pump_autocomplete().await;
    }
    assert_eq!(editor.get_text(), "/cmd ");
    assert!(editor.is_showing_autocomplete());
    let at_argument = editor
        .render(80)
        .iter()
        .map(|line| strip_vt(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(at_argument.contains("repo"));

    editor.handle_input("\x1b[D");
    editor.pump_autocomplete().await;

    let after_move = editor
        .render(80)
        .iter()
        .map(|line| strip_vt(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!after_move.contains("repo"));
    assert!(!after_move.contains("message"));
}

#[tokio::test]
async fn does_not_trigger_autocomplete_during_a_single_line_paste() {
    let (_tui, mut editor) = editor();
    let (provider, calls) = MockProvider::new(Box::new(|_lines, _line, _cursor_col, _force| None));
    editor.set_autocomplete_provider(provider);

    editor.handle_input("\x1b[200~look at @node_modules/react/index.js please\x1b[201~");

    assert_eq!(
        editor.get_text(),
        "look at @node_modules/react/index.js please"
    );
    assert_eq!(calls.get(), 0);
    assert!(!editor.is_showing_autocomplete());
}

#[tokio::test]
async fn undoes_autocomplete() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, _force| {
        let prefix = &lines[0][..cursor_col];
        (prefix == "di").then(|| suggestion(&[("dist/", "dist/")], "di"))
    }));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "di");
    assert_eq!(editor.get_text(), "di");

    editor.handle_input("\t");
    editor.pump_autocomplete().await;
    assert_eq!(editor.get_text(), "dist/");
    assert!(!editor.is_showing_autocomplete());

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "di");
}

#[tokio::test]
async fn resets_custom_trigger_characters_when_the_provider_changes() {
    let (_tui, mut editor) = editor();
    let (first, _first_calls) = MockProvider::with_triggers(
        &["$"],
        Box::new(|_lines, _line, _cursor_col, _force| {
            Some(suggestion(&[("$skill-name", "skill-name")], "$"))
        }),
    );
    editor.set_autocomplete_provider(first);

    let (second, calls) = MockProvider::new(Box::new(|_lines, _line, _cursor_col, _force| {
        Some(suggestion(&[("$skill-name", "skill-name")], "$"))
    }));
    editor.set_autocomplete_provider(second);

    editor.handle_input("$");
    editor.handle_input("s");
    editor.pump_autocomplete().await;

    assert_eq!(calls.get(), 0);
    assert!(!editor.is_showing_autocomplete());
}

/// in a background promise, so the in-flight state is produced by dropping the
/// pump future; further typing must then abort the signal the provider holds.
#[tokio::test]
async fn aborts_an_active_at_autocomplete_when_typing_continues() {
    let (_tui, mut editor) = editor();
    let seen_signal: Rc<std::cell::RefCell<Option<notagent_tui::autocomplete::AbortSignal>>> =
        Rc::new(std::cell::RefCell::new(None));
    let captured = seen_signal.clone();

    struct SlowProvider {
        seen_signal: Rc<std::cell::RefCell<Option<notagent_tui::autocomplete::AbortSignal>>>,
    }

    #[async_trait::async_trait(?Send)]
    impl AutocompleteProvider for SlowProvider {
        async fn get_suggestions(
            &self,
            _lines: &[String],
            _cursor_line: usize,
            _cursor_col: usize,
            options: SuggestionOptions,
        ) -> Option<AutocompleteSuggestions> {
            *self.seen_signal.borrow_mut() = Some(options.signal.clone());
            for _ in 0..100 {
                if options.signal.aborted() {
                    return None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            Some(suggestion(&[("@main.ts", "main.ts")], "@main"))
        }

        fn apply_completion(
            &self,
            lines: &[String],
            cursor_line: usize,
            cursor_col: usize,
            item: &AutocompleteItem,
            prefix: &str,
        ) -> AppliedCompletion {
            apply_completion_helper(lines, cursor_line, cursor_col, item, prefix)
        }
    }

    editor.set_autocomplete_provider(Rc::new(SlowProvider {
        seen_signal: captured,
    }));

    type_text(&mut editor, "@mai");
    // Leave the request in flight: the pump future is dropped after 60 ms.
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(60),
        editor.pump_autocomplete(),
    )
    .await;
    assert!(seen_signal.borrow().is_some(), "the request started");
    assert!(!seen_signal.borrow().as_ref().expect("signal").aborted());

    editor.handle_input("n");
    assert!(
        seen_signal.borrow().as_ref().expect("signal").aborted(),
        "typing aborts the in-flight request"
    );
}

#[tokio::test]
async fn hides_autocomplete_when_backspacing_a_slash_command_to_empty() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, _force| {
        let prefix = &lines[0][..cursor_col];
        if !prefix.starts_with('/') {
            return None;
        }
        let commands = [("/model", "model"), ("/help", "help")];
        let query = &prefix[1..];
        let filtered: Vec<(&str, &str)> = commands
            .iter()
            .filter(|(value, _)| value.starts_with(query))
            .copied()
            .collect();
        (!filtered.is_empty()).then(|| suggestion(&filtered, prefix))
    }));
    editor.set_autocomplete_provider(provider);

    editor.handle_input("/");
    editor.pump_autocomplete().await;
    assert_eq!(editor.get_text(), "/");
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\x7f");
    editor.pump_autocomplete().await;
    assert_eq!(editor.get_text(), "");
    assert!(!editor.is_showing_autocomplete());
}

/// The argument completion provider shared by the `/argtest` cases.
fn argtest_provider(
    all_arguments: &'static [(&'static str, &'static str)],
    filter: bool,
) -> SuggestFn {
    Box::new(
        move |lines: &[String], _line: usize, cursor_col: usize, _force: bool| {
            let before_cursor = &lines[0][..cursor_col];
            let rest = before_cursor.strip_prefix("/argtest")?;
            let argument_text = rest.trim_start();
            if argument_text.is_empty()
                || rest.len() == argument_text.len()
                || argument_text.contains(char::is_whitespace)
            {
                return None;
            }
            let filtered: Vec<(&str, &str)> = if filter {
                all_arguments
                    .iter()
                    .filter(|(value, _)| value.starts_with(argument_text))
                    .copied()
                    .collect()
            } else {
                all_arguments.to_vec()
            };
            (!filtered.is_empty()).then(|| suggestion(&filtered, argument_text))
        },
    )
}

#[tokio::test]
async fn applies_the_exact_typed_slash_argument_on_enter() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(argtest_provider(
        &[("one", "one"), ("two", "two"), ("three", "three")],
        true,
    ));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "/argtest two");
    assert_eq!(editor.get_text(), "/argtest two");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "/argtest two");
}

#[tokio::test]
async fn selects_the_first_prefix_match_on_enter_when_the_typed_argument_is_not_exact() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(argtest_provider(
        &[("two", "two"), ("three", "three"), ("twelve", "twelve")],
        true,
    ));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "/argtest t");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "/argtest two");
}

#[tokio::test]
async fn highlights_a_unique_prefix_match_before_the_full_exact_match() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(argtest_provider(
        &[("one", "one"), ("two", "two"), ("three", "three")],
        false,
    ));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "/argtest tw");
    assert_eq!(editor.get_text(), "/argtest tw");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "/argtest two");
}

#[tokio::test]
async fn selects_the_first_prefix_match_when_multiple_items_match() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(argtest_provider(
        &[("one", "one"), ("two", "two"), ("three", "three")],
        false,
    ));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "/argtest t");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "/argtest two");
}

#[tokio::test]
async fn works_for_the_built_in_style_command_argument_completion_path() {
    let (_tui, mut editor) = editor();
    let (provider, _calls) = MockProvider::new(Box::new(|lines, _line, cursor_col, _force| {
        let before_cursor = &lines[0][..cursor_col];
        let rest = before_cursor.strip_prefix("/model")?;
        let model_text = rest.trim_start();
        if model_text.is_empty()
            || rest.len() == model_text.len()
            || model_text.contains(char::is_whitespace)
        {
            return None;
        }
        let all_models = [
            ("gpt-4o", "gpt-4o"),
            ("gpt-4o-mini", "gpt-4o-mini"),
            ("claude-sonnet", "claude-sonnet"),
        ];
        let filtered: Vec<(&str, &str)> = all_models
            .iter()
            .filter(|(value, _)| value.starts_with(model_text))
            .copied()
            .collect();
        (!filtered.is_empty()).then(|| suggestion(&filtered, model_text))
    }));
    editor.set_autocomplete_provider(provider);

    type_text(&mut editor, "/model gpt-4o-mini");
    assert_eq!(editor.get_text(), "/model gpt-4o-mini");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\r");
    assert_eq!(editor.get_text(), "/model gpt-4o-mini");
}

#[tokio::test]
async fn awaits_async_slash_command_argument_completions() {
    let (_tui, mut editor) = editor();
    let provider = CombinedAutocompleteProvider::new(
        vec![CommandEntry::Command(SlashCommand {
            name: "load-skills".to_string(),
            description: Some("Load skills".to_string()),
            argument_hint: None,
            get_argument_completions: Some(Rc::new(|prefix: &str| {
                let prefix = prefix.to_string();
                Box::pin(async move {
                    prefix
                        .starts_with('s')
                        .then(|| vec![AutocompleteItem::new("skill-a", "skill-a")])
                })
            })),
        })],
        std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .as_ref(),
        None,
    );
    editor.set_autocomplete_provider(Rc::new(provider));
    editor.set_text("/load-skills ");

    editor.handle_input("s");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\t");
    assert_eq!(editor.get_text(), "/load-skills skill-a");
    assert!(!editor.is_showing_autocomplete());
}

#[tokio::test]
async fn does_not_show_argument_completions_without_an_argument_completer() {
    let (_tui, mut editor) = editor();
    let provider = CombinedAutocompleteProvider::new(
        vec![
            CommandEntry::Command(SlashCommand {
                name: "help".to_string(),
                description: Some("Show help".to_string()),
                argument_hint: None,
                get_argument_completions: None,
            }),
            CommandEntry::Command(SlashCommand {
                name: "model".to_string(),
                description: Some("Switch model".to_string()),
                argument_hint: None,
                get_argument_completions: Some(Rc::new(|_prefix: &str| {
                    Box::pin(async move {
                        Some(vec![AutocompleteItem::new("claude-opus", "claude-opus")])
                    })
                })),
            }),
        ],
        std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .as_ref(),
        None,
    );
    editor.set_autocomplete_provider(Rc::new(provider));

    type_text(&mut editor, "/he");
    editor.pump_autocomplete().await;
    assert!(editor.is_showing_autocomplete());

    editor.handle_input("\t");
    assert_eq!(editor.get_text(), "/help ");
    assert!(!editor.is_showing_autocomplete());
}

// === Character jump (Ctrl+]) ===

#[test]
fn jumps_forward_to_the_first_occurrence_on_the_same_line() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1d");
    editor.handle_input("o");
    assert_eq!(editor.get_cursor(), (0, 4));
}

#[test]
fn jumps_forward_to_the_next_occurrence_after_the_cursor() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");
    for _ in 0..4 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 4));

    editor.handle_input("\x1d");
    editor.handle_input("o");
    assert_eq!(editor.get_cursor(), (0, 7));
}

#[test]
fn jumps_forward_across_multiple_lines() {
    let (_tui, mut editor) = editor();
    editor.set_text("abc\ndef\nghi");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1d");
    editor.handle_input("g");
    assert_eq!(editor.get_cursor(), (2, 0));
}

#[test]
fn jumps_backward_to_the_first_occurrence_before_the_cursor() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    assert_eq!(editor.get_cursor(), (0, 11));

    editor.handle_input("\x1b\x1d");
    editor.handle_input("o");
    assert_eq!(editor.get_cursor(), (0, 7));
}

#[test]
fn jumps_backward_across_multiple_lines() {
    let (_tui, mut editor) = editor();
    editor.set_text("abc\ndef\nghi");
    assert_eq!(editor.get_cursor(), (2, 3));

    editor.handle_input("\x1b\x1d");
    editor.handle_input("a");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn does_nothing_when_the_character_is_not_found_forward() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");

    editor.handle_input("\x1d");
    editor.handle_input("z");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn does_nothing_when_the_character_is_not_found_backward() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    assert_eq!(editor.get_cursor(), (0, 11));

    editor.handle_input("\x1b\x1d");
    editor.handle_input("z");
    assert_eq!(editor.get_cursor(), (0, 11));
}

#[test]
fn the_character_jump_is_case_sensitive() {
    let (_tui, mut editor) = editor();
    editor.set_text("Hello World");
    editor.handle_input("\x01");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1d");
    editor.handle_input("h");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1d");
    editor.handle_input("W");
    assert_eq!(editor.get_cursor(), (0, 6));
}

#[test]
fn cancels_jump_mode_when_ctrl_bracket_is_pressed_again() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");

    editor.handle_input("\x1d");
    editor.handle_input("\x1d");

    editor.handle_input("o");
    assert_eq!(editor.get_text(), "ohello world");
}

#[test]
fn cancels_jump_mode_on_escape_and_processes_the_escape() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");

    editor.handle_input("\x1d");
    editor.handle_input("\x1b");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("o");
    assert_eq!(editor.get_text(), "ohello world");
}

#[test]
fn cancels_backward_jump_mode_when_pressed_again() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    assert_eq!(editor.get_cursor(), (0, 11));

    editor.handle_input("\x1b\x1d");
    editor.handle_input("\x1b\x1d");

    editor.handle_input("o");
    assert_eq!(editor.get_text(), "hello worldo");
}

#[test]
fn searches_for_special_characters() {
    let (_tui, mut editor) = editor();
    editor.set_text("foo(bar) = baz;");
    editor.handle_input("\x01");

    editor.handle_input("\x1d");
    editor.handle_input("(");
    assert_eq!(editor.get_cursor(), (0, 3));

    editor.handle_input("\x1d");
    editor.handle_input("=");
    assert_eq!(editor.get_cursor(), (0, 9));
}

#[test]
fn handles_empty_text_gracefully() {
    let (_tui, mut editor) = editor();
    editor.set_text("");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1d");
    editor.handle_input("x");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn resets_the_last_action_when_jumping() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world");
    editor.handle_input("\x01");

    editor.handle_input("x");
    assert_eq!(editor.get_text(), "xhello world");

    editor.handle_input("\x1d");
    editor.handle_input("o");

    editor.handle_input("Y");
    assert_eq!(editor.get_text(), "xhellYo world");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "xhello world");
}

// === Sticky column ===

fn position_cursor(editor: &mut Editor, line: usize, col: usize) {
    for _ in 0..20 {
        editor.handle_input("\x1b[A");
    }
    for _ in 0..line {
        editor.handle_input("\x1b[B");
    }
    editor.handle_input("\x01");
    for _ in 0..col {
        editor.handle_input("\x1b[C");
    }
}

#[test]
fn preserves_the_target_column_when_moving_up_through_a_shorter_line() {
    let (_tui, mut editor) = editor();
    editor.set_text("2222222222x222\n\n1111111111_111111111111");

    assert_eq!(editor.get_cursor(), (2, 23));
    editor.handle_input("\x01");
    for _ in 0..10 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (2, 10));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (1, 0));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 10));
}

#[test]
fn preserves_the_target_column_when_moving_down_through_a_shorter_line() {
    let (_tui, mut editor) = editor();
    editor.set_text("1111111111_111\n\n2222222222x222222222222");

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    for _ in 0..10 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 10));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 0));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 10));
}

#[test]
fn resets_the_sticky_column_on_left_arrow() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\n\n1234567890");

    editor.handle_input("\x01");
    for _ in 0..5 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (2, 5));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 5));

    editor.handle_input("\x1b[D");
    assert_eq!(editor.get_cursor(), (0, 4));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 4));
}

#[test]
fn resets_the_sticky_column_on_right_arrow() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\n\n1234567890");

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    for _ in 0..5 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 5));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 5));

    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (2, 6));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 6));
}

#[test]
fn resets_the_sticky_column_on_typing() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\n\n1234567890");

    editor.handle_input("\x01");
    for _ in 0..8 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 8));

    editor.handle_input("X");
    assert_eq!(editor.get_cursor(), (0, 9));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 9));
}

#[test]
fn resets_the_sticky_column_on_backspace() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\n\n1234567890");

    editor.handle_input("\x01");
    for _ in 0..8 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 8));

    editor.handle_input("\x7f");
    assert_eq!(editor.get_cursor(), (0, 7));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 7));
}

#[test]
fn resets_the_sticky_column_on_ctrl_a() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\n\n1234567890");

    editor.handle_input("\x01");
    for _ in 0..8 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x1b[A");

    editor.handle_input("\x01");
    assert_eq!(editor.get_cursor(), (1, 0));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn resets_the_sticky_column_on_ctrl_e() {
    let (_tui, mut editor) = editor();
    editor.set_text("12345\n\n1234567890");

    editor.handle_input("\x01");
    for _ in 0..3 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 3));

    editor.handle_input("\x05");
    assert_eq!(editor.get_cursor(), (0, 5));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 5));
}

#[test]
fn resets_the_sticky_column_on_ctrl_left() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world\n\nhello world");

    assert_eq!(editor.get_cursor(), (2, 11));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 11));

    editor.handle_input("\x1b[1;5D");
    assert_eq!(editor.get_cursor(), (0, 6));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 6));
}

#[test]
fn resets_the_sticky_column_on_ctrl_right() {
    let (_tui, mut editor) = editor();
    editor.set_text("hello world\n\nhello world");

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 0));

    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (2, 5));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 5));
}

#[test]
fn resets_the_sticky_column_on_undo() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\n\n1234567890");

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    for _ in 0..8 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 8));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 8));

    editor.handle_input("X");
    assert_eq!(editor.get_text(), "1234567890\n\n12345678X90");
    assert_eq!(editor.get_cursor(), (2, 9));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 9));

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), "1234567890\n\n1234567890");
    assert_eq!(editor.get_cursor(), (2, 8));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 8));
}

#[test]
fn handles_multiple_consecutive_up_down_movements() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\nab\ncd\nef\n1234567890");

    editor.handle_input("\x01");
    for _ in 0..7 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (4, 7));

    for _ in 0..4 {
        editor.handle_input("\x1b[A");
    }
    assert_eq!(editor.get_cursor(), (0, 7));

    for _ in 0..4 {
        editor.handle_input("\x1b[B");
    }
    assert_eq!(editor.get_cursor(), (4, 7));
}

#[test]
fn moves_through_wrapped_visual_lines_without_getting_stuck() {
    let (_tui, core) = create_test_tui(15, 24);
    let mut editor = Editor::new(core, default_editor_theme(), EditorOptions::default());

    editor.set_text("short\n123456789012345678901234567890");
    editor.render(15);

    assert_eq!(editor.get_cursor(), (1, 30));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor().0, 1);

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor().0, 1);

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor().0, 0);
}

#[test]
fn handles_set_text_resetting_the_sticky_column() {
    let (_tui, mut editor) = editor();
    editor.set_text("1234567890\n\n1234567890");

    editor.handle_input("\x01");
    for _ in 0..8 {
        editor.handle_input("\x1b[C");
    }
    editor.handle_input("\x1b[A");

    editor.set_text("abcdefghij\n\nabcdefghij");
    assert_eq!(editor.get_cursor(), (2, 10));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 10));
}

#[test]
fn sets_the_preferred_visual_column_when_pressing_right_at_the_end_of_the_prompt() {
    let (_tui, mut editor) = editor();
    editor.set_text("111111111x1111111111\n\n333333333_");

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x05");
    assert_eq!(editor.get_cursor(), (0, 20));

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 10));

    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (2, 10));

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 10));
}

#[test]
fn handles_resizes_when_the_preferred_column_is_on_the_same_line() {
    let (_tui, mut editor) = editor();
    editor.set_text("12345678901234567890\n\n12345678901234567890");

    editor.handle_input("\x01");
    for _ in 0..15 {
        editor.handle_input("\x1b[C");
    }

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 15));

    editor.render(12);

    editor.handle_input("\x1b[B");
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor().1, 4);
}

#[test]
fn handles_resizes_when_the_preferred_column_is_on_a_different_line() {
    let (_tui, mut editor) = editor();
    editor.set_text("short\n12345678901234567890");

    editor.handle_input("\x01");
    for _ in 0..15 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (1, 15));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 5));

    editor.render(10);

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 8));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 5));

    editor.render(80);

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 15));
}

#[test]
fn rewrapped_lines_target_fits_the_current_visual_column() {
    let (_tui, mut editor) = editor();
    editor.set_text("abcdefghijklmnopqr\n123456789012345678");

    position_cursor(&mut editor, 0, 18);
    assert_eq!(editor.get_cursor(), (0, 18));

    editor.render(10);

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 8));

    editor.render(80);
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 8));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 8));
}

#[test]
fn rewrapped_lines_target_shorter_than_the_current_visual_column() {
    let (_tui, mut editor) = editor();
    editor.set_text("abcdefghijklmnopqr\n123456789012345678\nab");

    position_cursor(&mut editor, 0, 18);
    assert_eq!(editor.get_cursor(), (0, 18));

    editor.render(10);
    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 8));

    editor.render(80);

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 2));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (1, 8));
}

// === Paste marker atomic behavior ===

fn paste_with_marker(editor: &mut Editor) -> String {
    let big_content = "line\n".repeat(20).trim_end().to_string();
    editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));
    editor.get_text()
}

fn big_paste(tag: &str) -> String {
    (0..12)
        .map(|index| format!("{tag}{index}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first paste marker in `text`.
fn find_marker(text: &str) -> String {
    let start = text.find("[paste #").expect("marker present");
    let end = text[start..].find(']').expect("marker end") + start + 1;
    text[start..end].to_string()
}

#[test]
fn creates_a_paste_marker_for_large_pastes() {
    let (_tui, mut editor) = editor();
    let text = paste_with_marker(&mut editor);
    let marker = find_marker(&text);
    assert!(marker.starts_with("[paste #") && marker.ends_with(" lines]"));
}

#[test]
fn treats_the_paste_marker_as_a_single_unit_for_the_right_arrow() {
    let (_tui, mut editor) = editor();
    editor.handle_input("A");
    paste_with_marker(&mut editor);
    editor.handle_input("B");

    editor.handle_input("\x01");
    assert_eq!(editor.get_cursor(), (0, 0));

    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (0, 1));

    editor.handle_input("\x1b[C");
    let marker = find_marker(&editor.get_text());
    assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (0, 1 + marker.len() + 1));
}

#[test]
fn treats_the_paste_marker_as_a_single_unit_for_the_left_arrow() {
    let (_tui, mut editor) = editor();
    editor.handle_input("A");
    paste_with_marker(&mut editor);
    editor.handle_input("B");

    editor.handle_input("\x1b[D");
    let marker = find_marker(&editor.get_text());
    assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

    editor.handle_input("\x1b[D");
    assert_eq!(editor.get_cursor(), (0, 1));

    editor.handle_input("\x1b[D");
    assert_eq!(editor.get_cursor(), (0, 0));
}

#[test]
fn treats_the_paste_marker_as_a_single_unit_for_backspace() {
    let (_tui, mut editor) = editor();
    editor.handle_input("A");
    paste_with_marker(&mut editor);
    editor.handle_input("B");

    let marker = find_marker(&editor.get_text());

    editor.handle_input("\x01");
    editor.handle_input("\x1b[C");
    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "AB");
    assert_eq!(editor.get_cursor(), (0, 1));
}

#[test]
fn treats_the_paste_marker_as_a_single_unit_for_forward_delete() {
    let (_tui, mut editor) = editor();
    editor.handle_input("A");
    paste_with_marker(&mut editor);
    editor.handle_input("B");

    editor.handle_input("\x01");
    editor.handle_input("\x1b[C");

    editor.handle_input("\x1b[3~");
    assert_eq!(editor.get_text(), "AB");
    assert_eq!(editor.get_cursor(), (0, 1));
}

#[test]
fn treats_the_paste_marker_as_a_single_unit_for_word_movement() {
    let (_tui, mut editor) = editor();
    editor.handle_input("X");
    editor.handle_input(" ");
    paste_with_marker(&mut editor);
    editor.handle_input(" ");
    editor.handle_input("Y");

    let marker = find_marker(&editor.get_text());

    editor.handle_input("\x01");

    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 1));

    editor.handle_input("\x1b[1;5C");
    assert_eq!(editor.get_cursor(), (0, 2 + marker.len()));
}

#[test]
fn undo_restores_the_marker_after_a_backspace_deletion() {
    let (_tui, mut editor) = editor();
    editor.handle_input("A");
    paste_with_marker(&mut editor);
    editor.handle_input("B");

    let text_before = editor.get_text();

    editor.handle_input("\x01");
    editor.handle_input("\x1b[C");
    editor.handle_input("\x1b[C");

    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "AB");

    editor.handle_input("\x1b[45;5u");
    assert_eq!(editor.get_text(), text_before);
}

#[test]
fn undo_after_a_paste_marker_deletion_restores_the_registry() {
    let (_tui, mut editor) = editor();
    let paste = big_paste("alpha");
    editor.handle_input(&format!("\x1b[200~{paste}\x1b[201~"));
    editor.handle_input("\x7f");
    editor.handle_input("\x1b[45;5u");
    editor.handle_input("\r");
    assert_eq!(editor.take_submitted(), [paste]);
}

#[test]
fn undo_after_deleting_the_first_of_two_markers_restores_both_entries() {
    let (_tui, mut editor) = editor();
    let paste_a = big_paste("alpha");
    let paste_b = big_paste("beta");
    editor.handle_input(&format!("\x1b[200~{paste_a}\x1b[201~"));
    editor.handle_input(&format!("\x1b[200~{paste_b}\x1b[201~"));
    editor.handle_input("\x01");
    editor.handle_input("\x1b[C");
    editor.handle_input("\x7f");
    editor.handle_input("\x1b[45;5u");
    editor.handle_input("\r");
    assert_eq!(editor.take_submitted(), [format!("{paste_a}{paste_b}")]);
}

#[test]
fn renumbers_the_registry_in_ascending_id_order() {
    let (_tui, mut editor) = editor();
    let paste_a = big_paste("alpha");
    let paste_b = big_paste("beta");
    let paste_c = big_paste("gamma");
    editor.handle_input(&format!("\x1b[200~{paste_a}\x1b[201~"));
    editor.handle_input("\x01");
    editor.handle_input(&format!("\x1b[200~{paste_b}\x1b[201~"));
    editor.handle_input("\x01");
    editor.handle_input(&format!("\x1b[200~{paste_c}\x1b[201~"));
    editor.handle_input("\x05");
    editor.handle_input("\x7f");
    editor.handle_input("\r");
    assert_eq!(editor.take_submitted(), [format!("{paste_c}{paste_b}")]);
}

#[test]
fn undo_after_set_text_restores_markers_and_registry() {
    let (_tui, mut editor) = editor();
    let paste = big_paste("alpha");
    editor.handle_input(&format!("\x1b[200~{paste}\x1b[201~"));
    editor.set_text("replacement");
    editor.handle_input("\x1b[45;5u");
    editor.handle_input("\r");
    assert_eq!(editor.take_submitted(), [paste]);
}

#[test]
fn handles_multiple_paste_markers_in_the_same_line() {
    let (_tui, mut editor) = editor();
    paste_with_marker(&mut editor);
    editor.handle_input(" ");
    paste_with_marker(&mut editor);

    let text = editor.get_text();
    let first = find_marker(&text);
    let second = find_marker(&text[first.len() + 1..]);

    editor.handle_input("\x01");

    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (0, first.len()));

    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (0, first.len() + 1));

    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (0, first.len() + 1 + second.len()));
}

#[test]
fn does_not_treat_manually_typed_marker_like_text_as_atomic() {
    let (_tui, mut editor) = editor();
    let fake_marker = "[paste #99 +5 lines]";
    type_text(&mut editor, fake_marker);

    assert_eq!(editor.get_text(), fake_marker);

    editor.handle_input("\x01");
    editor.handle_input("\x1b[C");
    assert_eq!(editor.get_cursor(), (0, 1));
}

#[test]
fn does_not_crash_when_the_marker_is_wider_than_the_terminal() {
    let (_tui, mut editor) = editor();
    let big_content = "line\n".repeat(47).trim_end().to_string();
    editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));

    let marker = find_marker(&editor.get_text());
    assert!(notagent_tui::visible_width(&marker) > 8);

    for line in editor.render(8) {
        assert!(
            notagent_tui::visible_width(&line) <= 8,
            "line exceeds width 8: {line:?}"
        );
    }
}

#[test]
fn does_not_crash_when_text_plus_marker_exceeds_the_width_with_the_cursor_on_the_marker() {
    let (_tui, mut editor) = editor();
    for _ in 0..35 {
        editor.handle_input("b");
    }

    let big_content = "line\n".repeat(27).trim_end().to_string();
    editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));

    for _ in 0..4 {
        editor.handle_input("b");
    }
    for _ in 0..5 {
        editor.handle_input("\x1b[D");
    }

    let render_width = 54;
    for line in editor.render(render_width) {
        assert!(
            notagent_tui::visible_width(&line) <= render_width,
            "line exceeds width {render_width}: {line:?}"
        );
    }
}

#[test]
fn word_wrap_rechecks_overflow_after_backtracking_to_a_wrap_opportunity() {
    let (_tui, mut editor) = editor();
    editor.handle_input(" ");
    for _ in 0..35 {
        editor.handle_input("b");
    }

    let big_content = "line\n".repeat(27).trim_end().to_string();
    editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));

    for _ in 0..4 {
        editor.handle_input("b");
    }

    let render_width = 54;
    for line in editor.render(render_width) {
        assert!(
            notagent_tui::visible_width(&line) <= render_width,
            "line exceeds width {render_width}: {line:?}"
        );
    }
}

/// The multi-line paste used by the expansion cases.
fn token_paste() -> String {
    let mut lines: Vec<String> = (1..=10).map(|index| format!("line {index}")).collect();
    lines.push("tokens $1 $2 $& $$ $` $' end".to_string());
    lines.join("\n")
}

#[test]
fn expands_large_pasted_content_literally_in_get_expanded_text() {
    let (_tui, mut editor) = editor();
    let pasted_text = token_paste();
    editor.handle_input(&format!("\x1b[200~{pasted_text}\x1b[201~"));

    let marker = find_marker(&editor.get_text());
    assert!(marker.ends_with(" lines]"));
    assert_eq!(editor.get_expanded_text(), pasted_text);
}

#[test]
fn submits_large_pasted_content_literally() {
    let (_tui, mut editor) = editor();
    let pasted_text = token_paste();
    editor.handle_input(&format!("\x1b[200~{pasted_text}\x1b[201~"));
    editor.handle_input("\r");
    assert_eq!(editor.take_submitted(), [pasted_text]);
}

#[test]
fn snaps_to_the_marker_start_when_navigating_down_into_it() {
    let (_tui, mut editor) = editor();
    editor.set_text("12345678901234567890\n\nhello ");

    let big_content = "x".repeat(2000);
    editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));
    editor.render(80);

    editor.handle_input("\x1b[A");
    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    for _ in 0..10 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 10));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 0));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 6));
}

#[test]
fn preserves_the_sticky_column_when_navigating_through_a_marker_line() {
    let (_tui, core) = create_test_tui(30, 24);
    let mut editor = Editor::new(core, default_editor_theme(), EditorOptions::default());

    type_text(&mut editor, "1234567890123456");
    editor.handle_input("\n");
    editor.handle_input("\n");
    editor.handle_input(&format!("\x1b[200~{}\x1b[201~", "x".repeat(2000)));
    editor.handle_input("\n");
    editor.handle_input("\n");
    type_text(&mut editor, "abcdefghijklmnop");
    editor.render(30);

    for _ in 0..4 {
        editor.handle_input("\x1b[A");
    }
    editor.handle_input("\x01");
    for _ in 0..10 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 10));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 0));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (2, 0));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (3, 0));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (4, 10));
}

#[test]
fn does_not_get_stuck_moving_down_from_a_multi_visual_line_marker() {
    let (_tui, core) = create_test_tui(20, 24);
    let mut editor = Editor::new(core, default_editor_theme(), EditorOptions::default());

    type_text(&mut editor, "abcdefgh");
    let big_content = "line\n".repeat(100).trim_end().to_string();
    editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));
    type_text(&mut editor, "ijklmnopqr");
    editor.handle_input("\n");
    type_text(&mut editor, "123456789012345678");
    editor.render(20);

    let marker = find_marker(&editor.get_text());
    assert!(marker.len() > 20);
    let marker_start = 8;
    let marker_end = marker_start + marker.len();

    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    for _ in 0..6 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 6));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (0, marker_start));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (0, marker_end));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, marker_start));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 6));
}

#[test]
fn skips_marker_continuation_lines_when_the_preferred_column_is_in_the_tail() {
    let (_tui, core) = create_test_tui(20, 24);
    let mut editor = Editor::new(core, default_editor_theme(), EditorOptions::default());

    type_text(&mut editor, "abcdefgh");
    let big_content = "line\n".repeat(100).trim_end().to_string();
    editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));
    type_text(&mut editor, "ijklmnopqr");
    editor.handle_input("\n");
    type_text(&mut editor, "123456789012345678");
    editor.render(20);

    editor.handle_input("\x1b[A");
    editor.handle_input("\x01");
    for _ in 0..3 {
        editor.handle_input("\x1b[C");
    }
    assert_eq!(editor.get_cursor(), (0, 3));

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor().1, 8);

    editor.handle_input("\x1b[B");
    assert_eq!(editor.get_cursor(), (1, 3));

    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor().1, 8);
    editor.handle_input("\x1b[A");
    assert_eq!(editor.get_cursor(), (0, 3));
}

// === Atomic markers ===

/// A marker stands for content the editor does not hold — a pasted image lives
/// with the caller, which resolves it by matching the exact string. Half a
/// marker matches nothing, so the whole thing has to delete at once.
#[test]
fn deletes_a_registered_marker_in_one_backspace() {
    let (_tui, mut editor) = editor();
    editor.set_atomic_markers(vec!["[Image #1]".to_string()]);
    editor.set_text("look at [Image #1]");
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "look at ");
}

/// Without the registration it is ordinary text, and a backspace takes one
/// character. This is what the paste path looked like before.
#[test]
fn treats_an_unregistered_marker_as_plain_text() {
    let (_tui, mut editor) = editor();
    editor.set_text("look at [Image #1]");
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "look at [Image #1");
}

#[test]
fn steps_over_a_registered_marker_with_the_arrow_keys() {
    let (_tui, mut editor) = editor();
    editor.set_atomic_markers(vec!["[Image #1]".to_string()]);
    editor.set_text("[Image #1]!");
    editor.handle_input("\x01");
    editor.handle_input("\x1b[C");
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "!", "one step crossed the whole marker");
}

#[test]
fn keeps_each_of_several_markers_whole() {
    let (_tui, mut editor) = editor();
    editor.set_atomic_markers(vec!["[Image #1]".to_string(), "[Image #2]".to_string()]);
    editor.set_text("[Image #1] and [Image #2]");
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "[Image #1] and ");
    editor.handle_input("\x7f");
    assert_eq!(editor.get_text(), "[Image #1] and");
}
