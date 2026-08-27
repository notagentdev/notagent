//! The panel a side question is asked and answered in.
//!
//! It sits directly above the editor and frames the exchange, so the question
//! and its answer are visibly not part of the transcript — which is the whole
//! claim the feature makes about them.
//!
//! The box is closed on all four sides here. Where the editor below carries a
//! frame of its own, a panel would leave its bottom edge to that frame; ours
//! draws plain rules with no sides, so the panel closes itself rather than
//! ending in mid-air.

use std::rc::Rc;

use notagent_tui::components::markdown::{Markdown, MarkdownTheme};
use notagent_tui::components::text::Text;
use notagent_tui::tui::{Component, Line, shared_lines};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

/// How many lines of the model's reasoning stand in while an answer is still
/// empty. Enough to show that something is happening, few enough that the
/// reasoning never becomes the content of the panel.
const THINKING_PREVIEW_LINES: usize = 2;

/// The panel never collapses below this, however short the terminal is.
const MIN_COLLAPSED_PANEL_LINES: usize = 3;

/// What a question is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnPhase {
    Running,
    Done,
    Failed,
}

/// One question and what came back.
struct SideQuestionTurn {
    prompt: String,
    answer: String,
    thinking: String,
    error: Option<String>,
    phase: TurnPhase,
}

/// The body after the height rule has been applied to it.
struct BodyRender {
    lines: Vec<String>,
    truncated: bool,
}

/// What the panel has to ask its surroundings.
pub struct SideQuestionPanelOptions {
    pub markdown_theme: MarkdownTheme,
    /// Whether the arrow keys are the panel's to take. They are not while the
    /// user is typing, since the editor needs them to move the caret.
    pub can_use_scroll_keys: Rc<dyn Fn() -> bool>,
    pub terminal_rows: Rc<dyn Fn() -> usize>,
}

pub struct SideQuestionPanel {
    options: SideQuestionPanelOptions,
    turns: Vec<SideQuestionTurn>,
    transient_notices: Vec<String>,
    /// The panel never shrinks back once it has grown, so an answer arriving
    /// line by line does not make the editor below it jump up and down.
    min_body_lines: usize,
    /// Whether new content should scroll into view. True until the user scrolls
    /// away from the end, and true again once they scroll back to it.
    follow_tail: bool,
    scroll_top: usize,
    max_scroll_top: usize,
    /// What the last question was, for the caller that has to send it.
    pending_prompt: Option<String>,
}

impl SideQuestionPanel {
    pub fn new(options: SideQuestionPanelOptions) -> Self {
        Self {
            options,
            turns: Vec::new(),
            transient_notices: Vec::new(),
            min_body_lines: 0,
            follow_tail: true,
            scroll_top: 0,
            max_scroll_top: 0,
            pending_prompt: None,
        }
    }

    /// Records a question. Refused while one is still being answered, and for
    /// an empty question.
    pub fn submit(&mut self, prompt: &str) -> bool {
        let normalized = prompt.trim();
        if normalized.is_empty() || self.is_running() {
            return false;
        }
        self.follow_tail = true;
        self.scroll_top = 0;
        self.transient_notices.clear();
        self.turns.push(SideQuestionTurn {
            prompt: normalized.to_string(),
            answer: String::new(),
            thinking: String::new(),
            error: None,
            phase: TurnPhase::Running,
        });
        self.pending_prompt = Some(normalized.to_string());
        true
    }

    /// The question the caller still has to send, taken once.
    pub fn take_pending_prompt(&mut self) -> Option<String> {
        self.pending_prompt.take()
    }

    /// A note that belongs to the moment rather than to the exchange — it goes
    /// away as soon as anything else happens.
    pub fn add_transient_notice(&mut self, message: impl Into<String>) {
        self.transient_notices.push(message.into());
        self.follow_tail = true;
    }

    /// The answer so far, whole rather than as a delta: the events this panel
    /// is driven by carry the accumulated message, so replacing is what
    /// appending a delta means here.
    pub fn set_answer(&mut self, answer: &str) {
        if let Some(turn) = self.turns.last_mut() {
            turn.answer.clear();
            turn.answer.push_str(answer);
        }
    }

    pub fn set_thinking(&mut self, thinking: &str) {
        if let Some(turn) = self.turns.last_mut() {
            turn.thinking.clear();
            turn.thinking.push_str(thinking);
        }
    }

    /// Closes the current question. `result_summary` stands in when the model
    /// finished without having produced any text.
    pub fn mark_done(&mut self, result_summary: Option<&str>) {
        let Some(turn) = self.turns.last_mut() else {
            return;
        };
        if turn.answer.trim().is_empty()
            && let Some(summary) = result_summary
        {
            turn.answer = summary.to_string();
        }
        self.transient_notices.clear();
        turn.phase = TurnPhase::Done;
    }

    /// Records a failure. A failure with no question in flight — a child that
    /// could not be started at all — becomes an entry of its own, so the reason
    /// is on screen rather than nowhere.
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        let error = error.into();
        match self.turns.last_mut() {
            Some(turn) if turn.phase == TurnPhase::Running => {
                turn.error = Some(error);
                turn.phase = TurnPhase::Failed;
            }
            _ => self.turns.push(SideQuestionTurn {
                prompt: String::new(),
                answer: String::new(),
                thinking: String::new(),
                error: Some(error),
                phase: TurnPhase::Failed,
            }),
        }
        self.transient_notices.clear();
    }

    pub fn is_running(&self) -> bool {
        self.turns
            .last()
            .is_some_and(|turn| turn.phase == TurnPhase::Running)
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Scrolls by one line. Returns whether there was anything to scroll.
    pub fn scroll(&mut self, up: bool) -> bool {
        if self.max_scroll_top == 0 {
            return false;
        }
        let current = if self.follow_tail {
            self.max_scroll_top
        } else {
            self.scroll_top
        };
        let next = if up {
            current.saturating_sub(1)
        } else {
            (current + 1).min(self.max_scroll_top)
        };
        self.scroll_top = next;
        self.follow_tail = next == self.max_scroll_top;
        true
    }

    fn render_top_border(&self, width: usize, truncated: bool) -> String {
        let theme = theme();
        let hint = if truncated && (self.options.can_use_scroll_keys)() {
            "Esc close · ↑↓ scroll "
        } else {
            "Esc close "
        };
        let title = theme.bold(&theme.fg(ThemeColor::Accent, " BTW "))
            + &theme.fg(ThemeColor::Border, "─ ")
            + &theme.fg(ThemeColor::Muted, hint);
        let inner_width = width.saturating_sub(2).max(1);
        let title = if visible_width(&title) > inner_width {
            truncate_to_width_opts(&title, inner_width, "", false)
        } else {
            title
        };
        let dashes = inner_width.saturating_sub(visible_width(&title));
        format!(
            "{}{title}{}{}",
            theme.fg(ThemeColor::Border, "╭"),
            theme.fg(ThemeColor::Border, &"─".repeat(dashes)),
            theme.fg(ThemeColor::Border, "╮")
        )
    }

    fn render_bottom_border(&self, width: usize) -> String {
        let theme = theme();
        let inner_width = width.saturating_sub(2).max(1);
        format!(
            "{}{}{}",
            theme.fg(ThemeColor::Border, "╰"),
            theme.fg(ThemeColor::Border, &"─".repeat(inner_width)),
            theme.fg(ThemeColor::Border, "╯")
        )
    }

    fn render_body_line(&self, line: &str, width: usize) -> String {
        let theme = theme();
        let content_width = width.saturating_sub(4).max(1);
        let clipped = if visible_width(line) > content_width {
            truncate_to_width_opts(line, content_width, "…", false)
        } else {
            line.to_string()
        };
        let padding = content_width.saturating_sub(visible_width(&clipped));
        format!(
            "{} {clipped}{} {}",
            theme.fg(ThemeColor::Border, "│"),
            " ".repeat(padding),
            theme.fg(ThemeColor::Border, "│")
        )
    }

    fn render_body(&mut self, width: usize) -> BodyRender {
        let mut lines: Vec<String> = Vec::new();
        for (index, turn) in self.turns.iter().enumerate() {
            if index > 0 {
                lines.push(String::new());
            }
            lines.extend(render_turn(turn, width, &self.options.markdown_theme));
        }
        if self.turns.is_empty() {
            lines.push(theme().fg(ThemeColor::Dim, "Ready for a side question..."));
        }
        for notice in &self.transient_notices {
            let mut text = Text::new(theme().fg(ThemeColor::Dim, notice), 0, 0);
            lines.extend(text.render(width).iter().map(|line| line.to_string()));
        }
        self.fit_body_lines(lines)
    }

    /// Applies the height rule and the scroll position to the body.
    fn fit_body_lines(&mut self, lines: Vec<String>) -> BodyRender {
        let body_limit = self.collapsed_body_limit();
        let target_uncapped = self.min_body_lines.max(lines.len());
        let target = match body_limit {
            Some(limit) => limit.min(target_uncapped),
            None => target_uncapped,
        };
        self.min_body_lines = self.min_body_lines.max(target);

        if lines.len() > target {
            self.max_scroll_top = lines.len() - target;
            self.scroll_top = if self.follow_tail {
                self.max_scroll_top
            } else {
                self.scroll_top.min(self.max_scroll_top)
            };
            let start = self.scroll_top;
            return BodyRender {
                lines: lines[start..(start + target).min(lines.len())].to_vec(),
                truncated: true,
            };
        }

        self.follow_tail = true;
        self.scroll_top = 0;
        self.max_scroll_top = 0;
        let mut padded = lines;
        while padded.len() < target {
            padded.push(String::new());
        }
        BodyRender {
            lines: padded,
            truncated: false,
        }
    }

    /// At most a third of the terminal, less the line the top border takes.
    fn collapsed_body_limit(&self) -> Option<usize> {
        let rows = (self.options.terminal_rows)();
        if rows == 0 {
            return None;
        }
        let max_panel_lines = MIN_COLLAPSED_PANEL_LINES.max(rows / 3);
        Some(max_panel_lines.saturating_sub(1).max(1))
    }
}

/// One exchange: the question, then whatever stands for the answer so far.
fn render_turn(
    turn: &SideQuestionTurn,
    width: usize,
    markdown_theme: &MarkdownTheme,
) -> Vec<String> {
    let theme = theme();
    let mut lines: Vec<String> = Vec::new();
    if !turn.prompt.is_empty() {
        let mut prompt = Text::new(
            theme.fg(ThemeColor::Accent, &format!("Q: {}", turn.prompt)),
            0,
            0,
        );
        lines.extend(prompt.render(width).iter().map(|line| line.to_string()));
    }

    let answer = turn.answer.trim();
    let thinking = turn.thinking.trim();
    if !answer.is_empty() {
        let mut markdown = Markdown::new(answer, 0, 0, markdown_theme.clone(), None, None);
        lines.extend(markdown.render(width).iter().map(|line| line.to_string()));
    } else if !thinking.is_empty() {
        // The tail rather than the head: what the model is thinking now says
        // more than where it started.
        let mut text = Text::new(theme.fg(ThemeColor::Dim, thinking), 0, 0);
        let rendered: Vec<String> = text
            .render(width)
            .iter()
            .map(|line| line.to_string())
            .collect();
        let start = rendered.len().saturating_sub(THINKING_PREVIEW_LINES);
        lines.extend(rendered[start..].to_vec());
    } else if turn.error.is_none() {
        lines.push(theme.fg(ThemeColor::Dim, "Waiting for answer..."));
    }

    if let Some(error) = &turn.error {
        let mut text = Text::new(theme.fg(ThemeColor::Error, error), 0, 0);
        lines.extend(text.render(width).iter().map(|line| line.to_string()));
    }
    lines
}

impl Component for SideQuestionPanel {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let safe_width = width.max(4);
        let content_width = safe_width.saturating_sub(4).max(1);
        let body = self.render_body(content_width);
        let mut lines = vec![self.render_top_border(safe_width, body.truncated)];
        for line in &body.lines {
            lines.push(self.render_body_line(line, safe_width));
        }
        lines.push(self.render_bottom_border(safe_width));
        shared_lines(lines)
    }

    fn invalidate(&mut self) {}
}
