//! Lets the model stop and ask the user a structured question.
//! The tool holds no dialog of its own: it validates, hands the questions to
//! the presenter the interactive mode installed, and waits. Where nobody can
//! answer, it fails with an instruction to decide instead. In auto and yolo the
//! permission chain refuses it, because a model that loses its approval
//! prompts would otherwise reintroduce them as questions. It is not removed
//! from those modes' tool list: the list stays identical across worker modes
//! so a mode switch does not invalidate the prompt cache.

use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use notagent_tui::tui::ComponentRef;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::tools::render_utils::call_title;
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, ToolRenderContext, ToolRenderResult,
    ToolRenderResultOptions, render_text_call, render_text_result, wrap_tool_definition,
};
use crate::core::user_questions::{
    QuestionAnswer, QuestionPresenterSource, UserQuestion, format_answers, validate_questions,
};
use crate::modes::interactive::theme::theme::{Theme, ThemeColor};

pub const ASK_USER_QUESTION_TOOL_NAME: &str = "ask_user_question";

pub const ASK_USER_QUESTION_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Ask the user a structured multiple-choice question",
        guidelines: &[
            "Use ask_user_question only when a decision is genuinely the user's and you cannot resolve it from the request, the code, or a sensible default; otherwise decide and say which reading you chose.",
        ],
    };

const DESCRIPTION: &str = concat!(
    "Ask the user one to four multiple-choice questions and wait for the answers.\n",
    "\n",
    "Use it when you are blocked on a decision that belongs to the user: two plausible readings of the request, a trade-off only they can weigh, or missing information you cannot find. Do not use it to ask permission for a tool call, to confirm a plan you could simply carry out, or for anything the code or a sensible default already answers.\n",
    "\n",
    "Rules:\n",
    "- Each question has a stable snake_case `id`; the answer echoes it.\n",
    "- Offer 2-4 distinct options with a 1-5 word `label` and a one-sentence `description` of the trade-off.\n",
    "- If you recommend one, put it first and append \" (Recommended)\" to its label.\n",
    "- Do not add an \"Other\" option: the dialog always lets the user type their own answer.\n",
    "- Set `multi_select` only when the options are not mutually exclusive.\n",
    "\n",
    "If the user dismisses the dialog, you get no answer; do not invent one. If nobody can be asked in this session, the call fails and you decide yourself."
);

fn ask_user_question_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "description": "The questions to ask, 1-4. Prefer one.",
                "minItems": 1,
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Stable snake_case identifier, echoed in the answer." },
                        "header": { "type": "string", "description": "Short tag shown above the question, at most 12 characters, e.g. \"Database\"." },
                        "question": { "type": "string", "description": "One specific question, ending with '?'." },
                        "options": {
                            "type": "array",
                            "description": "2-4 distinct choices. No \"Other\" option; free text is always offered.",
                            "minItems": 2,
                            "maxItems": 4,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string", "description": "1-5 words. Append \" (Recommended)\" to the one you recommend." },
                                    "description": { "type": "string", "description": "One sentence on the trade-off of this choice." },
                                },
                                "required": ["label", "description"],
                            },
                        },
                        "multi_select": { "type": "boolean", "description": "Whether several options may be chosen. Defaults to false." },
                    },
                    "required": ["id", "question", "options"],
                },
            },
        },
        "required": ["questions"],
    })
}

#[derive(Clone)]
pub struct AskUserQuestionToolSources {
    /// Where the tool finds someone to ask. Absent everywhere but the
    /// interactive mode.
    pub presenter: QuestionPresenterSource,
}

impl Default for AskUserQuestionToolSources {
    /// Nobody to ask, so the tool is constructible before a session supplies
    /// a presenter and fails cleanly where none ever arrives.
    fn default() -> Self {
        Self {
            presenter: Arc::new(|| None),
        }
    }
}

pub struct AskUserQuestionToolDefinition {
    sources: AskUserQuestionToolSources,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_ask_user_question_tool_definition(
    sources: Option<AskUserQuestionToolSources>,
) -> AskUserQuestionToolDefinition {
    AskUserQuestionToolDefinition {
        sources: sources.unwrap_or_default(),
        parameters: ask_user_question_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

fn parse_questions(params: &Value) -> Result<Vec<UserQuestion>, String> {
    let raw = params
        .get("questions")
        .cloned()
        .ok_or_else(|| "`questions` is required.".to_owned())?;
    serde_json::from_value(raw).map_err(|error| format!("Invalid questions: {error}"))
}

fn text_result(text: String, details: Value) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageContent::Text(TextContent::new(text))],
        details: Some(details),
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

impl ToolDefinition for AskUserQuestionToolDefinition {
    fn name(&self) -> &str {
        ASK_USER_QUESTION_TOOL_NAME
    }

    fn label(&self) -> &str {
        ASK_USER_QUESTION_TOOL_NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(ASK_USER_QUESTION_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        ASK_USER_QUESTION_TOOL_SYSTEM_PROMPT_CONTRIBUTION
            .guidelines
            .iter()
            .map(|guideline| (*guideline).to_owned())
            .collect()
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn constrained_sampling(&self) -> Option<&ConstrainedSampling> {
        self.constrained_sampling.as_ref()
    }

    fn render_call(
        &self,
        args: &Value,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        let questions = args.get("questions").and_then(Value::as_array);
        let first = questions
            .and_then(|questions| questions.first())
            .and_then(|question| question.get("question"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let more = questions.map_or(0, Vec::len).saturating_sub(1);
        let suffix = if more > 0 {
            theme.fg(ThemeColor::Muted, &format!(" (+{more})"))
        } else {
            String::new()
        };
        Some(render_text_call(
            context,
            &format!("{}{first}{suffix}", call_title(theme, self.label())),
        ))
    }

    fn render_result(
        &self,
        result: ToolRenderResult<'_>,
        _options: ToolRenderResultOptions,
        theme: &Theme,
        context: &ToolRenderContext,
    ) -> Option<ComponentRef> {
        // A call that failed — invalid questions, a refusal in auto or yolo,
        // nobody to ask — is handed to the generic renderer, which shows the
        // error text; an empty component would hide it. The agent loop gives
        // an error result an empty details object, so the flag decides, not
        // the absence of details.
        if context.is_error {
            return None;
        }
        let details = result.details?;
        if details.get("dismissed").and_then(Value::as_bool) == Some(true) {
            return Some(render_text_result(
                context,
                &format!("\n{}", theme.fg(ThemeColor::Muted, "  dismissed")),
            ));
        }
        let questions: Vec<UserQuestion> = details
            .get("questions")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let answers: Vec<QuestionAnswer> = details
            .get("answers")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let lines: Vec<String> = questions
            .iter()
            .map(|question| {
                let answer = answers.iter().find(|answer| answer.id == question.id);
                let text = match answer {
                    Some(QuestionAnswer {
                        custom: Some(custom),
                        ..
                    }) => custom.clone(),
                    Some(answer) if !answer.selected.is_empty() => answer.selected.join(", "),
                    _ => "(no answer)".to_owned(),
                };
                format!(
                    "  {} {}",
                    theme.fg(ThemeColor::Muted, &format!("{}:", question.question)),
                    theme.fg(ThemeColor::Accent, &text)
                )
            })
            .collect();
        let text = if lines.is_empty() {
            String::new()
        } else {
            format!("\n{}", lines.join("\n"))
        };
        Some(render_text_result(context, &text))
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let questions = parse_questions(&params).map_err(ToolExecutionError::new)?;
            validate_questions(&questions).map_err(ToolExecutionError::new)?;
            let Some(present) = (self.sources.presenter)() else {
                return Err(ToolExecutionError::new(
                    "Nobody can answer questions in this session. Make a reasonable decision, say which one you made, and continue.",
                ));
            };

            let asked = present(questions.clone());
            let answers = match signal {
                Some(signal) => tokio::select! {
                    answers = asked => answers,
                    () = signal.cancelled() => {
                        return Err(ToolExecutionError::new("The question was cancelled."));
                    }
                },
                None => asked.await,
            };

            let questions_value = serde_json::to_value(&questions).unwrap_or(Value::Null);
            let Some(answers) = answers else {
                return Ok(text_result(
                    "The user dismissed the question without answering. Do not assume an answer: continue with what does not depend on it, or stop and explain what you need.".to_owned(),
                    json!({ "questions": questions_value, "dismissed": true }),
                ));
            };
            Ok(text_result(
                format_answers(&questions, &answers),
                json!({
                    "questions": questions_value,
                    "answers": serde_json::to_value(&answers).unwrap_or(Value::Null),
                }),
            ))
        })
    }
}

pub fn create_ask_user_question_tool(
    sources: Option<AskUserQuestionToolSources>,
) -> Arc<dyn AgentTool> {
    wrap_tool_definition(
        Arc::new(create_ask_user_question_tool_definition(sources)),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::user_questions::QuestionPresenter;

    fn sources(presenter: Option<QuestionPresenter>) -> AskUserQuestionToolSources {
        AskUserQuestionToolSources {
            presenter: Arc::new(move || presenter.clone()),
        }
    }

    fn params() -> Value {
        json!({ "questions": [{
            "id": "db",
            "question": "Which database?",
            "options": [
                { "label": "Postgres (Recommended)", "description": "Server." },
                { "label": "SQLite", "description": "File." },
            ],
        }]})
    }

    fn text_of(result: &AgentToolResult) -> String {
        match result.content.first() {
            Some(TextOrImageContent::Text(text)) => text.text.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn a_failed_call_is_left_to_the_generic_renderer_so_its_error_shows() {
        crate::modes::interactive::theme::theme::init_theme(None, false);
        let theme = crate::modes::interactive::theme::theme::theme();
        let tool = create_ask_user_question_tool_definition(None);
        let error = [TextOrImageContent::Text(TextContent::new(
            "Asking the user is disabled while the session runs unattended.",
        ))];
        // What the agent loop hands over for a failed call: an error flag and
        // an empty details object (`create_error_tool_result`).
        let empty_details = json!({});
        let mut context = ToolRenderContext::new("call", params(), "/tmp");
        context.is_error = true;
        let rendered = tool.render_result(
            ToolRenderResult {
                content: &error,
                details: Some(&empty_details),
            },
            ToolRenderResultOptions::default(),
            &theme,
            &context,
        );
        assert!(
            rendered.is_none(),
            "an error result must reach the generic renderer, which shows its text"
        );
    }

    #[tokio::test]
    async fn fails_with_an_instruction_to_decide_when_nobody_can_answer() {
        let tool = create_ask_user_question_tool_definition(Some(sources(None)));
        let error = tool
            .execute("call", params(), None, None, None)
            .await
            .expect_err("without a presenter the call must fail");
        assert!(
            error.to_string().contains("Make a reasonable decision"),
            "the failure must tell the model what to do instead; saw: {error}"
        );
    }

    #[tokio::test]
    async fn returns_the_chosen_label_keyed_by_the_question_id() {
        let presenter: QuestionPresenter = Arc::new(|questions: Vec<UserQuestion>| {
            Box::pin(async move {
                Some(vec![QuestionAnswer {
                    id: questions[0].id.clone(),
                    selected: vec!["SQLite".to_owned()],
                    custom: None,
                }])
            })
        });
        let tool = create_ask_user_question_tool_definition(Some(sources(Some(presenter))));
        let result = tool
            .execute("call", params(), None, None, None)
            .await
            .expect("answered");
        let text = text_of(&result);
        assert!(
            text.contains("- db (Which database?): SQLite"),
            "saw: {text}"
        );
    }

    #[tokio::test]
    async fn a_dismissal_is_reported_as_no_answer_rather_than_a_choice() {
        let presenter: QuestionPresenter =
            Arc::new(|_questions: Vec<UserQuestion>| Box::pin(async { None }));
        let tool = create_ask_user_question_tool_definition(Some(sources(Some(presenter))));
        let result = tool
            .execute("call", params(), None, None, None)
            .await
            .expect("a dismissal is a result, not a failure");
        assert!(
            text_of(&result).contains("Do not assume an answer"),
            "saw: {}",
            text_of(&result)
        );
    }

    #[tokio::test]
    async fn an_invalid_question_never_reaches_the_user() {
        let presenter: QuestionPresenter = Arc::new(|_questions: Vec<UserQuestion>| {
            Box::pin(async { panic!("an invalid question must not be presented") })
        });
        let tool = create_ask_user_question_tool_definition(Some(sources(Some(presenter))));
        let invalid = json!({ "questions": [{
            "id": "db", "question": "Which?",
            "options": [{ "label": "A" }, { "label": "A" }],
        }]});
        assert!(
            tool.execute("call", invalid, None, None, None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_cancelled_turn_does_not_wait_for_the_answer() {
        let presenter: QuestionPresenter = Arc::new(|_questions: Vec<UserQuestion>| {
            Box::pin(std::future::pending::<Option<Vec<QuestionAnswer>>>())
        });
        let tool = create_ask_user_question_tool_definition(Some(sources(Some(presenter))));
        let signal = CancellationToken::new();
        signal.cancel();
        assert!(
            tool.execute("call", params(), Some(signal), None, None)
                .await
                .is_err()
        );
    }
}
