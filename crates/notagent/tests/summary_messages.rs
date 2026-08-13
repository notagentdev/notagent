//! Behaviour of the compaction and branch summary components, which have no
//! TypeScript tests. Expectations are taken from
//! `packages/coding-agent/src/modes/interactive/components/{compaction,branch}-summary-message.ts`.

use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::branch_summary_message::BranchSummaryMessageComponent;
use notagent::modes::interactive::components::compaction_summary_message::CompactionSummaryMessageComponent;
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_agent::{BranchSummaryMessage, CompactionSummaryMessage};
use notagent_tui::tui::Component;

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[test]
fn collapses_and_expands_the_compaction_summary() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = CompactionSummaryMessageComponent::new(
        CompactionSummaryMessage {
            summary: "The summary body.".to_string(),
            tokens_before: 123_456,
            timestamp: 0,
        },
        None,
    );

    let collapsed = strip_ansi(&component.render(60).join("\n"));
    assert!(collapsed.contains("[compaction]"), "{collapsed}");
    // The token count is grouped like `Number.prototype.toLocaleString`.
    assert!(
        collapsed.contains("Compacted from 123,456 tokens ("),
        "{collapsed}"
    );
    assert!(collapsed.contains(" to expand)"), "{collapsed}");
    assert!(!collapsed.contains("The summary body."), "{collapsed}");

    component.set_expanded(true);
    let expanded = strip_ansi(&component.render(60).join("\n"));
    assert!(expanded.contains("[compaction]"), "{expanded}");
    assert!(
        expanded.contains("Compacted from 123,456 tokens"),
        "{expanded}"
    );
    assert!(expanded.contains("The summary body."), "{expanded}");
}

#[test]
fn collapses_and_expands_the_branch_summary() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = BranchSummaryMessageComponent::new(
        BranchSummaryMessage {
            summary: "What the branch did.".to_string(),
            from_id: "entry-1".to_string(),
            timestamp: 0,
        },
        None,
    );

    let collapsed = strip_ansi(&component.render(60).join("\n"));
    assert!(collapsed.contains("[branch]"), "{collapsed}");
    assert!(collapsed.contains("Branch summary ("), "{collapsed}");
    assert!(!collapsed.contains("What the branch did."), "{collapsed}");

    component.set_expanded(true);
    let expanded = strip_ansi(&component.render(60).join("\n"));
    assert!(expanded.contains("Branch Summary"), "{expanded}");
    assert!(expanded.contains("What the branch did."), "{expanded}");
}

#[test]
fn keeps_the_bold_label_sequences() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut component = BranchSummaryMessageComponent::new(
        BranchSummaryMessage {
            summary: String::new(),
            from_id: String::new(),
            timestamp: 0,
        },
        None,
    );
    let rendered = component.render(60).join("\n");
    assert!(rendered.contains("\x1b[1m[branch]\x1b[22m"), "{rendered:?}");
}
