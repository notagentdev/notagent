use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use notagent_tui::components::alt_screen_flash::AltScreenFlashContainer;
use notagent_tui::components::box_component::BoxComponent;
use notagent_tui::components::cancellable_loader::CancellableLoader;
use notagent_tui::components::image::{Image, ImageOptions, ImageTheme};
use notagent_tui::components::loader::{Loader, LoaderIndicatorOptions};
use notagent_tui::components::spacer::Spacer;
use notagent_tui::components::text::Text;
use notagent_tui::native_modifiers::{ModifierKey, is_native_modifier_pressed};
use notagent_tui::tui::{Component, Line, component_ref};
use notagent_tui::utils::visible_width;

const ESCAPE: &str = "\x1b";

fn plain() -> Rc<dyn Fn(&str) -> String> {
    Rc::new(|text: &str| text.to_string())
}

#[test]
fn a_spacer_renders_that_many_empty_lines_whatever_the_width() {
    let mut spacer = Spacer::new(3);

    assert_eq!(spacer.render(80), vec![Line::from(""); 3]);
    assert_eq!(spacer.render(1), vec![Line::from(""); 3]);

    spacer.set_lines(0);
    assert!(spacer.render(80).is_empty());
}

#[test]
fn a_box_pads_its_children_and_stays_empty_without_them() {
    let mut boxed = BoxComponent::new(2, 1, None);
    assert!(
        boxed.render(20).is_empty(),
        "an empty box renders nothing at all, padding included"
    );

    boxed.add_child(component_ref(Text::new("hi", 0, 0)));
    let lines = boxed.render(20);

    // paddingY above and below, paddingX in front — every line padded to width.
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].as_ref(), " ".repeat(20));
    assert_eq!(lines[1].as_ref(), format!("  hi{}", " ".repeat(16)));
    assert_eq!(lines[2].as_ref(), " ".repeat(20));

    boxed.clear();
    assert!(boxed.render(20).is_empty());
}

#[test]
fn a_box_reuses_its_cache_until_the_background_changes() {
    let calls = Rc::new(RefCell::new(0usize));
    let child_calls = Rc::clone(&calls);
    let mut boxed = BoxComponent::new(1, 0, None);
    boxed.add_child(component_ref(Text::new("row", 0, 0)));

    let first = boxed.render(10);
    assert_eq!(boxed.render(10), first, "the second render is the cache");

    // A different width misses the cache …
    assert_ne!(boxed.render(12), first);

    // … and so does a background function whose output differs.
    boxed.set_bg_fn(Some(Rc::new(move |text: &str| {
        *child_calls.borrow_mut() += 1;
        format!("<{text}>")
    })));
    let painted = boxed.render(10);
    assert_ne!(painted, first);
    assert!(
        *calls.borrow() > 0,
        "the background function was never asked"
    );
    assert!(painted[0].starts_with('<'));
}

#[test]
fn the_loader_leads_with_a_blank_line_and_paints_frame_and_message() {
    let mut loader = Loader::new(
        Rc::new(|frame: &str| format!("[{frame}]")),
        Rc::new(|message: &str| format!("<{message}>")),
        "Loading...",
        None,
    );

    let lines = loader.render(40);
    assert_eq!(
        lines.len(),
        2,
        "`render` is `[\"\", ...super.render(width)]`"
    );
    assert_eq!(lines[0].as_ref(), "");
    assert!(
        lines[1].contains("[⠋]"),
        "the first frame, coloured: {lines:?}"
    );
    assert!(lines[1].contains("<Loading...>"), "{lines:?}");

    loader.set_message("Still loading");
    assert!(loader.render(40)[1].contains("<Still loading>"));
}

#[test]
fn the_loader_advances_its_frame_on_tick_and_stops_on_stop() {
    let mut loader = Loader::new(plain(), plain(), "work", None);
    assert!(loader.next_frame_deadline().is_some());

    let first = loader.render(20)[1].clone();
    loader.tick();
    let second = loader.render(20)[1].clone();
    assert_ne!(first, second, "the frame advanced");

    // Ten default frames, so ten ticks come back around.
    for _ in 1..10 {
        loader.tick();
    }
    assert_eq!(loader.render(20)[1], first);

    loader.stop();
    assert!(loader.next_frame_deadline().is_none());
}

#[test]
fn an_indicator_is_rendered_verbatim_and_a_single_frame_does_not_animate() {
    let mut loader = Loader::new(
        Rc::new(|frame: &str| format!("[{frame}]")),
        plain(),
        "done",
        Some(LoaderIndicatorOptions {
            frames: Some(vec!["✓".to_string()]),
            interval_ms: None,
        }),
    );

    // `renderIndicatorVerbatim` — the spinner colour is skipped. The line
    // carries the one-column padding of the loader's `Text`.
    assert_eq!(loader.render(20)[1].trim_end(), " ✓ done");
    assert!(
        loader.next_frame_deadline().is_none(),
        "one frame is nothing to animate"
    );

    // An empty frame list hides the indicator entirely.
    loader.set_indicator(Some(LoaderIndicatorOptions {
        frames: Some(Vec::new()),
        interval_ms: None,
    }));
    assert_eq!(loader.render(20)[1].trim_end(), " done");
}

#[test]
fn a_zero_interval_falls_back_to_the_default() {
    let mut loader = Loader::new(
        plain(),
        plain(),
        "work",
        Some(LoaderIndicatorOptions {
            frames: None,
            interval_ms: Some(0),
        }),
    );
    let before = loader.next_frame_deadline().expect("animating");
    loader.tick();
    let after = loader.next_frame_deadline().expect("still animating");
    assert!(
        after.duration_since(before) < Duration::from_millis(200),
        "the deadline moved by the default interval, not by zero"
    );
}

#[test]
fn escape_cancels_the_token_and_calls_back_once_per_press() {
    let aborts = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&aborts);
    let mut loader = CancellableLoader::new(plain(), plain(), "working", None);
    loader.on_abort = Some(Box::new(move || *sink.borrow_mut() += 1));
    let token = loader.token();

    assert!(!loader.aborted());
    assert!(!token.is_cancelled());

    loader.handle_input("x");
    assert!(!loader.aborted(), "other keys do not cancel");

    loader.handle_input(ESCAPE);
    assert!(loader.aborted());
    assert!(token.is_cancelled(), "the handed-out token trips as well");
    assert_eq!(*aborts.borrow(), 1);

    // It renders its loader, blank line included.
    assert_eq!(loader.render(20)[0].as_ref(), "");
}

#[test]
fn a_flash_is_inverse_video_padded_by_a_space_and_truncated_to_the_width() {
    let mut flashes = AltScreenFlashContainer::new();
    assert!(flashes.render(20).is_empty());

    flashes.flash("copied", None);
    let lines = flashes.render(20);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].as_ref(), "\x1b[7m copied \x1b[27m");
    assert!(flashes.take_render_request(), "a flash asks for a frame");
    assert!(!flashes.take_render_request(), "and only once");

    flashes.flash("a message that does not fit into ten columns", None);
    let narrow = flashes.render(10);
    assert_eq!(narrow.len(), 2, "both messages are on screen");
    assert_eq!(visible_width(&narrow[1]), 10);
}

#[test]
fn a_flash_expires_after_its_duration_and_dispose_clears_the_rest() {
    let mut flashes = AltScreenFlashContainer::new();
    flashes.flash("gone in a moment", Some(0));
    flashes.flash("stays", Some(60_000));
    assert!(flashes.next_deadline().is_some());

    std::thread::sleep(Duration::from_millis(5));
    assert!(flashes.expire(), "the elapsed message was dropped");
    assert!(!flashes.expire(), "nothing changed the second time");
    let lines = flashes.render(40);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("stays"));

    flashes.dispose();
    assert!(flashes.render(40).is_empty());
    assert!(flashes.next_deadline().is_none());
}

/// A 1×1 PNG, so the header parser has real dimensions to find.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[test]
fn an_image_without_terminal_support_falls_back_to_one_text_line() {
    // The test process has no image capability (no Kitty/iTerm2 environment),
    let mut image = Image::new(
        PNG_1X1,
        "image/png",
        ImageTheme {
            fallback_color: Rc::new(|text: &str| format!("<{text}>")),
        },
        ImageOptions {
            filename: Some("shot.png".to_string()),
            ..ImageOptions::default()
        },
        None,
    );

    let lines = image.render(40);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with('<'), "the theme paints it: {lines:?}");
    assert!(lines[0].contains("shot.png"), "{lines:?}");
    assert!(visible_width(&lines[0]) <= 40);

    // The line is cached per width and dropped by `invalidate`.
    assert_eq!(image.render(40), lines);
    assert_ne!(image.render(12), lines, "a new width re-renders");
    image.invalidate();
    assert_eq!(image.render(40), lines);
}

#[test]
fn asking_for_a_modifier_reaches_the_platform_and_answers() {
    // What a test can pin down here: the query resolves — on macOS through the
    // prebuilt addon does not load. Whether the answer is `true` depends on the
    // keyboard at that moment, which no test can arrange; that ceiling is in
    // the ledger row.
    for key in [
        ModifierKey::Shift,
        ModifierKey::Control,
        ModifierKey::Option,
        ModifierKey::Command,
    ] {
        let _pressed: bool = is_native_modifier_pressed(key);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    assert!(!is_native_modifier_pressed(ModifierKey::Shift));
}
