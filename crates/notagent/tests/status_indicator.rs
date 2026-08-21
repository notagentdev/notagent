//! Port of `packages/coding-agent/test/status-indicator.test.ts` (32 LOC), plus
//! the retry countdown behaviour the TypeScript test only observes indirectly
//! through `requestRender`.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::status_indicator::{
    CompactionStatusReason, IdleStatus, StatusIndicator, StatusIndicatorKind,
};
use notagent::modes::interactive::theme::theme::init_theme;
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
