use std::sync::Arc;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::events::HookEvent;
use super::payload::{HookSessionContext, build_payload};
use super::runner::{
    HookRunResult, HookStdout, HookVerdict, HookVerdictOutcome, decide_hooks, hook_context,
    is_hook_fault, run_hooks,
};
use super::{Hook, HookDiagnostic, select_hooks};

/// How a message reaches the user. Mirrors the levels the UI already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookReportLevel {
    Info,
    Warning,
    Error,
}

pub type HookReporter = Arc<dyn Fn(&str, HookReportLevel) + Send + Sync>;
/// Read at emit time, since a session's id and file appear after startup.
pub type HookContextSource = Arc<dyn Fn() -> HookSessionContext + Send + Sync>;
/// Cancels running hooks when the turn is aborted.
pub type HookSignalSource = Arc<dyn Fn() -> Option<CancellationToken> + Send + Sync>;

pub struct HookRuntimeOptions {
    pub hooks: Vec<Hook>,
    pub diagnostics: Vec<HookDiagnostic>,
    pub context: HookContextSource,
    pub report: Option<HookReporter>,
    pub signal: Option<HookSignalSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookContextOutput {
    pub event: HookEvent,
    pub declaration: usize,
    pub text: String,
}

/// Describes one failed run for the user, naming the hook and what it wrote.
pub fn describe_failure(result: &HookRunResult) -> String {
    if let HookStdout::Invalid(error) = &result.output {
        return format!(
            "{} hook produced {error} ({})",
            result.hook.event, result.hook.command
        );
    }
    let text = if result.stderr.trim().is_empty() {
        result.stdout.trim()
    } else {
        result.stderr.trim()
    };
    let written = text.split('\n').next().unwrap_or("").trim();
    let outcome = if result.cancelled {
        "was cancelled".to_string()
    } else if result.timed_out {
        format!("timed out after {}ms", result.hook.timeout_ms)
    } else {
        format!(
            "exited with {}",
            result
                .exit_code
                .map_or_else(|| "no status".to_string(), |code| code.to_string())
        )
    };
    let detail = if written.is_empty() {
        String::new()
    } else {
        format!(": {written}")
    };
    format!(
        "{} hook {outcome}{detail} ({})",
        result.hook.event, result.hook.command
    )
}

pub struct HookRuntime {
    hooks: Vec<Hook>,
    diagnostics: Vec<HookDiagnostic>,
    context: HookContextSource,
    report: Option<HookReporter>,
    signal: Option<HookSignalSource>,
}

impl HookRuntime {
    pub fn new(options: HookRuntimeOptions) -> Self {
        Self {
            hooks: options.hooks,
            diagnostics: options.diagnostics,
            context: options.context,
            report: options.report,
            signal: options.signal,
        }
    }

    fn report(&self, message: &str, level: HookReportLevel) {
        if let Some(report) = &self.report {
            report(message, level);
        }
    }

    fn signal(&self) -> Option<CancellationToken> {
        self.signal.as_ref().and_then(|signal| signal())
    }

    /// Whether anything at all is declared, so callers can skip work entirely.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Whether an event would run anything. Cheap enough to call per tool call.
    pub fn has(&self, event: HookEvent, tool_name: Option<&str>) -> bool {
        !select_hooks(&self.hooks, event, tool_name).is_empty()
    }

    /// Shows what was rejected or accepted with a caveat at load time. Called
    /// once the UI exists, since a diagnostic printed before it does is lost.
    pub fn report_diagnostics(&self) {
        for diagnostic in &self.diagnostics {
            self.report(
                &format!(
                    "Hook declaration in {}: {}",
                    diagnostic.source, diagnostic.message
                ),
                HookReportLevel::Warning,
            );
        }
    }

    /// Runs everything declared for an event. Failures are reported and then
    /// dropped: a formatter that broke must be visible, but it must not take the
    /// turn down with it.
    /// Returns what the hooks wrote to standard output, which the caller may
    /// hand to the model as context. That is what makes an event like
    /// `UserPromptSubmit` worth declaring at all — a hook that can only act on
    /// the world but never tell the agent anything is half a mechanism.
    pub async fn emit(
        &self,
        event: HookEvent,
        fields: Map<String, Value>,
        tool_name: Option<&str>,
    ) -> Vec<HookContextOutput> {
        let selected = select_hooks(&self.hooks, event, tool_name);
        if selected.is_empty() {
            return Vec::new();
        }
        let payload = Value::Object(build_payload(event, &(self.context)(), fields));
        // outside the hook itself breaks; `run_hooks` cannot fail here.
        let results = run_hooks(&selected, &payload, self.signal().as_ref()).await;

        let mut context = Vec::new();
        for (index, result) in results.iter().enumerate() {
            if is_hook_fault(result) {
                self.report(&describe_failure(result), HookReportLevel::Warning);
            }
            if let HookStdout::Structured(output) = &result.output
                && output.decision.is_some()
                && !event.is_blocking()
            {
                self.report(
                    &format!("Hook event {event} does not support decisions; decision ignored"),
                    HookReportLevel::Warning,
                );
            }
            if !result.ok {
                continue;
            }
            // Only a successful hook contributes context. Output from one that
            // failed is an error message, and feeding a diagnostic to the model
            // as though it were information is how a broken hook starts steering
            // the conversation.
            if let Some(text) = hook_context(result) {
                context.push(HookContextOutput {
                    event,
                    declaration: index + 1,
                    text,
                });
            }
        }
        context
    }

    /// Runs one block-capable event and returns what its hooks decided.
    /// A refusal is not reported here — the permission chain reports it as the
    /// decision it is. A fault is: a hook that could not run has stopped
    /// enforcing whatever it was written to enforce, and the author believes it
    /// is still in force.
    pub async fn decide(
        &self,
        event: HookEvent,
        fields: Map<String, Value>,
        tool_name: Option<&str>,
    ) -> HookVerdictOutcome {
        if !event.is_blocking() {
            self.report(
                &format!("Hook event {event} does not support decisions"),
                HookReportLevel::Error,
            );
            return HookVerdictOutcome {
                verdict: HookVerdict::Abstain,
                hook: None,
                results: Vec::new(),
                cancelled: false,
            };
        }
        let selected = select_hooks(&self.hooks, event, tool_name);
        if selected.is_empty() {
            return HookVerdictOutcome {
                verdict: HookVerdict::Abstain,
                hook: None,
                results: Vec::new(),
                cancelled: false,
            };
        }
        let payload = Value::Object(build_payload(event, &(self.context)(), fields));
        let outcome = decide_hooks(&selected, &payload, self.signal().as_ref()).await;
        for result in &outcome.results {
            if is_hook_fault(result) {
                self.report(&describe_failure(result), HookReportLevel::Warning);
            }
        }
        outcome
    }
}
