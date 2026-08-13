//! Differenzielle Terminal-Rendering-Engine und Komponenten.
//!
//! 1:1-Port von `packages/tui` (siehe crates/notagent-tui/PARITY.md).
//! Das Export-Set spiegelt `packages/tui/src/index.ts`.

pub mod keys;
pub mod native_modifiers;
pub mod stdin_buffer;
pub mod terminal;
#[cfg(feature = "test-terminal")]
pub mod test_terminal;
pub mod tui;
mod unicode_tables;
pub mod utils;

pub use keys::{
    KeyEventType, decode_kitty_printable, decode_printable_key, is_key_release, is_key_repeat,
    is_kitty_protocol_active, matches_key, parse_key, set_kitty_protocol_active,
};
pub use stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};
pub use terminal::{InputHandler, ResizeHandler, Terminal};
pub use tui::{CURSOR_MARKER, Component, ComponentRef, Focusable};
pub use utils::{
    apply_background_to_line, extract_ansi_code, extract_segments, get_grapheme_cell_range,
    get_osc8_link_at_column, is_cjk_break, is_punctuation_char, is_whitespace_char,
    normalize_terminal_output, slice_by_column, slice_with_width, strip_terminal_sequences,
    truncate_to_width, truncate_to_width_opts, visible_width, wrap_text_with_ansi,
};
