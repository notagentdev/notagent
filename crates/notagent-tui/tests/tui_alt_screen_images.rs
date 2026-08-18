//! Port of the image cases of `packages/tui/test/tui-alt-screen.test.ts`.
//!
//! They live in their own test binary because they replace the global terminal
//! capabilities and the Kitty image registry, which must not race with the
//! other cases.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::h_stack::HStack;
use notagent_tui::components::image::{Image, ImageOptions, ImageTheme};
use notagent_tui::components::scroll_view::{ScrollView, ScrollViewOptions};
use notagent_tui::components::stack::{StackEntryOptions, StackOptions};
use notagent_tui::components::text::Text;
use notagent_tui::components::v_stack::VStack;
use notagent_tui::layout_node::StackBasis;
use notagent_tui::terminal_image::{
    EncodeKittyOptions, ImageDimensions, ImageProtocol, KittyImageMetadata, TerminalCapabilities,
    encode_kitty, register_kitty_image_metadata, reset_capabilities_cache, set_capabilities,
};
use notagent_tui::test_terminal::{TerminalEvent, VirtualTerminal};
use notagent_tui::tui::{
    Component, ComponentRef, Line, TuiStopOptions, component_ref, shared_lines,
};
use notagent_tui::tui_alt_screen::{TuiAltScreen, TuiAltScreenOptions};

/// Serializes the cases of this binary, which all share the global capabilities
/// and the Kitty registry. The guard is held across `.await`, so it is a plain
/// flag with a spin wait rather than a `MutexGuard`.
static CAPABILITY_LOCK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct CapabilityGuard;

impl Drop for CapabilityGuard {
    fn drop(&mut self) {
        CAPABILITY_LOCK.store(false, std::sync::atomic::Ordering::Release);
    }
}

async fn lock_capabilities() -> CapabilityGuard {
    while CAPABILITY_LOCK
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        tokio::task::yield_now().await;
    }
    CapabilityGuard
}

fn use_capabilities(images: ImageProtocol) {
    set_capabilities(TerminalCapabilities {
        images: Some(images),
        true_color: true,
        hyperlinks: true,
    });
}

/// `{ render: () => lines, invalidate: () => {} }` of the TS suite.
struct Lines(Vec<String>);

impl Component for Lines {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        shared_lines(self.0.clone())
    }

    fn invalidate(&mut self) {}
}

fn lines(lines: &[&str]) -> ComponentRef {
    component_ref(Lines(
        lines.iter().map(|line| (*line).to_string()).collect(),
    ))
}

fn writes_since(terminal: &VirtualTerminal, event_index: usize) -> String {
    terminal
        .events()
        .iter()
        .skip(event_index)
        .filter_map(|event| match event {
            TerminalEvent::Write(data) => Some(data.as_str()),
            _ => None,
        })
        .collect()
}

fn image_theme() -> ImageTheme {
    ImageTheme {
        fallback_color: Rc::new(|value| value.to_string()),
    }
}

#[tokio::test]
async fn does_not_emit_kitty_graphics_commands_or_osc133_zones_in_iterm2() {
    let _capabilities = lock_capabilities().await;
    use_capabilities(ImageProtocol::ITerm2);

    let terminal = VirtualTerminal::new(20, 3);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    tui.core().add_child(lines(&[
        "\x1b]133;B\x07\x1b]133;C\x07\x1b]133;A\x07content",
    ]));
    tui.core().add_child(component_ref(Image::new(
        "AAAA",
        "image/png",
        image_theme(),
        ImageOptions {
            filename: Some("example.png".to_string()),
            ..ImageOptions::default()
        },
        Some(ImageDimensions {
            width_px: 10,
            height_px: 10,
        }),
    )));
    tui.start();
    tui.wait_for_render().await;
    tui.stop(TuiStopOptions::default());

    let writes = terminal.get_writes();
    assert!(!writes.contains("\x1b_G"));
    assert!(!writes.contains("\x1b]133;"));
    assert!(!writes.contains("\x1b]1337;File="));
    assert!(writes.contains("[Image:"));

    reset_capabilities_cache();
}

#[tokio::test]
async fn clears_stale_iterm2_image_placements_when_they_leave_the_viewport() {
    let _capabilities = lock_capabilities().await;
    use_capabilities(ImageProtocol::ITerm2);

    let terminal = VirtualTerminal::new(20, 3);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let image_line = "\x1b]1337;File=inline=1;width=2;height=auto:AAAA\x07";
    tui.core()
        .add_child(lines(&[image_line, "", "", "after", "more", "end"]));
    tui.start();
    tui.wait_for_render().await;
    tui.scroll_to_top();
    tui.wait_for_render().await;
    let event_count = terminal.events().len();

    tui.scroll_by(1);
    tui.wait_for_render().await;
    assert!(writes_since(&terminal, event_count).contains("\x1b[2J"));
    tui.stop(TuiStopOptions::default());

    reset_capabilities_cache();
}

#[tokio::test]
async fn crops_a_kitty_image_whose_first_line_is_above_the_viewport() {
    let _capabilities = lock_capabilities().await;
    use_capabilities(ImageProtocol::Kitty);

    let terminal = VirtualTerminal::new(20, 3);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let image_id = 123;
    let image_line = encode_kitty(
        "AAAA",
        EncodeKittyOptions {
            columns: Some(2),
            rows: Some(3),
            image_id: Some(image_id),
            move_cursor: Some(false),
        },
    );
    register_kitty_image_metadata(KittyImageMetadata {
        image_id,
        columns: 2,
        rows: 3,
        width_px: 100,
        height_px: 100,
    });
    tui.core()
        .add_child(lines(&["before", &image_line, "", "", "after", "end"]));
    tui.start();
    tui.wait_for_render().await;

    assert_eq!(tui.viewport_top(), 3);
    let writes = terminal.get_writes();
    assert!(
        terminal.events().iter().any(|event| match event {
            TerminalEvent::Write(data) => data.contains("i=123") && data.contains("y=66,h=34,r=1"),
            _ => false,
        }),
        "writes: {writes:?}"
    );

    tui.stop(TuiStopOptions::default());
    reset_capabilities_cache();
}

#[tokio::test]
async fn reuses_moved_kitty_images_without_dropping_hstack_siblings() {
    let _capabilities = lock_capabilities().await;
    use_capabilities(ImageProtocol::Kitty);

    let terminal = VirtualTerminal::new(20, 6);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let label = Rc::new(RefCell::new(Text::new("left", 0, 0)));
    let header = Rc::new(RefCell::new(Text::new("header", 0, 0)));
    let image = Image::new(
        "A".repeat(8192),
        "image/png",
        image_theme(),
        ImageOptions::default(),
        Some(ImageDimensions {
            width_px: 100,
            height_px: 100,
        }),
    );

    let mut row = HStack::new(StackOptions::default());
    row.add_child_with(
        label.clone() as ComponentRef,
        StackEntryOptions {
            basis: Some(StackBasis::Size(10)),
            ..StackEntryOptions::default()
        },
    );
    row.add_child_with(
        component_ref(image),
        StackEntryOptions {
            basis: Some(StackBasis::Size(10)),
            ..StackEntryOptions::default()
        },
    );
    let mut root = VStack::new(StackOptions::default());
    root.add_child_with(
        header.clone() as ComponentRef,
        StackEntryOptions {
            basis: Some(StackBasis::Auto),
            ..StackEntryOptions::default()
        },
    );
    root.add_child_with(
        component_ref(row),
        StackEntryOptions {
            basis: Some(StackBasis::Size(4)),
            ..StackEntryOptions::default()
        },
    );
    tui.set_layout_root(Some(component_ref(root)));
    tui.start();
    tui.wait_for_render().await;
    assert!(terminal.get_writes().contains("\x1b_Ga=T"));

    let event_count = terminal.events().len();
    label.borrow_mut().set_text("changed");
    header.borrow_mut().set_text("header\nsecond");
    tui.request_render(false);
    tui.wait_for_render().await;
    let redraw_writes = writes_since(&terminal, event_count);
    let placement_index = redraw_writes.find("\x1b_Ga=p,q=2");
    assert!(redraw_writes.contains("\x1b_Ga=d,d=a,q=2\x1b\\"));
    assert!(placement_index > redraw_writes.find("changed"));
    assert!(!redraw_writes.contains("\x1b_Ga=T"));
    assert!(
        redraw_writes.len() < 2000,
        "expected placement-only redraw, got {} bytes",
        redraw_writes.len()
    );
    assert!(
        terminal
            .get_viewport()
            .iter()
            .any(|line| line.trim_end() == "changed")
    );

    tui.stop(TuiStopOptions::default());
    reset_capabilities_cache();
}

#[tokio::test]
async fn retains_recently_offscreen_kitty_images_for_placement_only_reuse() {
    let _capabilities = lock_capabilities().await;
    use_capabilities(ImageProtocol::Kitty);

    let terminal = VirtualTerminal::new(20, 1);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let image_id = 321;
    let image_line = encode_kitty(
        "AAAA",
        EncodeKittyOptions {
            columns: Some(2),
            rows: Some(1),
            image_id: Some(image_id),
            move_cursor: Some(false),
        },
    );
    register_kitty_image_metadata(KittyImageMetadata {
        image_id,
        columns: 2,
        rows: 1,
        width_px: 100,
        height_px: 50,
    });
    tui.set_layout_root(Some(component_ref(ScrollView::new(
        lines(&[&image_line, "after"]),
        ScrollViewOptions {
            primary: true,
            ..ScrollViewOptions::default()
        },
    ))));
    tui.start();
    tui.wait_for_render().await;
    assert!(terminal.get_writes().contains("\x1b_Ga=T"));

    let event_count = terminal.events().len();
    tui.scroll_by(1);
    tui.wait_for_render().await;
    tui.scroll_by(-1);
    tui.wait_for_render().await;
    let reentry_writes = writes_since(&terminal, event_count);
    assert!(reentry_writes.contains("\x1b_Ga=p,q=2"));
    assert!(!reentry_writes.contains("\x1b_Ga=T"));
    assert!(!reentry_writes.contains(&format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\")));

    tui.stop(TuiStopOptions::default());
    reset_capabilities_cache();
}

#[tokio::test]
async fn evicts_the_least_recently_visible_kitty_image_when_the_cache_is_full() {
    let _capabilities = lock_capabilities().await;
    use_capabilities(ImageProtocol::Kitty);

    let terminal = VirtualTerminal::new(20, 1);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let first_image_id = 500;
    let image_lines: Vec<String> = (0..18)
        .map(|index| {
            let image_id = first_image_id + index;
            register_kitty_image_metadata(KittyImageMetadata {
                image_id,
                columns: 2,
                rows: 1,
                width_px: 100,
                height_px: 50,
            });
            encode_kitty(
                "AAAA",
                EncodeKittyOptions {
                    columns: Some(2),
                    rows: Some(1),
                    image_id: Some(image_id),
                    move_cursor: Some(false),
                },
            )
        })
        .collect();
    let line_count = image_lines.len();
    tui.set_layout_root(Some(component_ref(ScrollView::new(
        component_ref(Lines(image_lines)),
        ScrollViewOptions {
            primary: true,
            ..ScrollViewOptions::default()
        },
    ))));
    tui.start();
    tui.wait_for_render().await;
    for _ in 1..line_count {
        tui.scroll_by(1);
        tui.wait_for_render().await;
    }
    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b_Ga=d,d=I,i={first_image_id},q=2\x1b\\"))
    );

    let event_count = terminal.events().len();
    tui.scroll_to_top();
    tui.wait_for_render().await;
    assert!(writes_since(&terminal, event_count).contains("\x1b_Ga=T"));

    tui.stop(TuiStopOptions::default());
    reset_capabilities_cache();
}

#[tokio::test]
async fn evicts_offscreen_kitty_images_when_raster_memory_exceeds_the_quota() {
    let _capabilities = lock_capabilities().await;
    use_capabilities(ImageProtocol::Kitty);

    let terminal = VirtualTerminal::new(20, 1);
    let mut tui = TuiAltScreen::new(Box::new(terminal.clone()), TuiAltScreenOptions::default());
    let first_image_id = 600;
    let image_lines: Vec<String> = (0..4)
        .map(|index| {
            let image_id = first_image_id + index;
            register_kitty_image_metadata(KittyImageMetadata {
                image_id,
                columns: 2,
                rows: 1,
                width_px: 3840,
                height_px: 2160,
            });
            encode_kitty(
                "AAAA",
                EncodeKittyOptions {
                    columns: Some(2),
                    rows: Some(1),
                    image_id: Some(image_id),
                    move_cursor: Some(false),
                },
            )
        })
        .collect();
    let line_count = image_lines.len();
    tui.set_layout_root(Some(component_ref(ScrollView::new(
        component_ref(Lines(image_lines)),
        ScrollViewOptions {
            primary: true,
            ..ScrollViewOptions::default()
        },
    ))));
    tui.start();
    tui.wait_for_render().await;
    for _ in 1..line_count {
        tui.scroll_by(1);
        tui.wait_for_render().await;
    }
    assert!(
        terminal
            .get_writes()
            .contains(&format!("\x1b_Ga=d,d=I,i={first_image_id},q=2\x1b\\"))
    );

    tui.stop(TuiStopOptions::default());
    reset_capabilities_cache();
}
