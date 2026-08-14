//! Port of `packages/coding-agent/src/core/tools/skill.ts` (tool half).
//!
//! The `skill` tool: loads a skill or mode body on demand.
//!
//! Two callers share this one path. The model invokes it to pull guidance it
//! decides is relevant — that is the lazy load, and it is why skill bodies no
//! longer need to sit in the system prompt. The runtime invokes it when the
//! operating mode changes, synthesizing the call so activation is a hard
//! trigger rather than something the model must remember to do.
//!
//! Both produce the same envelope, so a mode's guidance reads identically
//! regardless of who asked for it.
//!
//! Modes come from `core/modes.rs` as they do in TypeScript. Deviation
//! (class 1): skills are still resolved against the small view below rather
//! than against `Skill` of `core/skills.ts`, which lands with plan task 11. It
//! carries exactly the fields the TS `resolve` reads, so wiring the real type
//! onto it is a mapping; the resolution order, the case handling, the envelope
//! and the resource listing are unchanged.
//!
//! `renderCall`/`renderResult` need the theme and are wired in task 13.

use std::path::Path;
use std::sync::Arc;

use notagent_agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, BoxFuture, ToolExecutionError,
};
use notagent_ai::types::{ConstrainedSampling, TextContent, TextOrImageContent};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::core::experimental::get_experimental_tool_sampling;
use crate::core::modes::indicator::estimate_injected_tokens;
use crate::core::modes::{Mode, render_mode_injection};
use crate::core::tools::tool_definition::{
    SystemPromptContribution, ToolContext, ToolDefinition, wrap_tool_definition,
};

pub const SKILL_TOOL_SYSTEM_PROMPT_CONTRIBUTION: SystemPromptContribution =
    SystemPromptContribution {
        snippet: "Load a skill or mode's full instructions on demand",
        guidelines: &[
            "Load a skill with the skill tool when the task matches its description, instead of guessing at its content.",
        ],
    };

fn skill_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": "Name of the skill or mode to load." },
        },
        "required": ["name"],
    })
}

const DESCRIPTION: &str = concat!(
    "Loads the full instructions for a named skill or operating mode.\n",
    "\n",
    "Only names listed in the available skills section can be loaded. Do not load a skill whose instructions are already present in the conversation.\n",
    "The result is delimited and carries the source path; resolve any relative path the instructions mention against that path.\n",
    "Sibling files are listed by name only — read one with the read tool when the instructions call for it."
);

/// A skill, as the tool reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillToolSkill {
    pub name: String,
    pub file_path: String,
}

/// What the tool can resolve a name against. Supplied by the session.
#[derive(Clone)]
pub struct SkillToolSources {
    pub skills: Arc<dyn Fn() -> Vec<SkillToolSkill> + Send + Sync>,
    pub modes: Arc<dyn Fn() -> Vec<Mode> + Send + Sync>,
}

impl Default for SkillToolSources {
    /// Empty sources, so the tool is constructible before a session supplies
    /// real ones.
    fn default() -> Self {
        Self {
            skills: Arc::new(Vec::new),
            modes: Arc::new(Vec::new),
        }
    }
}

struct ResolvedBody {
    name: String,
    path: String,
    body: String,
    resources: Vec<String>,
}

/// Lists sibling files of a skill without reading them. Large assets stay out
/// of context until the model asks for one by path.
fn list_resources(file_path: &str) -> Vec<String> {
    let path = Path::new(file_path);
    let Some(directory) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    // `localeCompare(a, b, "en")` on plain file names orders like a
    // case-insensitive comparison with ties broken by the raw bytes.
    names.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });

    let mut resources = Vec::new();
    for name in names {
        let full = directory.join(&name);
        if full == path {
            continue;
        }
        match std::fs::metadata(&full) {
            Ok(metadata) if metadata.is_dir() => continue,
            Ok(_) => {}
            Err(_) => continue,
        }
        resources.push(name);
    }
    resources
}

fn resolve(name: &str, sources: &SkillToolSources) -> Option<ResolvedBody> {
    let wanted = name.trim().to_lowercase();

    if let Some(mode) = (sources.modes)()
        .into_iter()
        .find(|candidate| candidate.id.to_lowercase() == wanted)
    {
        let resources = mode
            .skills
            .first()
            .map(|skill| list_resources(&skill.file_path.to_string_lossy()))
            .unwrap_or_default();
        return Some(ResolvedBody {
            name: mode.id.clone(),
            path: mode.source_dir.to_string_lossy().into_owned(),
            body: render_mode_injection(&mode),
            resources,
        });
    }

    let skill = (sources.skills)()
        .into_iter()
        .find(|candidate| candidate.name.to_lowercase() == wanted)?;
    let body = std::fs::read_to_string(&skill.file_path).unwrap_or_default();
    let resources = list_resources(&skill.file_path);
    Some(ResolvedBody {
        name: skill.name,
        path: skill.file_path,
        body,
        resources,
    })
}

/// Wraps the body in a delimited envelope carrying its source path, so the
/// model can tell loaded guidance from conversation text and can resolve the
/// skill's relative resource references.
fn render_skill_envelope(resolved: &ResolvedBody) -> String {
    let mut lines = vec![
        format!(
            "<skill name=\"{}\" path=\"{}\">",
            resolved.name, resolved.path
        ),
        resolved.body.trim().to_owned(),
    ];
    if !resolved.resources.is_empty() {
        lines.push("<resources>".to_owned());
        for resource in &resolved.resources {
            lines.push(resource.clone());
        }
        lines.push("</resources>".to_owned());
    }
    lines.push("</skill>".to_owned());
    lines.join("\n")
}

fn available_names(sources: &SkillToolSources) -> Vec<String> {
    let mut names: Vec<String> = (sources.modes)()
        .into_iter()
        .map(|mode| mode.id)
        .chain((sources.skills)().into_iter().map(|skill| skill.name))
        .collect();
    names.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    names
}

pub struct SkillToolDefinition {
    sources: SkillToolSources,
    parameters: Value,
    constrained_sampling: Option<ConstrainedSampling>,
}

pub fn create_skill_tool_definition(sources: Option<SkillToolSources>) -> SkillToolDefinition {
    SkillToolDefinition {
        sources: sources.unwrap_or_default(),
        parameters: skill_schema(),
        constrained_sampling: get_experimental_tool_sampling(),
    }
}

impl ToolDefinition for SkillToolDefinition {
    fn name(&self) -> &str {
        "skill"
    }

    fn label(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(SKILL_TOOL_SYSTEM_PROMPT_CONTRIBUTION.snippet)
    }

    fn prompt_guidelines(&self) -> Vec<String> {
        SKILL_TOOL_SYSTEM_PROMPT_CONTRIBUTION
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

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        params: Value,
        _signal: Option<CancellationToken>,
        _on_update: Option<AgentToolUpdateCallback>,
        _context: Option<ToolContext>,
    ) -> BoxFuture<'a, Result<AgentToolResult, ToolExecutionError>> {
        Box::pin(async move {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let Some(resolved) = resolve(&name, &self.sources) else {
                let names = available_names(&self.sources);
                let known = if names.is_empty() {
                    "(none loaded)".to_owned()
                } else {
                    names.join(", ")
                };
                return Err(ToolExecutionError::new(format!(
                    "Unknown skill \"{name}\". Available: {known}"
                )));
            };
            let text = render_skill_envelope(&resolved);
            let mut details = Map::new();
            details.insert("name".to_owned(), Value::String(resolved.name.clone()));
            details.insert(
                "tokens".to_owned(),
                json!(estimate_injected_tokens(&resolved.body)),
            );
            details.insert("resources".to_owned(), json!(resolved.resources));
            Ok(AgentToolResult {
                content: vec![TextOrImageContent::Text(TextContent::new(text))],
                details: Some(Value::Object(details)),
                usage: None,
                added_tool_names: None,
                terminate: None,
            })
        })
    }
}

/// `createCreateSkillTool` — the tool as the agent loop takes it.
pub fn create_skill_tool(sources: Option<SkillToolSources>) -> Arc<dyn AgentTool> {
    wrap_tool_definition(Arc::new(create_skill_tool_definition(sources)), None)
}
