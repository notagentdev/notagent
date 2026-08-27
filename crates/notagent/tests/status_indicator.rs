//! Port of `packages/coding-agent/test/status-indicator.test.ts` (32 LOC), plus
//! the retry countdown behaviour the TypeScript test only observes indirectly
//! through `requestRender`.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::status_indicator::{
    CompactionStatusReason, IdleStatus, StatusIndicator, StatusIndicatorKind,
};
use notagent::modes::interactive::theme::theme::{ThemeColor, init_theme, theme};
use notagent_tui::tui::{Component, Line};

/// The status message carries its animation as a colour sequence per run of
/// characters, so the words are only contiguous once the escapes are gone.
fn visible(text: &str) -> String {
    let mut out = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\x1b' {
            out.push(character);
            continue;
        }
        for escaped in characters.by_ref() {
            if escaped == 'm' {
                break;
            }
        }
    }
    out
}

/// The global theme is a process global.
fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[test]
fn keeps_idle_status_at_the_same_height_as_status_indicators() {
    let mut idle_status = IdleStatus;

    let lines = idle_status.render(20);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines, vec![Line::from(" ".repeat(20)); 2]);
}

#[test]
fn disposes_retry_countdown_updates() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let mut indicator = StatusIndicator::retry(1, 3, 1000);
    assert_eq!(indicator.kind, StatusIndicatorKind::Retry);
    assert!(indicator.countdown_deadline().is_some());

    indicator.dispose();

    // TypeScript advances the fake timers by 2 s and asserts that no further
    // `requestRender` arrives; the polled port has no deadline left to fire.
    assert!(indicator.countdown_deadline().is_none());
    assert!(!indicator.tick_countdown());
}

#[test]
fn counts_the_retry_delay_down_second_by_second() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    // `app.interrupt` is an app keybinding; until workstream C registers it the
    // hint renders empty, exactly as an unbound keybinding does in TypeScript.
    let mut indicator = StatusIndicator::retry(2, 5, 2400);
    let first = visible(&indicator.render(80).join("\n"));
    assert!(first.contains("Retrying (2/5) in 3s..."), "{first}");

    std::thread::sleep(std::time::Duration::from_millis(1050));
    assert!(indicator.tick_countdown());
    let second = visible(&indicator.render(80).join("\n"));
    assert!(second.contains("Retrying (2/5) in 2s..."), "{second}");
    assert!(indicator.countdown_deadline().is_some());
}

#[test]
fn labels_every_indicator_kind() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut working = StatusIndicator::working("Working...", None);
    assert_eq!(working.kind, StatusIndicatorKind::Working);
    assert!(visible(&working.render(80).join("\n")).contains("Working..."));

    let mut manual = StatusIndicator::compaction(CompactionStatusReason::Manual);
    assert_eq!(manual.kind, StatusIndicatorKind::Compaction);
    assert!(visible(&manual.render(80).join("\n")).contains("Compacting context... ( to cancel)"));

    let mut threshold = StatusIndicator::compaction(CompactionStatusReason::Threshold);
    assert!(visible(&threshold.render(80).join("\n")).contains("Auto-compacting... ( to cancel)"));

    let mut overflow = StatusIndicator::compaction(CompactionStatusReason::Overflow);
    assert!(
        visible(&overflow.render(80).join("\n"))
            .contains("Context overflow detected, Auto-compacting... ( to cancel)")
    );

    let mut branch = StatusIndicator::branch_summary();
    assert_eq!(branch.kind, StatusIndicatorKind::BranchSummary);
    assert!(visible(&branch.render(80).join("\n")).contains("Summarizing branch... ( to cancel)"));
}

/// The running time appears once there is a whole second to show, and the
/// figure only changes when the second does.
#[test]
fn counts_the_time_the_work_is_taking() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut working = StatusIndicator::working("Working...", None);
    assert!(!visible(&working.render(80).join("\n")).contains('('));

    std::thread::sleep(std::time::Duration::from_millis(1050));
    assert!(working.tick_elapsed(), "the first second was not picked up");
    assert!(
        visible(&working.render(80).join("\n")).contains("Working... (1s)"),
        "{}",
        visible(&working.render(80).join("\n"))
    );
    // The clock sits outside the animated message: its figure carries the
    // plain muted colour, never the travelling fade.
    let raw = working.render(80).join("\n");
    assert!(
        raw.contains(&theme().fg(ThemeColor::Muted, "1s")),
        "the elapsed figure is styled as a still suffix: {raw:?}"
    );
    assert!(!working.tick_elapsed(), "the same second was redrawn again");
}

/// The line stays behind and says what the work took, instead of being blanked
/// at the moment that figure becomes final.
#[test]
fn settles_into_what_the_work_took() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut working = StatusIndicator::working("Working...", None);
    assert!(!working.is_settled());

    working.settle();

    assert!(working.is_settled());
    let settled = visible(&working.render(80).join("\n"));
    assert!(settled.contains("Worked for "), "{settled}");
    assert!(!settled.contains("Working..."), "{settled}");
    // Nothing is animating any more.
    assert!(working.loader_mut().next_frame_deadline().is_none());
    assert!(!working.tick_elapsed());
}

/// Settling twice must not restart the clock or overwrite the figure.
#[test]
fn settles_only_once() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut working = StatusIndicator::working("Working...", None);
    working.settle();
    let first = visible(&working.render(80).join("\n"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    working.settle();
    assert_eq!(visible(&working.render(80).join("\n")), first);
}

/// A retry counts down; a second clock counting up beside it would be two
/// figures disagreeing about what they measure.
#[test]
fn a_retry_carries_no_second_clock() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut retry = StatusIndicator::retry(1, 3, 2000);
    std::thread::sleep(std::time::Duration::from_millis(1050));
    assert!(!retry.tick_elapsed());
    let rendered = visible(&retry.render(80).join("\n"));
    assert!(rendered.contains("Retrying (1/3) in"), "{rendered}");
}

/// Every place that shows a running time writes it the same way.
///
/// There were three copies of this arithmetic, and one of them rounded where
/// the others floored, so a badge and the task roster could disagree by a
/// second about the same task.
#[test]
fn every_running_time_is_written_the_same_way() {
    use notagent::modes::interactive::components::subagent_panel::format_elapsed as panel_elapsed;
    use notagent::modes::interactive::theme::theme::{
        format_elapsed, format_elapsed_live, format_elapsed_precise,
    };
    use std::time::Duration;

    for (millis, expected) in [
        (0, "0s"),
        (999, "0s"),
        (12_000, "12s"),
        (59_999, "59s"),
        (60_000, "1m 0s"),
        (95_000, "1m 35s"),
        (3_600_000, "1h 0m"),
        (3_900_000, "1h 5m"),
    ] {
        let duration = Duration::from_millis(millis);
        assert_eq!(format_elapsed(duration), expected, "{millis}ms");
        // The panel counts between two stamps and must land on the same string.
        assert_eq!(panel_elapsed(0, millis as i64), expected, "{millis}ms");
        if millis >= 1000 {
            assert_eq!(format_elapsed_live(duration).as_deref(), Some(expected));
            assert_eq!(format_elapsed_precise(duration), expected);
        }
    }

    // Below a second the two differ on purpose: a badge that has finished says
    // how long it took, a badge still running says nothing rather than
    // flickering through milliseconds.
    assert_eq!(format_elapsed_live(Duration::from_millis(450)), None);
    assert_eq!(format_elapsed_precise(Duration::from_millis(450)), "450ms");
}
