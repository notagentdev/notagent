//! Status markers are resolved after clipping, so hidden rows cannot drive animation.
use crate::tui::Line;
use std::time::{Duration, Instant};

const START: &str = "\x1b_notagent:a\x07";
const END: &str = "\x1b_notagent:/a\x07";
pub const RUNNING_DOT: &str = "\x1b_notagent:a\x07●\x1b_notagent:/a\x07";
const INTERVAL: Duration = Duration::from_millis(600);

#[derive(Clone)]
pub(crate) struct ActivityClock {
    epoch: Instant,
    pub focused: bool,
    pub allowed: bool,
    pub deadline: Option<Instant>,
}
impl Default for ActivityClock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            focused: true,
            allowed: false,
            deadline: None,
        }
    }
}
impl ActivityClock {
    pub fn paint(
        &mut self,
        lines: &mut [Line],
        first_visible: usize,
        previous: &[Line],
        now: Instant,
        blocked: bool,
    ) {
        let enabled = self.allowed && self.focused && !blocked;
        let elapsed = now.saturating_duration_since(self.epoch);
        let hidden = enabled && (elapsed.as_millis() / 600) % 2 == 1;
        let mut active = false;
        for (index, line) in lines.iter_mut().enumerate() {
            if !line.contains(START) && !line.contains(END) {
                continue;
            }
            let running = line.contains(RUNNING_DOT);
            let visible = index >= first_visible;
            active |= visible && running;
            let shown = static_markers(line);
            let blank = line
                .replace(RUNNING_DOT, " ")
                .replace(START, "")
                .replace(END, "");
            // Main-screen scrollback cannot be repainted by a blink tick.
            if !visible
                && let Some(old) = previous.get(index)
                && (old.as_ref() == shown
                    || old.as_ref() == blank
                    || old.as_ref() == crate::tui::finish_line(&shown)
                    || old.as_ref() == crate::tui::finish_line(&blank))
            {
                *line = old.clone();
            } else {
                *line = Line::from(if hidden && visible { blank } else { shown });
            }
        }
        self.deadline = (enabled && active)
            .then(|| now + INTERVAL - Duration::from_millis((elapsed.as_millis() % 600) as u64));
    }
}

/// Remove internal animation metadata while retaining a readable static dot.
pub fn static_markers(text: &str) -> String {
    text.replace(START, "").replace(END, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_markers_share_phase_and_leave_unchanged_lines_shared() {
        let mut clock = ActivityClock {
            allowed: true,
            ..ActivityClock::default()
        };
        let body = Line::from("unchanged output");
        for (ms, expected) in [(0, "●"), (599, "●"), (600, " "), (1199, " "), (1200, "●")] {
            let mut lines = vec![
                Line::from(RUNNING_DOT),
                body.clone(),
                Line::from(RUNNING_DOT),
            ];
            clock.paint(
                &mut lines,
                0,
                &[],
                clock.epoch + Duration::from_millis(ms),
                false,
            );
            assert_eq!(lines[0].as_ref(), expected);
            assert_eq!(lines[0], lines[2]);
            assert!(
                std::sync::Arc::ptr_eq(&body, &lines[1]),
                "body lines must stay cached"
            );
        }
    }
    #[test]
    fn completing_during_the_hidden_phase_restores_a_static_colored_dot() {
        for color in ["\x1b[32m", "\x1b[31m"] {
            let mut clock = ActivityClock {
                allowed: true,
                ..ActivityClock::default()
            };
            let hidden_at = clock.epoch + INTERVAL;
            let mut running = vec![Line::from(format!("{RUNNING_DOT} tool"))];
            clock.paint(&mut running, 0, &[], hidden_at, false);
            assert_eq!(running[0].as_ref(), "  tool");
            let finished = Line::from(format!("{color}●\x1b[39m tool"));
            for ms in [600, 601, 1199, 1200, 1800] {
                let mut lines = vec![finished.clone()];
                clock.paint(
                    &mut lines,
                    0,
                    &running,
                    clock.epoch + Duration::from_millis(ms),
                    false,
                );
                assert_eq!(
                    lines[0], finished,
                    "a final dot must be visible independently of blink phase"
                );
                assert!(
                    clock.deadline.is_none(),
                    "completion must stop the animation timer"
                );
            }
        }
    }

    #[test]
    fn offscreen_rows_preserve_their_last_painted_phase() {
        let mut clock = ActivityClock {
            allowed: true,
            ..ActivityClock::default()
        };
        let previous = vec![Line::from(crate::tui::finish_line(" "))];
        let mut lines = vec![Line::from(RUNNING_DOT)];
        clock.paint(&mut lines, 1, &previous, clock.epoch, false);
        assert!(
            std::sync::Arc::ptr_eq(&previous[0], &lines[0]),
            "scrollback must not be rewritten to change an invisible phase"
        );
        assert!(clock.deadline.is_none());
    }

    #[test]
    fn hidden_unfocused_and_blocked_markers_do_not_schedule_ticks() {
        for (focused, allowed, first, blocked) in [
            (false, true, 0, false),
            (true, false, 0, false),
            (true, true, 1, false),
            (true, true, 0, true),
        ] {
            let mut clock = ActivityClock {
                focused,
                allowed,
                ..ActivityClock::default()
            };
            let mut lines = vec![Line::from(RUNNING_DOT)];
            clock.paint(&mut lines, first, &[], clock.epoch + INTERVAL, blocked);
            assert!(clock.deadline.is_none());
            assert_eq!(lines[0].as_ref(), "●");
        }
    }
}
