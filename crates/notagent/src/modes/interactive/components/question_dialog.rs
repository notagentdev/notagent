use notagent_tui::components::input::Input;
use notagent_tui::components::text::Text;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::tui::{Component, Focusable, Line};

use crate::core::user_questions::{QuestionAnswer, UserQuestion};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::key_hint;

const OWN_ANSWER_LABEL: &str = "Type my own answer";

/// Called once with the answers, or `None` when the dialog was dismissed.
pub type QuestionDialogDone = Box<dyn FnOnce(Option<Vec<QuestionAnswer>>)>;

/// Walks the user through the model's questions one at a time.
/// Drawn by hand rather than on `SelectList`, because a multi-select question
/// needs a checkbox per row and a way to confirm several of them at once.
/// Escape on the list dismisses the whole dialog and reports no answers at
/// all: a partial set would let the model treat the unanswered ones as
/// settled.
pub struct QuestionDialogComponent {
    questions: Vec<UserQuestion>,
    index: usize,
    answers: Vec<QuestionAnswer>,
    /// Highlighted row; the last row is the free-text entry.
    cursor: usize,
    /// Checked rows of the current question, for multi-select.
    checked: Vec<bool>,
    /// Present while the user types their own answer.
    typing: Option<Input>,
    on_done: Option<QuestionDialogDone>,
    focused: bool,
}

impl QuestionDialogComponent {
    pub fn new(questions: Vec<UserQuestion>, on_done: QuestionDialogDone) -> Self {
        let checked = questions
            .first()
            .map_or_else(Vec::new, |question| vec![false; question.options.len()]);
        Self {
            questions,
            index: 0,
            answers: Vec::new(),
            cursor: 0,
            checked,
            typing: None,
            on_done: Some(on_done),
            focused: false,
        }
    }

    fn current(&self) -> Option<&UserQuestion> {
        self.questions.get(self.index)
    }

    fn row_count(&self) -> usize {
        self.current().map_or(0, |question| question.options.len()) + 1
    }

    fn finish(&mut self, answers: Option<Vec<QuestionAnswer>>) {
        if let Some(done) = self.on_done.take() {
            done(answers);
        }
    }

    fn record(&mut self, selected: Vec<String>, custom: Option<String>) {
        let Some(question) = self.current() else {
            return;
        };
        self.answers.push(QuestionAnswer {
            id: question.id.clone(),
            selected,
            custom,
        });
        self.index += 1;
        self.cursor = 0;
        self.typing = None;
        self.checked = self
            .current()
            .map_or_else(Vec::new, |question| vec![false; question.options.len()]);
        if self.index >= self.questions.len() {
            let answers = std::mem::take(&mut self.answers);
            self.finish(Some(answers));
        }
    }

    fn confirm_row(&mut self) {
        let Some(question) = self.current() else {
            return;
        };
        let option_count = question.options.len();
        if self.cursor >= option_count {
            let mut input = Input::new();
            input.set_focused(self.focused);
            self.typing = Some(input);
            return;
        }
        let selected: Vec<String> = if question.multi_select {
            let ticked: Vec<String> = question
                .options
                .iter()
                .zip(&self.checked)
                .filter(|(_, checked)| **checked)
                .map(|(option, _)| option.label.clone())
                .collect();
            // Enter on an unticked list takes the highlighted row, so a
            // multi-select question can still be answered with one keypress.
            if ticked.is_empty() {
                vec![question.options[self.cursor].label.clone()]
            } else {
                ticked
            }
        } else {
            vec![question.options[self.cursor].label.clone()]
        };
        self.record(selected, None);
    }

    fn handle_typing(&mut self, data: &str) {
        if keybindings_match(data, "tui.select.cancel") {
            self.typing = None;
            return;
        }
        if keybindings_match(data, "tui.input.submit") || data == "\n" {
            let text = self
                .typing
                .as_ref()
                .map(|input| input.get_value().trim().to_owned())
                .unwrap_or_default();
            if !text.is_empty() {
                self.record(Vec::new(), Some(text));
            }
            return;
        }
        if let Some(input) = self.typing.as_mut() {
            input.handle_input(data);
        }
    }
}

impl Component for QuestionDialogComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let theme = theme();
        let mut lines: Vec<Line> = Vec::new();
        lines.extend(DynamicBorder::new(None).render(width));
        let Some(question) = self.current().cloned() else {
            return lines;
        };

        let mut heading = theme.fg(ThemeColor::Warning, &theme.bold("Question"));
        if self.questions.len() > 1 {
            heading.push_str(&theme.fg(
                ThemeColor::Muted,
                &format!(" {}/{}", self.index + 1, self.questions.len()),
            ));
        }
        if !question.header.is_empty() {
            heading.push_str(&theme.fg(ThemeColor::Muted, &format!(" · {}", question.header)));
        }
        lines.extend(Text::new(heading, 1, 0).render(width));
        lines.extend(Text::new(theme.bold(&question.question), 1, 0).render(width));

        if let Some(input) = self.typing.as_mut() {
            lines
                .extend(Text::new(theme.fg(ThemeColor::Muted, "Your answer:"), 1, 0).render(width));
            lines.extend(
                input
                    .render(width.saturating_sub(1))
                    .into_iter()
                    .map(|line| Line::from(format!(" {line}"))),
            );
            let hint = format!(
                "{}  {}",
                key_hint("tui.input.submit", "submit"),
                key_hint("tui.select.cancel", "back to the options"),
            );
            lines.extend(Text::new(hint, 1, 0).render(width));
        } else {
            for (row, option) in question.options.iter().enumerate() {
                let selected = row == self.cursor;
                let marker = if selected { "→ " } else { "  " };
                let checkbox = if question.multi_select {
                    if self.checked.get(row).copied().unwrap_or(false) {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                } else {
                    ""
                };
                let label = format!("{marker}{checkbox}{}", option.label);
                let label = if selected {
                    theme.fg(ThemeColor::Accent, &label)
                } else {
                    label
                };
                let text = if option.description.is_empty() {
                    label
                } else {
                    format!(
                        "{label}  {}",
                        theme.fg(ThemeColor::Muted, &option.description)
                    )
                };
                lines.extend(Text::new(text, 1, 0).render(width));
            }
            let own_selected = self.cursor >= question.options.len();
            let own = format!(
                "{}{OWN_ANSWER_LABEL}",
                if own_selected { "→ " } else { "  " }
            );
            let own = if own_selected {
                theme.fg(ThemeColor::Accent, &own)
            } else {
                theme.fg(ThemeColor::Muted, &own)
            };
            lines.extend(Text::new(own, 1, 0).render(width));

            let mut hints = vec![key_hint("tui.select.confirm", "choose")];
            if question.multi_select {
                hints.push(theme.fg(ThemeColor::Dim, "space toggle"));
            }
            hints.push(key_hint("tui.select.cancel", "dismiss"));
            lines.extend(Text::new(hints.join("  "), 1, 0).render(width));
        }

        lines.extend(DynamicBorder::new(None).render(width));
        lines
    }

    fn invalidate(&mut self) {}

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }

    fn handle_input(&mut self, data: &str) {
        if self.on_done.is_none() {
            return;
        }
        if self.typing.is_some() {
            self.handle_typing(data);
            return;
        }
        let rows = self.row_count();
        if keybindings_match(data, "tui.select.up") {
            self.cursor = if self.cursor == 0 {
                rows - 1
            } else {
                self.cursor - 1
            };
        } else if keybindings_match(data, "tui.select.down") {
            self.cursor = (self.cursor + 1) % rows;
        } else if keybindings_match(data, "tui.select.confirm") {
            self.confirm_row();
        } else if keybindings_match(data, "tui.select.cancel") {
            self.finish(None);
        } else if data == " "
            && self.current().is_some_and(|question| question.multi_select)
            && let Some(checked) = self.checked.get_mut(self.cursor)
        {
            *checked = !*checked;
        }
    }
}

impl Focusable for QuestionDialogComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if let Some(input) = self.typing.as_mut() {
            input.set_focused(focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::core::user_questions::QuestionOption;

    const ENTER: &str = "\r";
    const DOWN: &str = "\x1b[B";
    const ESCAPE: &str = "\x1b";

    fn question(id: &str, multi_select: bool) -> UserQuestion {
        UserQuestion {
            id: id.to_owned(),
            header: String::new(),
            question: format!("{id}?"),
            options: ["A", "B", "C"]
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).to_owned(),
                    description: String::new(),
                })
                .collect(),
            multi_select,
        }
    }

    type Outcome = Rc<RefCell<Option<Option<Vec<QuestionAnswer>>>>>;

    fn dialog(questions: Vec<UserQuestion>) -> (QuestionDialogComponent, Outcome) {
        let outcome: Outcome = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&outcome);
        let dialog = QuestionDialogComponent::new(
            questions,
            Box::new(move |answers| *sink.borrow_mut() = Some(answers)),
        );
        (dialog, outcome)
    }

    #[test]
    fn answers_every_question_in_order_before_reporting() {
        let (mut dialog, outcome) =
            dialog(vec![question("first", false), question("second", false)]);
        dialog.handle_input(DOWN);
        dialog.handle_input(ENTER);
        assert!(
            outcome.borrow().is_none(),
            "the dialog must not report before the last question is answered"
        );
        dialog.handle_input(ENTER);
        let answers = outcome.borrow().clone().flatten().expect("answered");
        assert_eq!(answers[0].selected, vec!["B"]);
        assert_eq!(answers[1].selected, vec!["A"]);
        assert_eq!(answers[1].id, "second");
    }

    #[test]
    fn a_multi_select_question_reports_every_ticked_option() {
        let (mut dialog, outcome) = dialog(vec![question("pick", true)]);
        dialog.handle_input(" ");
        dialog.handle_input(DOWN);
        dialog.handle_input(DOWN);
        dialog.handle_input(" ");
        dialog.handle_input(ENTER);
        let answers = outcome.borrow().clone().flatten().expect("answered");
        assert_eq!(answers[0].selected, vec!["A", "C"]);
    }

    #[test]
    fn escape_dismisses_the_whole_dialog_even_after_a_partial_answer() {
        let (mut dialog, outcome) =
            dialog(vec![question("first", false), question("second", false)]);
        dialog.handle_input(ENTER);
        dialog.handle_input(ESCAPE);
        assert_eq!(
            *outcome.borrow(),
            Some(None),
            "a dismissal must report no answers, not the ones given so far"
        );
    }

    #[test]
    fn typed_text_becomes_the_answer_and_escape_while_typing_goes_back() {
        let (mut dialog, outcome) = dialog(vec![question("own", false)]);
        for _ in 0..3 {
            dialog.handle_input(DOWN);
        }
        dialog.handle_input(ENTER);
        dialog.handle_input(ESCAPE);
        assert!(
            outcome.borrow().is_none(),
            "escape while typing must not dismiss"
        );
        dialog.handle_input(ENTER);
        for key in ["M", "y", "S", "Q", "L"] {
            dialog.handle_input(key);
        }
        dialog.handle_input(ENTER);
        let answers = outcome.borrow().clone().flatten().expect("answered");
        assert_eq!(answers[0].custom.as_deref(), Some("MySQL"));
        assert!(answers[0].selected.is_empty());
    }
}
