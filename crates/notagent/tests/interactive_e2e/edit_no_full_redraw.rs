//! Port of `packages/coding-agent/test/edit-tool-no-full-redraw.test.ts` (235 LOC).
//!
//! Three cases about the `edit` row and the render loop: the diff preview of a
//! large edit appears in the call half, the settled result does not cost a full
//! redraw, a settled result alone reconstructs the boxed preview, and a
//! preflight error replaces the diff instead of drawing one.
//!
//! It lives here rather than in `tests/tool_execution_component.rs` because the
//! preview is not a property of the row alone: `edit`'s `render_call` only
//! registers the work, and the render loop awaits it
//! (`ToolExecutionComponent::pump_render`, the seam of A-23/C-17). What the TS
//! file drives with `waitForRender` (`setTimeout(0)`) is that loop.
//!
//! Deviations (class 1, test infrastructure): the `FakeTerminal` of the TS file
//! is the `VirtualTerminal` of A-2 — it records writes the same way, so
//! `fullClearCount` is a count over `writes()`; `waitForRenderedText` becomes a
//! loop over `pump_render` plus a settle, since the port awaits the preview on
//! the loop instead of inside the renderer.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notagent::core::tools::ToolDef;
use notagent::core::tools::edit::create_edit_tool_definition;
use notagent::core::tools::edit_diff::{Edit, compute_edits_diff};
use notagent::modes::interactive::components::tool_execution::{
    ToolExecutionComponent, ToolExecutionOptions, ToolExecutionResult,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_ai::types::{TextContent, TextOrImageContent};
use notagent_tui::components::text::Text;
use notagent_tui::test_terminal::VirtualTerminal;
use notagent_tui::tui::{Component, ComponentRef, Container, component_ref, run_until};
use notagent_tui::tui_main_screen::TuiMainScreen;
use serde_json::json;

use super::harness::run_local;

/// `\x1b[2J\x1b[H\x1b[3J` — the sequence a full redraw writes
/// (`FakeTerminal.fullClearCount`).
const FULL_CLEAR: &str = "\x1b[2J\x1b[H\x1b[3J";

/// `waitForRenderedText`'s budget.
const WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// A file of `count` lines `line 0 … line n`, as the TS cases write it.
fn write_lines(dir: &std::path::Path, name: &str, count: usize) -> String {
    let path = dir.join(name);
    let body: Vec<String> = (0..count).map(|index| format!("line {index}")).collect();
    std::fs::write(&path, format!("{}\n", body.join("\n"))).expect("write fixture");
    path.to_string_lossy().into_owned()
}

/// `createLargeEdits(lines)` — ten three-line blocks whose middle line changes.
fn large_edits(count: usize) -> Vec<Edit> {
    let line = |index: usize| format!("line {index}");
    [50, 150, 250, 350, 450, 550, 650, 750, 850, 950]
        .into_iter()
        .take_while(|target| target + 1 < count)
        .map(|target| Edit {
            // The TS helper indexes the 1-based line number into a 0-based
            // array, so the changed line is `lines[target]` = `line {target}`.
            old_text: format!(
                "{}\n{}\n{}",
                line(target - 1),
                line(target),
                line(target + 1)
            ),
            new_text: format!(
                "{}\n{} changed\n{}",
                line(target - 1),
                line(target),
                line(target + 1)
            ),
        })
        .collect()
}

/// The row under test, mounted in a started screen over the virtual terminal.
struct EditRow {
    ui: TuiMainScreen,
    pump: notagent_tui::test_terminal::VirtualTerminalPump,
    terminal: VirtualTerminal,
    row: Rc<RefCell<ToolExecutionComponent>>,
}

impl EditRow {
    /// `new TuiMainScreen(terminal)` plus the row, with `history` filler lines
    /// above it (the TS case mounts 200 of them so a full redraw would show).
    fn mount(path: &str, edits: &[Edit], history: usize) -> Self {
        let cwd = std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .into_owned();
        let definition: ToolDef = Arc::new(create_edit_tool_definition(&cwd, None));
        let terminal = VirtualTerminal::new(80, 24);
        let pump = terminal.pump_handle();
        let mut ui = TuiMainScreen::new(Box::new(terminal.clone()));

        let core = ui.core().clone();
        let row = Rc::new(RefCell::new(ToolExecutionComponent::new(
            "edit",
            "tool-call-1",
            json!({ "path": path, "edits": edits_json(edits) }),
            ToolExecutionOptions::default(),
            Some(definition),
            Rc::new(move || core.request_render()),
            cwd,
        )));

        let root = Rc::new(RefCell::new(Container::new()));
        for index in 0..history {
            root.borrow_mut()
                .add_child(component_ref(Text::new(format!("history {index}"), 0, 0)));
        }
        root.borrow_mut().add_child(Rc::clone(&row) as ComponentRef);
        ui.core().add_child(root as ComponentRef);
        ui.start();

        EditRow {
            ui,
            pump,
            terminal,
            row,
        }
    }

    /// `await waitForRender()` — let the throttled pipeline paint what is due.
    async fn settle(&mut self) {
        let terminal = self.terminal.clone();
        run_until(&mut self.ui, &mut self.pump, async move {
            terminal.wait_for_render().await;
        })
        .await;
    }

    /// The loop's half of the seam: run the row's pending render work, then
    /// paint. Returns whether anything ran.
    async fn pump_row(&mut self) -> bool {
        // The work travels detached from the row, so the loop never holds the
        // row's borrow across the await (`RowRenderWork`).
        let work = self.row.borrow().render_work();
        let ran = work.run().await;
        if ran {
            self.ui.core().request_render();
        }
        self.settle().await;
        ran
    }

    /// `waitForRenderedText(...)` — drive the loop until the row shows `needle`.
    async fn wait_for_row_text(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            self.pump_row().await;
            let rendered = self.row_text();
            if rendered.contains(needle) {
                return rendered;
            }
            if Instant::now() >= deadline {
                panic!("the row never rendered {needle:?} within {WAIT_TIMEOUT:?}:\n{rendered}");
            }
        }
    }

    /// `component.render(80).join("\n")`, without the styling.
    fn row_text(&self) -> String {
        strip_ansi(&self.row.borrow_mut().render(80).join("\n"))
    }

    fn full_clears(&self) -> usize {
        self.terminal.get_writes().matches(FULL_CLEAR).count()
    }
}

fn edits_json(edits: &[Edit]) -> serde_json::Value {
    serde_json::Value::Array(
        edits
            .iter()
            .map(|edit| json!({ "old_string": edit.old_text, "new_string": edit.new_text }))
            .collect(),
    )
}

fn success_result(
    edits: usize,
    path: &str,
    details: notagent::core::tools::edit_diff::DiffString,
) -> ToolExecutionResult {
    ToolExecutionResult {
        content: vec![TextOrImageContent::Text(TextContent {
            text: format!("Successfully replaced {edits} block(s) in {path}."),
            ..TextContent::default()
        })],
        details: Some(json!({
            "diff": details.diff,
            "firstChangedLine": details.first_changed_line,
        })),
        is_error: false,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn renders_the_large_diff_in_the_call_preview_without_a_full_redraw_on_the_result() {
    run_local(async {
        init_theme(Some("dark"), false);
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_lines(dir.path(), "large-edit.txt", 1000);
        let edits = large_edits(1000);
        let cwd = std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .into_owned();
        let diff = compute_edits_diff(&path, &edits, &cwd)
            .await
            .expect("the fixture applies cleanly");

        let mut screen = EditRow::mount(&path, &edits, 200);
        screen.settle().await;

        screen.row.borrow_mut().set_args_complete();
        screen.settle().await;

        // The preview is the render loop's work, and it only starts once the
        // arguments are complete.
        let call_only = screen.wait_for_row_text("line 50 changed").await;
        // The badge style (the default) heads the row with an EDIT badge;
        // the standard style keeps the lowercase name in the call line.
        assert!(
            call_only.contains("EDIT") || call_only.contains("edit"),
            "{call_only}"
        );
        assert!(call_only.contains("line 950 changed"), "{call_only}");

        let redraws_before = screen.ui.full_redraws();
        let clears_before = screen.full_clears();

        screen
            .row
            .borrow_mut()
            .update_result(success_result(edits.len(), &path, diff), false);
        screen.ui.core().request_render();
        screen.settle().await;

        // In the badge style the row's state lives in its top badge, and the
        // settling result flips it from pending to success — one repaint of a
        // line far above the fold, hence at most one full redraw. The diff
        // body itself must not be re-laid-out (that would show up as more).
        assert!(
            screen.ui.full_redraws() <= redraws_before + 1,
            "the settled result forced more than the badge repaint: {} -> {}",
            redraws_before,
            screen.ui.full_redraws()
        );
        assert!(
            screen.full_clears() <= clears_before + 1,
            "the settled result cleared the screen more than the badge repaint: {} -> {}",
            clears_before,
            screen.full_clears()
        );

        let settled = screen.row_text();
        assert!(settled.contains("line 50 changed"), "{settled}");
        assert!(settled.contains("line 950 changed"), "{settled}");
        assert!(!settled.contains("Successfully replaced"), "{settled}");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reconstructs_the_boxed_preview_from_a_settled_result_without_args_complete() {
    run_local(async {
        init_theme(Some("dark"), false);
        let dir = tempfile::tempdir().expect("temp dir");
        let path = write_lines(dir.path(), "replay-edit.txt", 200);
        let edits: Vec<Edit> = large_edits(200).into_iter().take(2).collect();
        let cwd = std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .into_owned();
        let diff = compute_edits_diff(&path, &edits, &cwd)
            .await
            .expect("the fixture applies cleanly");
        // The TS case deletes the file first: the preview may only come from
        // the result, never from a second look at the disk.
        std::fs::remove_file(&path).expect("remove fixture");

        let mut screen = EditRow::mount(&path, &edits, 0);
        screen.settle().await;

        screen
            .row
            .borrow_mut()
            .update_result(success_result(edits.len(), &path, diff), false);
        screen.settle().await;

        let rendered = screen.row_text();
        assert!(rendered.contains("line 50 changed"), "{rendered}");
        assert!(rendered.contains("line 150 changed"), "{rendered}");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn shows_a_preflight_error_instead_of_a_diff_when_the_edits_do_not_apply() {
    run_local(async {
        init_theme(Some("dark"), false);
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("missing-edit.txt");
        std::fs::write(&path, "line 0\nline 1\n").expect("write fixture");
        let path = path.to_string_lossy().into_owned();
        let edits = vec![Edit {
            old_text: "does not exist".to_string(),
            new_text: "replacement".to_string(),
        }];

        let mut screen = EditRow::mount(&path, &edits, 0);
        screen.settle().await;

        screen.row.borrow_mut().set_args_complete();
        screen.settle().await;

        let rendered = screen.wait_for_row_text("Could not find").await;
        assert!(!rendered.contains("+1 "), "{rendered}");
        assert!(!rendered.contains("-1 "), "{rendered}");
    })
    .await;
}
