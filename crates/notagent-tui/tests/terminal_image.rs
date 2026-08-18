//! Port of `packages/tui/test/terminal-image.test.ts` (632 LOC).
//!
//! The environment-dependent `detectCapabilities` cases run in this binary
//! only, serialized through a lock, because they mutate process globals.

use notagent_tui::components::image::{Image, ImageOptions, ImageTheme};
use notagent_tui::terminal_image::{
    CellDimensions, EncodeITerm2Options, EncodeKittyOptions, ImageDimensions, ImageProtocol,
    ImageRenderOptions, KittyImageMetadata, TerminalCapabilities, crop_kitty_image_line,
    delete_all_kitty_images, delete_all_kitty_placements, delete_kitty_image, detect_capabilities,
    encode_iterm2, encode_kitty, get_kitty_image_metadata, get_kitty_image_placement, hyperlink,
    image_fallback, is_image_line, register_kitty_image_metadata, render_image,
    reset_capabilities_cache, set_capabilities, set_cell_dimensions,
};
use notagent_tui::tui::{Component, Line};
use notagent_tui::visible_width;

// === isImageLine ===

#[test]
fn detects_iterm2_image_escape_sequences() {
    assert!(is_image_line(
        "\x1b]1337;File=size=100,100;inline=1:base64encodeddata==\x07"
    ));
    assert!(is_image_line(
        "Some text \x1b]1337;File=size=100,100;inline=1:base64data==\x07 more text"
    ));
    assert!(is_image_line(
        "Text before image...\x1b]1337;File=inline=1:verylongbase64data==...text after"
    ));
    assert!(is_image_line(
        "Regular text ending with \x1b]1337;File=inline=1:base64data==\x07"
    ));
    assert!(is_image_line("\x1b]1337;File=:\x07"));
}

#[test]
fn detects_kitty_image_escape_sequences() {
    assert!(is_image_line(
        "\x1b_Ga=T,f=100,t=f,d=base64data...\x1b\\\x1b_Gm=i=1;\x1b\\"
    ));
    assert!(is_image_line(
        "Output: \x1b_Ga=T,f=100;data...\x1b\\\x1b_Gm=i=1;\x1b\\"
    ));
    assert!(is_image_line(
        "  \x1b_Ga=T,f=100...\x1b\\\x1b_Gm=i=1;\x1b\\  "
    ));
}

#[test]
fn detects_image_sequences_in_very_long_lines() {
    let base64_chunk = "A".repeat(100);
    let long_line = format!(
        "Text prefix \x1b]1337;File=size=800,600;inline=1:{} suffix",
        base64_chunk.repeat(3000)
    );
    assert!(long_line.len() > 300_000);
    assert!(is_image_line(&long_line));
}

#[test]
fn detects_image_sequences_regardless_of_terminal_support() {
    assert!(is_image_line(
        "Read image file [image/jpeg]\x1b]1337;File=inline=1:base64data==\x07"
    ));
    assert!(is_image_line(
        "\x1b[31mError output \x1b]1337;File=inline=1:image==\x07"
    ));
    assert!(is_image_line(
        "\x1b_Ga=T,f=100:data...\x1b\\\x1b_Gm=i=1;\x1b\\\x1b[0m reset"
    ));
}

#[test]
fn does_not_detect_images_in_lines_without_them() {
    assert!(!is_image_line(
        "This is just a regular text line without any escape sequences"
    ));
    assert!(!is_image_line(
        "\x1b[31mRed text\x1b[0m and \x1b[32mgreen text\x1b[0m"
    ));
    assert!(!is_image_line("\x1b[1A\x1b[2KLine cleared and moved up"));
    assert!(!is_image_line(
        "Some text with ]1337;File but missing ESC at start"
    ));
    assert!(!is_image_line("Some text with _G but missing ESC at start"));
    assert!(!is_image_line(""));
    assert!(!is_image_line("\n"));
    assert!(!is_image_line("\n\n"));
}

#[test]
fn handles_mixed_content_scenarios() {
    assert!(is_image_line(
        "Kitty: \x1b_Ga=T...\x1b\\\x1b_Gm=i=1;\x1b\\ iTerm2: \x1b]1337;File=inline=1:data==\x07"
    ));
    assert!(is_image_line(
        "Start \x1b]1337;File=img1==\x07 middle \x1b]1337;File=img2==\x07 end"
    ));
    assert!(!is_image_line("/path/to/File_1337_backup/image.jpg"));
}

// === detectCapabilities ===

const ENV_KEYS: [&str; 13] = [
    "TERM",
    "TERM_PROGRAM",
    "TERMINAL_EMULATOR",
    "COLORTERM",
    "TMUX",
    "KITTY_WINDOW_ID",
    "GHOSTTY_RESOURCES_DIR",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "WT_SESSION",
    "CMUX_WORKSPACE_ID",
    "WARP_SESSION_ID",
    "WARP_TERMINAL_SESSION_UUID",
];

/// Serializes the cases that mutate the environment or global capabilities.
static ENVIRONMENT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_environment() -> std::sync::MutexGuard<'static, ()> {
    ENVIRONMENT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `withEnv()` of the TS suite.
///
/// Deviation class 1: `std::env::set_var` is `unsafe` in edition 2024; the
/// cases hold [`lock_environment`], which is the safety condition.
fn with_env<T>(overrides: &[(&str, &str)], body: impl FnOnce() -> T) -> T {
    let saved: Vec<(&str, Option<String>)> = ENV_KEYS
        .iter()
        .map(|key| (*key, std::env::var(key).ok()))
        .collect();
    unsafe {
        for key in ENV_KEYS {
            std::env::remove_var(key);
        }
        for (key, value) in overrides {
            std::env::set_var(key, value);
        }
    }
    let result = body();
    unsafe {
        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
    result
}

fn capabilities(overrides: &[(&str, &str)], tmux_forwards: bool) -> TerminalCapabilities {
    with_env(overrides, || detect_capabilities(&|| tmux_forwards))
}

#[test]
fn detect_capabilities_matches_the_terminal_matrix() {
    let _environment = lock_environment();

    // Unknown terminals.
    let caps = capabilities(&[], false);
    assert!(!caps.hyperlinks);
    assert_eq!(caps.images, None);

    // tmux forwards hyperlinks.
    let caps = capabilities(
        &[
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM_PROGRAM", "ghostty"),
        ],
        true,
    );
    assert!(caps.hyperlinks);
    assert_eq!(caps.images, None);

    let caps = capabilities(
        &[
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM_PROGRAM", "ghostty"),
        ],
        false,
    );
    assert!(!caps.hyperlinks);
    assert_eq!(caps.images, None);

    // TERM starting with "tmux" consults the capability probe.
    let caps = capabilities(
        &[("TERM", "tmux-256color"), ("TERM_PROGRAM", "iterm.app")],
        true,
    );
    assert!(caps.hyperlinks);
    assert_eq!(caps.images, None);
    let caps = capabilities(
        &[("TERM", "tmux-256color"), ("TERM_PROGRAM", "iterm.app")],
        false,
    );
    assert!(!caps.hyperlinks);

    // TERM starting with "screen" never gets hyperlinks.
    let caps = capabilities(&[("TERM", "screen-256color")], true);
    assert!(!caps.hyperlinks);
    assert_eq!(caps.images, None);

    assert!(capabilities(&[("TERM_PROGRAM", "ghostty")], false).hyperlinks);

    let caps = capabilities(
        &[
            ("TERM_PROGRAM", "ghostty"),
            ("CMUX_WORKSPACE_ID", "workspace"),
        ],
        false,
    );
    assert_eq!(caps.images, Some(ImageProtocol::Kitty));
    assert!(caps.hyperlinks);

    assert!(capabilities(&[("KITTY_WINDOW_ID", "1")], false).hyperlinks);
    assert!(capabilities(&[("WEZTERM_PANE", "0")], false).hyperlinks);

    for warp in [
        ("TERM_PROGRAM", "WarpTerminal"),
        ("WARP_SESSION_ID", "some-session-id"),
        (
            "WARP_TERMINAL_SESSION_UUID",
            "d0e1a2e5-7ca7-44cd-9037-ac7222011161",
        ),
    ] {
        let caps = capabilities(&[warp], false);
        assert_eq!(caps.images, Some(ImageProtocol::Kitty), "{warp:?}");
        assert!(caps.true_color, "{warp:?}");
        assert!(caps.hyperlinks, "{warp:?}");
    }

    // Warp inside tmux loses image support.
    let caps = capabilities(
        &[
            ("TERM_PROGRAM", "WarpTerminal"),
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM", "tmux-256color"),
        ],
        true,
    );
    assert_eq!(caps.images, None);
    assert!(caps.hyperlinks);

    assert!(capabilities(&[("TERM_PROGRAM", "iterm.app")], false).hyperlinks);
    assert!(capabilities(&[("TERM_PROGRAM", "vscode")], false).hyperlinks);

    let caps = capabilities(
        &[("WT_SESSION", "session"), ("TERM", "xterm-256color")],
        false,
    );
    assert!(caps.true_color);
    assert!(caps.hyperlinks);
    assert_eq!(caps.images, None);

    let caps = capabilities(
        &[
            ("TERMINAL_EMULATOR", "JetBrains-JediTerm"),
            ("TERM", "xterm-256color"),
        ],
        false,
    );
    assert!(caps.true_color);
    assert!(!caps.hyperlinks);
    assert_eq!(caps.images, None);

    // Windows Terminal truecolor is not inherited through tmux.
    let caps = capabilities(
        &[
            ("WT_SESSION", "session"),
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM", "tmux-256color"),
        ],
        false,
    );
    assert!(!caps.true_color);
    assert!(!caps.hyperlinks);
    assert_eq!(caps.images, None);

    // An explicit truecolor hint is trusted through tmux.
    let caps = capabilities(
        &[
            ("COLORTERM", "truecolor"),
            ("TMUX", "/tmp/tmux-1000/default,1234,0"),
            ("TERM", "tmux-256color"),
        ],
        false,
    );
    assert!(caps.true_color);
    assert!(!caps.hyperlinks);
    assert_eq!(caps.images, None);
}

// === Encoding ===

#[test]
fn includes_the_decoded_payload_size_in_osc1337_metadata() {
    let sequence = encode_iterm2(
        "AAAA",
        EncodeITerm2Options {
            width: Some("2".to_string()),
            height: Some("auto".to_string()),
            ..EncodeITerm2Options::default()
        },
    );
    assert_eq!(
        sequence,
        "\x1b]1337;File=inline=1;size=3;width=2;height=auto:AAAA\x07"
    );
}

#[test]
fn can_request_no_terminal_side_cursor_movement() {
    let sequence = encode_kitty(
        "AAAA",
        EncodeKittyOptions {
            columns: Some(2),
            rows: Some(2),
            move_cursor: Some(false),
            image_id: None,
        },
    );
    assert!(sequence.starts_with("\x1b_Ga=T,f=100,q=2,C=1,c=2,r=2;"));
}

#[test]
fn suppresses_kitty_replies_for_delete_commands() {
    assert_eq!(delete_kitty_image(42), "\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
    assert_eq!(delete_all_kitty_images(), "\x1b_Ga=d,d=A,q=2\x1b\\");
    assert_eq!(delete_all_kitty_placements(), "\x1b_Ga=d,d=a,q=2\x1b\\");
}

// === renderImage ===

/// Install Kitty capabilities and square cells for the duration of a case.
fn with_kitty_cells<T>(cell_size: (u32, u32), body: impl FnOnce() -> T) -> T {
    set_capabilities(TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: cell_size.0,
        height_px: cell_size.1,
    });
    let result = body();
    reset_capabilities_cache();
    set_cell_dimensions(CellDimensions {
        width_px: 9,
        height_px: 18,
    });
    result
}

#[test]
fn preserves_render_images_default_cursor_movement() {
    let _environment = lock_environment();
    with_kitty_cells((10, 10), || {
        let result = render_image(
            "AAAA",
            ImageDimensions {
                width_px: 20,
                height_px: 20,
            },
            ImageRenderOptions {
                max_width_cells: Some(2),
                ..ImageRenderOptions::default()
            },
        )
        .expect("image rendered");
        assert!(!result.sequence.contains(",C=1,"));
        assert_eq!(result.rows, 2);
    });
}

#[test]
fn can_opt_render_image_into_no_cursor_movement() {
    let _environment = lock_environment();
    with_kitty_cells((10, 10), || {
        let result = render_image(
            "AAAA",
            ImageDimensions {
                width_px: 20,
                height_px: 20,
            },
            ImageRenderOptions {
                max_width_cells: Some(2),
                move_cursor: Some(false),
                ..ImageRenderOptions::default()
            },
        )
        .expect("image rendered");
        assert!(result.sequence.contains(",C=1,"));
        assert_eq!(result.rows, 2);
    });
}

#[test]
fn registers_metadata_and_crops_a_partially_visible_placement() {
    let _environment = lock_environment();
    with_kitty_cells((10, 10), || {
        let result = render_image(
            "AAAA",
            ImageDimensions {
                width_px: 100,
                height_px: 100,
            },
            ImageRenderOptions {
                max_width_cells: Some(3),
                image_id: Some(42),
                move_cursor: Some(false),
                ..ImageRenderOptions::default()
            },
        )
        .expect("image rendered");

        assert_eq!(
            get_kitty_image_metadata(&result.sequence),
            Some(KittyImageMetadata {
                image_id: 42,
                columns: 3,
                rows: 3,
                width_px: 100,
                height_px: 100,
            })
        );
        assert!(crop_kitty_image_line(&result.sequence, 2, 1).contains("y=66,h=34,r=1"));
    });
}

#[test]
fn creates_placement_only_commands_for_uploaded_and_cropped_images() {
    let _environment = lock_environment();
    register_kitty_image_metadata(KittyImageMetadata {
        image_id: 42,
        columns: 3,
        rows: 3,
        width_px: 100,
        height_px: 100,
    });
    let transmission = encode_kitty(
        &"A".repeat(8192),
        EncodeKittyOptions {
            columns: Some(3),
            rows: Some(3),
            image_id: Some(42),
            move_cursor: Some(false),
        },
    );
    let line = format!("left {} right", crop_kitty_image_line(&transmission, 2, 1));
    let placement = get_kitty_image_placement(&line).expect("placement");

    assert_eq!(
        placement.transmission_bytes,
        line.len() - "left ".len() - " right".len()
    );
    assert_eq!(placement.estimated_decoded_bytes, 100 * 100 * 4);
    assert_eq!(
        placement.sequence,
        "\x1b_Ga=p,q=2,C=1,c=3,i=42,y=66,h=34,r=1\x1b\\"
    );
    assert_eq!(
        placement.replacement_line,
        format!("left {} right", placement.sequence)
    );
    assert!(!placement.replacement_line.contains("AAAA"));
}

#[test]
fn honors_max_height_cells_by_reducing_the_rendered_width() {
    let _environment = lock_environment();
    with_kitty_cells((10, 10), || {
        let result = render_image(
            "AAAA",
            ImageDimensions {
                width_px: 10,
                height_px: 100,
            },
            ImageRenderOptions {
                max_width_cells: Some(10),
                max_height_cells: Some(5),
                ..ImageRenderOptions::default()
            },
        )
        .expect("image rendered");
        assert_eq!(result.rows, 5);
        assert!(result.sequence.contains(",c=1,r=5"));
    });
}

#[test]
fn caps_the_image_component_height_to_a_square_pixel_box() {
    let _environment = lock_environment();
    with_kitty_cells((10, 20), || {
        let mut image = Image::new(
            "AAAA",
            "image/png",
            ImageTheme {
                fallback_color: std::rc::Rc::new(|value| value.to_string()),
            },
            ImageOptions {
                max_width_cells: Some(10),
                ..ImageOptions::default()
            },
            Some(ImageDimensions {
                width_px: 10,
                height_px: 100,
            }),
        );
        let lines = image.render(12);
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains(",c=1,r=5"));
    });
}

#[test]
fn places_the_image_sequence_on_the_first_line_with_empty_padding_rows() {
    let _environment = lock_environment();
    with_kitty_cells((10, 10), || {
        let mut image = Image::new(
            "AAAA",
            "image/png",
            ImageTheme {
                fallback_color: std::rc::Rc::new(|value| value.to_string()),
            },
            ImageOptions {
                max_width_cells: Some(2),
                ..ImageOptions::default()
            },
            Some(ImageDimensions {
                width_px: 20,
                height_px: 20,
            }),
        );
        let lines = image.render(4);
        let image_id = image.get_image_id().expect("image id");
        assert!(lines[0].starts_with("\x1b_G"));
        assert!(lines[0].contains(",C=1,"));
        assert!(lines[0].contains(&format!(",i={image_id}")));
        assert!(lines[0].ends_with("\x1b\\"));
        assert_eq!(&lines[1..], [Line::from("")]);
    });
}

#[test]
fn truncates_long_image_fallback_lines_to_the_render_width() {
    let _environment = lock_environment();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: false,
    });

    let long_path = format!(
        "{}/images/{}.png",
        notagent_tui::node_path::homedir(),
        "generated-image-with-a-very-long-absolute-path".repeat(4)
    );
    let width = 40;
    let mut image = Image::new(
        "AAAA",
        "image/png",
        ImageTheme {
            fallback_color: std::rc::Rc::new(|value| format!("\x1b[33m{value}\x1b[0m")),
        },
        ImageOptions {
            filename: Some(long_path),
            ..ImageOptions::default()
        },
        Some(ImageDimensions {
            width_px: 1280,
            height_px: 720,
        }),
    );
    let lines = image.render(width);
    assert_eq!(lines.len(), 1);
    assert!(
        visible_width(&lines[0]) <= width,
        "fallback line wider than {width}: {:?}",
        lines[0]
    );
    assert!(lines[0].contains("..."));
    assert!(lines[0].contains('~'));

    reset_capabilities_cache();
}

// === imageFallback ===

#[test]
fn shortens_home_prefixed_absolute_paths_without_hyperlinks() {
    let _environment = lock_environment();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: false,
    });

    let absolute = format!(
        "{}/.notagent/agent/shot.png",
        notagent_tui::node_path::homedir()
    );
    let result = image_fallback(
        "image/png",
        Some(ImageDimensions {
            width_px: 1280,
            height_px: 720,
        }),
        Some(&absolute),
    );
    assert_eq!(
        result,
        "[Image: ~/.notagent/agent/shot.png [image/png] 1280x720]"
    );

    reset_capabilities_cache();
}

#[test]
fn wraps_shortened_absolute_paths_in_osc8_file_links() {
    let _environment = lock_environment();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: true,
    });

    let absolute = format!(
        "{}/.notagent/agent/shot.png",
        notagent_tui::node_path::homedir()
    );
    let result = image_fallback(
        "image/png",
        Some(ImageDimensions {
            width_px: 10,
            height_px: 10,
        }),
        Some(&absolute),
    );
    assert!(result.contains("\x1b]8;;file://"));
    assert!(result.contains(&absolute.replace('\\', "/")) || result.contains(&absolute));

    let visible = strip_osc8(&result);
    assert_eq!(
        visible,
        "[Image: ~/.notagent/agent/shot.png [image/png] 10x10]"
    );

    reset_capabilities_cache();
}

/// `result.replace(/\x1b\]8;;.*?\x1b\\/g, "")`.
fn strip_osc8(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("\x1b]8;;") {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("\x1b\\") else {
            rest = &rest[start..];
            break;
        };
        rest = &rest[start + end + 2..];
    }
    result.push_str(rest);
    result
}

#[test]
fn leaves_bare_basenames_unchanged_and_does_not_hyperlink_them() {
    let _environment = lock_environment();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: true,
    });

    let result = image_fallback(
        "image/png",
        Some(ImageDimensions {
            width_px: 1,
            height_px: 1,
        }),
        Some("clankolas.png"),
    );
    assert_eq!(result, "[Image: clankolas.png [image/png] 1x1]");
    assert!(!result.contains("\x1b]8;"));

    reset_capabilities_cache();
}

#[test]
fn omits_the_filename_segment_when_not_provided() {
    let _environment = lock_environment();
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks: false,
    });

    assert_eq!(
        image_fallback(
            "image/png",
            Some(ImageDimensions {
                width_px: 8,
                height_px: 6
            }),
            None
        ),
        "[Image: [image/png] 8x6]"
    );

    reset_capabilities_cache();
}

// === hyperlink ===

#[test]
fn wraps_text_in_osc8_open_and_close_sequences() {
    assert_eq!(
        hyperlink("click me", "https://example.com"),
        "\x1b]8;;https://example.com\x1b\\click me\x1b]8;;\x1b\\"
    );
}

#[test]
fn preserves_ansi_styling_inside_the_hyperlink() {
    let styled = "\x1b[4m\x1b[34mclick me\x1b[0m";
    let result = hyperlink(styled, "https://example.com");
    assert!(result.starts_with("\x1b]8;;https://example.com\x1b\\"));
    assert!(result.contains(styled));
    assert!(result.ends_with("\x1b]8;;\x1b\\"));
}

#[test]
fn works_with_empty_text() {
    assert_eq!(
        hyperlink("", "https://example.com"),
        "\x1b]8;;https://example.com\x1b\\\x1b]8;;\x1b\\"
    );
}

#[test]
fn works_with_file_uris() {
    let result = hyperlink("README.md", "file:///home/user/README.md");
    assert!(result.contains("file:///home/user/README.md"));
    assert!(result.contains("README.md"));
}
