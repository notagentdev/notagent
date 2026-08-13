//! Port of `packages/tui/test/input.test.ts` (647 LOC).
//!
//! The two Unicode word boundary cases record the documented segmentation
//! difference (ICU dictionary versus UAX #29, see PARITY.md); everything else
//! matches the TS expectations exactly.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::input::Input;
use notagent_tui::tui::Component;
use notagent_tui::visible_width;

/// Ctrl+A / Ctrl+E / Ctrl+W / Ctrl+Y / Ctrl+U / Ctrl+K as raw control bytes.
const CTRL_A: &str = "\x01";
const CTRL_E: &str = "\x05";
const CTRL_W: &str = "\x17";
const CTRL_Y: &str = "\x19";
const CTRL_U: &str = "\x15";
const CTRL_K: &str = "\x0b";
const ALT_Y: &str = "\x1by";
const ALT_D: &str = "\x1bd";
const RIGHT: &str = "\x1b[C";

fn press(input: &mut Input, keys: &[&str]) {
    for key in keys {
        input.handle_input(key);
    }
}

#[test]
fn submits_value_including_backslash_on_enter() {
    let submitted: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let mut input = Input::new();
    {
        let submitted = Rc::clone(&submitted);
        input.on_submit = Some(Box::new(move |value| {
            *submitted.borrow_mut() = Some(value.to_string());
        }));
    }

    press(&mut input, &["h", "e", "l", "l", "o", "\\", "\r"]);

    assert_eq!(submitted.borrow().as_deref(), Some("hello\\"));
}

#[test]
fn inserts_backslash_as_regular_character() {
    let mut input = Input::new();
    press(&mut input, &["\\", "x"]);
    assert_eq!(input.get_value(), "\\x");
}

// describe("render")

#[test]
fn does_not_overflow_with_wide_cjk_and_fullwidth_text() {
    let width = 93;
    let cases = [
        "가나다라마바사아자차카타파하 한글 텍스트가 터미널 너비를 초과하면 크래시가 발생합니다 이것은 재현용 테스트입니다",
        "これはテスト文章です。日本語のテキストが正しく表示されるかどうかを確認するためのサンプルテキストです。あいうえお",
        "这是一段测试文本，用于验证中文字符在终端中的显示宽度是否被正确计算，如果不正确就会导致用户界面崩溃的问题",
        "ＡＢＣＤＥＦＧＨＩＪＫＬＭＮＯＰＱＲＳＴＵＶＷＸＹＺ０１２３４５６７８９ａｂｃｄｅｆｇｈｉｊｋｌｍ",
    ];

    for text in cases {
        for position in ["start", "middle", "end"] {
            let mut input = Input::new();
            input.set_value(text);
            match position {
                "middle" => {
                    for _ in 0..10 {
                        input.handle_input(RIGHT);
                    }
                }
                "end" => input.handle_input(CTRL_E),
                _ => {}
            }
            for line in input.render(width) {
                assert!(
                    visible_width(&line) <= width,
                    "line overflows at {position}: {}",
                    visible_width(&line)
                );
            }
        }
    }
}

// describe("Kill ring")

#[test]
fn ctrl_w_saves_deleted_text_to_kill_ring_and_ctrl_y_yanks_it() {
    let mut input = Input::new();
    input.set_value("foo bar baz");
    press(&mut input, &[CTRL_E, CTRL_W]);
    assert_eq!(input.get_value(), "foo bar ");

    press(&mut input, &[CTRL_A, CTRL_Y]);
    assert_eq!(input.get_value(), "bazfoo bar ");
}

#[test]
fn ctrl_w_preserves_ascii_punctuation_boundaries() {
    let mut input = Input::new();
    input.set_value("foo.bar");
    press(&mut input, &[CTRL_E, CTRL_W]);
    assert_eq!(input.get_value(), "foo.");

    input.set_value("foo:bar");
    press(&mut input, &[CTRL_E, CTRL_W]);
    assert_eq!(input.get_value(), "foo:");
}

#[test]
fn ctrl_u_saves_deleted_text_to_kill_ring() {
    let mut input = Input::new();
    input.set_value("hello world");
    input.handle_input(CTRL_A);
    for _ in 0..6 {
        input.handle_input(RIGHT);
    }

    input.handle_input(CTRL_U);
    assert_eq!(input.get_value(), "world");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "hello world");
}

#[test]
fn ctrl_k_saves_deleted_text_to_kill_ring() {
    let mut input = Input::new();
    input.set_value("hello world");
    press(&mut input, &[CTRL_A, CTRL_K]);
    assert_eq!(input.get_value(), "");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "hello world");
}

#[test]
fn ctrl_y_does_nothing_when_kill_ring_is_empty() {
    let mut input = Input::new();
    input.set_value("test");
    press(&mut input, &[CTRL_E, CTRL_Y]);
    assert_eq!(input.get_value(), "test");
}

#[test]
fn alt_y_cycles_through_kill_ring_after_ctrl_y() {
    let mut input = Input::new();
    for value in ["first", "second", "third"] {
        input.set_value(value);
        press(&mut input, &[CTRL_E, CTRL_W]);
    }
    assert_eq!(input.get_value(), "");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "third");
    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "second");
    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "first");
    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "third");
}

#[test]
fn alt_y_does_nothing_if_not_preceded_by_yank() {
    let mut input = Input::new();
    input.set_value("test");
    press(&mut input, &[CTRL_E, CTRL_W]);
    input.set_value("other");
    press(&mut input, &[CTRL_E, "x"]);
    assert_eq!(input.get_value(), "otherx");

    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "otherx");
}

#[test]
fn alt_y_does_nothing_if_kill_ring_has_one_entry() {
    let mut input = Input::new();
    input.set_value("only");
    press(&mut input, &[CTRL_E, CTRL_W, CTRL_Y]);
    assert_eq!(input.get_value(), "only");

    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "only");
}

#[test]
fn consecutive_ctrl_w_accumulates_into_one_kill_ring_entry() {
    let mut input = Input::new();
    input.set_value("one two three");
    press(&mut input, &[CTRL_E, CTRL_W, CTRL_W, CTRL_W]);
    assert_eq!(input.get_value(), "");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "one two three");
}

#[test]
fn non_delete_actions_break_kill_accumulation() {
    let mut input = Input::new();
    input.set_value("foo bar baz");
    press(&mut input, &[CTRL_E, CTRL_W]);
    assert_eq!(input.get_value(), "foo bar ");

    input.handle_input("x");
    assert_eq!(input.get_value(), "foo bar x");

    input.handle_input(CTRL_W);
    assert_eq!(input.get_value(), "foo bar ");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "foo bar x");

    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "foo bar baz");
}

#[test]
fn non_yank_actions_break_alt_y_chain() {
    let mut input = Input::new();
    for value in ["first", "second"] {
        input.set_value(value);
        press(&mut input, &[CTRL_E, CTRL_W]);
    }
    input.set_value("");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "second");

    input.handle_input("x");
    assert_eq!(input.get_value(), "secondx");

    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "secondx");
}

#[test]
fn kill_ring_rotation_persists_after_cycling() {
    let mut input = Input::new();
    for value in ["first", "second", "third"] {
        input.set_value(value);
        press(&mut input, &[CTRL_E, CTRL_W]);
    }
    input.set_value("");

    press(&mut input, &[CTRL_Y, ALT_Y]);
    assert_eq!(input.get_value(), "second");

    input.handle_input("x");
    input.set_value("");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "second");
}

#[test]
fn backward_deletions_prepend_forward_deletions_append_during_accumulation() {
    let mut input = Input::new();
    input.set_value("prefix|suffix");
    input.handle_input(CTRL_A);
    for _ in 0..6 {
        input.handle_input(RIGHT);
    }

    input.handle_input(CTRL_K);
    assert_eq!(input.get_value(), "prefix");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "prefix|suffix");
}

#[test]
fn alt_d_deletes_word_forward_and_saves_to_kill_ring() {
    let mut input = Input::new();
    input.set_value("hello world test");
    input.handle_input(CTRL_A);

    input.handle_input(ALT_D);
    assert_eq!(input.get_value(), " world test");
    input.handle_input(ALT_D);
    assert_eq!(input.get_value(), " test");

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "hello world test");
}

#[test]
fn alt_d_preserves_ascii_punctuation_boundaries() {
    let mut input = Input::new();
    input.set_value("foo.bar baz");
    input.handle_input(CTRL_A);
    input.handle_input(ALT_D);
    assert_eq!(input.get_value(), ".bar baz");
    input.handle_input(ALT_D);
    assert_eq!(input.get_value(), "bar baz");
    input.handle_input(ALT_D);
    assert_eq!(input.get_value(), " baz");
}

#[test]
fn handles_yank_in_middle_of_text() {
    let mut input = Input::new();
    input.set_value("word");
    press(&mut input, &[CTRL_E, CTRL_W]);
    input.set_value("hello world");
    input.handle_input(CTRL_A);
    for _ in 0..6 {
        input.handle_input(RIGHT);
    }

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "hello wordworld");
}

#[test]
fn handles_yank_pop_in_middle_of_text() {
    let mut input = Input::new();
    input.set_value("FIRST");
    press(&mut input, &[CTRL_E, CTRL_W]);
    input.set_value("SECOND");
    press(&mut input, &[CTRL_E, CTRL_W]);

    input.set_value("hello world");
    input.handle_input(CTRL_A);
    for _ in 0..6 {
        input.handle_input(RIGHT);
    }

    input.handle_input(CTRL_Y);
    assert_eq!(input.get_value(), "hello SECONDworld");

    input.handle_input(ALT_Y);
    assert_eq!(input.get_value(), "hello FIRSTworld");
}

// describe("Undo")

#[test]
fn undo_does_nothing_when_undo_stack_is_empty() {
    let mut input = Input::new();
    input.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
    assert_eq!(input.get_value(), "");
}
