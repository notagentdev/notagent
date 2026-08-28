use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::bash_execution::BashExecutionComponent;
use notagent::modes::interactive::theme::theme::init_theme;
use notagent_tui::tui::Component;
use notagent_tui::utils::visible_width;

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[test]
fn collapsed_preview_lines_respect_render_time_width_not_construction_time_width() {
    let _guard = theme_lock();
    init_theme(None, false);
    let narrow_width = 80;

    let mut component = BashExecutionComponent::new("pwd", false);

    // Add output with long lines that will wrap differently at different widths
    let long_line = "x".repeat(150);
    component.append_output(&format!("{long_line}\n{long_line}\n"));

    // Complete the command so it enters collapsed mode
    component.set_complete(Some(0), false, None, None);

    // Render at the narrow width (simulating a resize or split pane)
    let lines = component.render(narrow_width);

    // Every rendered line must fit within the narrow width
    for (index, line) in lines.iter().enumerate() {
        let width = visible_width(line);
        assert!(
            width <= narrow_width,
            "Line {index} visibleWidth={width} > {narrow_width}"
        );
    }
}

#[test]
fn re_computes_lines_when_width_changes_between_renders() {
    let _guard = theme_lock();
    init_theme(None, false);

    let mut component = BashExecutionComponent::new("echo hello", false);

    let long_line = "abcdefghij".repeat(20); // 200 chars
    component.append_output(&format!("{long_line}\n"));
    component.set_complete(Some(0), false, None, None);

    // First render at width 200
    for line in component.render(200) {
        assert!(visible_width(&line) <= 200);
    }

    // Second render at width 60 (split pane scenario)
    let lines = component.render(60);
    for (index, line) in lines.iter().enumerate() {
        let width = visible_width(line);
        assert!(width <= 60, "Line {index} visibleWidth={width} > 60");
    }
}
