use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::armin::ArminComponent;
use notagent::modes::interactive::components::daxnuts::DaxnutsComponent;
use notagent::modes::interactive::components::earendil_announcement::EarendilAnnouncementComponent;
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::tui::Component;
use notagent_tui::utils::visible_width;

/// The global theme is a process global.
fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    guard
}

// --- armin -----------------------------------------------------------------------

#[test]
fn armin_renders_the_grid_plus_the_message_row() {
    let _guard = theme_lock();
    let mut armin = ArminComponent::new();

    let lines = armin.render(40);
    // 36 pixel rows are packed into 18 half-block rows, plus the message.
    assert_eq!(lines.len(), 19);
    assert!(
        strip_ansi(lines.last().unwrap()).contains("ARMIN SAYS HI"),
        "{:?}",
        lines.last()
    );
    // Every row is padded to the full width behind the one-column indent.
    for line in &lines {
        assert_eq!(visible_width(line), 40, "{:?}", strip_ansi(line));
    }
}

#[test]
fn armin_clips_the_grid_to_a_narrow_terminal() {
    let _guard = theme_lock();
    let mut armin = ArminComponent::new();

    let lines = armin.render(12);
    let (message, grid) = lines.split_last().expect("the message row is last");
    for line in grid {
        assert!(visible_width(line) <= 12, "{:?}", strip_ansi(line));
    }
    // bug-compat: only the grid rows are clipped. The message keeps its full
    // length and overflows a terminal narrower than 15 columns, because
    assert_eq!(strip_ansi(message), " ARMIN SAYS HI");
}

#[test]
fn armin_animates_until_disposed() {
    let _guard = theme_lock();
    let mut armin = ArminComponent::new();
    assert!(armin.deadline().is_some());

    // The frame is not due yet, so a tick changes nothing.
    assert!(!armin.tick());

    armin.dispose();
    assert!(armin.deadline().is_none());
    assert!(!armin.tick());
}

#[test]
fn armin_advances_a_frame_once_it_is_due() {
    let _guard = theme_lock();
    let mut armin = ArminComponent::new();
    let first = armin.deadline().expect("the animation runs");

    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(armin.tick(), "the due frame advances");
    // Either the next frame was scheduled, or the effect finished within it.
    if let Some(next) = armin.deadline() {
        assert!(next > first);
    }
}

// --- daxnuts ---------------------------------------------------------------------

#[test]
fn daxnuts_starts_with_the_scan_line_and_no_text() {
    let _guard = theme_lock();
    let mut daxnuts = DaxnutsComponent::new();

    let lines = daxnuts.render(60);
    // Blank, 16 half-block rows, blank, three text rows, blank, two link rows, blank.
    assert_eq!(lines.len(), 25);
    let plain = strip_ansi(&lines.join("\n"));
    // The reveal has not started, so the first row is the scan line.
    assert!(plain.contains(&"▓".repeat(32)), "{plain}");
    assert!(!plain.contains("Powered by daxnuts"), "{plain}");
}

#[test]
fn daxnuts_centres_its_rows() {
    let _guard = theme_lock();
    let mut daxnuts = DaxnutsComponent::new();

    let lines = daxnuts.render(60);
    let scanline = lines
        .iter()
        .find(|line| strip_ansi(line).contains('▓'))
        .expect("the scan line is drawn");
    // (60 - 32) / 2 = 14 columns of padding.
    assert!(
        strip_ansi(scanline).starts_with(&" ".repeat(14)),
        "{:?}",
        strip_ansi(scanline)
    );
}

#[test]
fn daxnuts_animates_until_disposed() {
    let _guard = theme_lock();
    let mut daxnuts = DaxnutsComponent::new();
    assert!(daxnuts.deadline().is_some());
    assert!(!daxnuts.tick(), "the first frame is not due yet");

    std::thread::sleep(std::time::Duration::from_millis(90));
    assert!(daxnuts.tick(), "the due frame advances");

    daxnuts.dispose();
    assert!(daxnuts.deadline().is_none());
    assert!(!daxnuts.tick());
}

// --- earendil announcement --------------------------------------------------------

#[test]
fn earendil_announces_the_blog_post() {
    let _guard = theme_lock();
    let mut announcement = EarendilAnnouncementComponent::new();

    let plain = strip_ansi(&announcement.render(80).join("\n"));
    assert!(plain.contains("notagent has joined Earendil"), "{plain}");
    assert!(plain.contains("Read the blog post:"), "{plain}");
    assert!(
        plain.contains("https://mariozechner.at/posts/2026-04-08-ive-sold-out/"),
        "{plain}"
    );
    // Top and bottom border.
    assert!(plain.starts_with(&"─".repeat(80)), "{plain}");
    assert!(plain.ends_with(&"─".repeat(80)), "{plain}");
    // The picture is part of the panel: it is compiled into the binary, so it
    // is there whether or not an assets directory exists next to it.
    assert!(
        plain.contains("[Image: clankolas.png [image/png] 640x537]"),
        "the picture is compiled into the binary, so it is there without an assets directory: {plain}"
    );
}
