pub mod alt_screen_search;
pub mod autocomplete;
pub mod components;
pub mod editor_component;
pub mod fuzzy;
pub mod keybindings;
pub mod keys;
pub mod kill_ring;
pub mod latex;
pub mod latex_tables;
pub mod layout;
pub mod layout_node;
pub mod markdown_lexer;
pub mod native_modifiers;
pub mod node_path;
pub mod stdin_buffer;
pub mod terminal;
pub mod terminal_colors;
pub mod terminal_image;
#[cfg(feature = "test-terminal")]
pub mod test_terminal;
pub mod tui;
pub mod tui_alt_screen;
pub mod tui_main_screen;
pub mod undo_stack;
mod unicode_tables;
pub mod utils;
pub mod word_navigation;

// Autocomplete support
pub use autocomplete::{
    AbortController, AbortSignal, AppliedCompletion, AutocompleteItem, AutocompleteProvider,
    AutocompleteSuggestions, CombinedAutocompleteProvider, CommandEntry, SlashCommand,
    SuggestionOptions,
};
// Components
pub use components::box_component::BoxComponent;
pub use components::cancellable_loader::CancellableLoader;
pub use components::editor::{Editor, EditorOptions, EditorTheme, TextChunk, word_wrap_line};
pub use components::h_stack::HStack;
pub use components::image::{Image, ImageOptions, ImageTheme};
pub use components::input::Input;
pub use components::loader::{Loader, LoaderIndicatorOptions};
pub use components::markdown::{DefaultTextStyle, Markdown, MarkdownOptions, MarkdownTheme};
pub use components::scroll_view::{
    Overscroll, ScrollToOptions, ScrollView, ScrollViewOptions, ScrollViewScrollbar,
    ScrollViewState,
};
pub use components::select_list::{
    SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme,
    SelectListTruncatePrimaryContext,
};
pub use components::settings_list::{SettingItem, SettingsList, SettingsListTheme};
pub use components::spacer::Spacer;
pub use components::stack::{StackEntryOptions, StackOptions};
pub use components::text::Text;
pub use components::truncated_text::TruncatedText;
pub use components::v_stack::VStack;
// Editor component interface (for custom editors)
pub use editor_component::EditorComponent;
// Fuzzy matching
pub use fuzzy::{FuzzyMatch, fuzzy_filter, fuzzy_match};
// Keybindings
pub use keybindings::{
    KeybindingConflict, KeybindingDefinition, KeybindingsConfig, KeybindingsManager,
    TUI_KEYBINDINGS, keybinding_keys, keybindings_match, set_keybindings,
};
// Keyboard input handling
pub use keys::{
    KeyEventType, decode_kitty_printable, decode_printable_key, is_key_release, is_key_repeat,
    is_kitty_protocol_active, matches_key, parse_key, set_kitty_protocol_active,
};
// LaTeX rendering
pub use latex::{RenderLatexOptions, render_latex};
// Markdown tokenizer (replaces `marked`)
pub use markdown_lexer::{TableCell, Token, inline_tokens, lex};
// Input buffering for batch splitting
pub use stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};
// Terminal interface and implementations
pub use terminal::{
    InputHandler, ProcessTerminal, ProcessTerminalPump, PumpResult, ResizeHandler,
    SharedProcessTerminal, Terminal, TerminalPump,
};
// Terminal colors
pub use terminal_colors::{
    RgbColor, TerminalColorScheme, is_osc11_background_color_response,
    parse_osc11_background_color, parse_terminal_color_scheme_report,
};
// Terminal image support
pub use terminal_image::{
    CellDimensions, ImageDimensions, ImageProtocol, ImageRenderOptions, TerminalCapabilities,
    allocate_image_id, calculate_image_rows, delete_all_kitty_images, delete_kitty_image,
    detect_capabilities, encode_iterm2, encode_kitty, get_capabilities, get_cell_dimensions,
    get_gif_dimensions, get_image_dimensions, get_jpeg_dimensions, get_png_dimensions,
    get_webp_dimensions, hyperlink, image_fallback, render_image, reset_capabilities_cache,
    set_capabilities, set_cell_dimensions,
};
// Core TUI abstractions
pub use alt_screen_search::{
    AltScreenSearchComponent, AltScreenSearchMatch, AltScreenSearchSegment,
    find_alt_screen_search_matches, get_alt_screen_search_match_key,
};
pub use tui::{
    CURSOR_MARKER, Component, ComponentRef, Container, Focusable, OverlayAnchor, OverlayHandle,
    OverlayMargin, OverlayOptions, OverlayUnfocusOptions, RenderLoop, SizeValue, TuiCore,
    TuiInputListener, TuiInputListenerResult, TuiMode, TuiStopOptions, component_ref,
    composite_tui_line, run_until,
};
pub use tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};
pub use tui_main_screen::{TuiMainScreen, TuiMainScreenRenderState};
// Utilities
pub use utils::{
    apply_background_to_line, extract_ansi_code, extract_segments, get_grapheme_cell_range,
    get_osc8_link_at_column, is_cjk_break, is_punctuation_char, is_whitespace_char,
    normalize_terminal_output, slice_by_column, slice_with_width, strip_terminal_sequences,
    truncate_to_width, truncate_to_width_opts, visible_width, wrap_text_with_ansi,
};
// Word navigation and editor helpers
pub use kill_ring::{KillRing, KillRingPushOptions};
pub use undo_stack::UndoStack;
pub use word_navigation::{WordNavigationOptions, find_word_backward, find_word_forward};
