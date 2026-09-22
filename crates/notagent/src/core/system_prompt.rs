use crate::config::DOCUMENTATION_URL;
use crate::core::skills::{SKILL_CRITICAL_LOADING_POLICY, Skill, format_skills_for_prompt};
use crate::core::tools::bash::BASH_CRITICAL_TOOL_POLICY;

/// One preloaded context file: `AGENTS.md`, `CLAUDE.md` and the like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: String,
    pub content: String,
}

/// Inputs of [`build_system_prompt`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildSystemPromptOptions {
    /// Custom system prompt; replaces the default body.
    pub custom_prompt: Option<String>,
    /// Tools to include in the prompt. Default: the six built-in editing tools.
    pub selected_tools: Option<Vec<String>>,
    /// One-line tool snippets keyed by tool name.
    pub tool_snippets: Vec<(String, String)>,
    /// Guideline bullets appended to the default guidelines.
    pub prompt_guidelines: Vec<String>,
    /// Text appended after the prompt body.
    pub append_system_prompt: Option<String>,
    pub cwd: String,
    pub context_files: Vec<ContextFile>,
    pub skills: Vec<Skill>,
}

impl BuildSystemPromptOptions {
    fn snippet(&self, name: &str) -> Option<&str> {
        self.tool_snippets
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The tools named when the caller names none.
const DEFAULT_TOOLS: [&str; 6] = [
    "read",
    "bash",
    "patch",
    "patch_minified",
    "multi_patch_minified",
    "write",
];

fn append_project_context(prompt: &mut String, context_files: &[ContextFile]) {
    if context_files.is_empty() {
        return;
    }
    prompt.push_str("\n\n<project_context>\n\n");
    prompt.push_str("Project-specific instructions and guidelines:\n\n");
    for file in context_files {
        prompt.push_str(&format!(
            "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
            file.path, file.content
        ));
    }
    prompt.push_str("</project_context>\n");
}

/// Builds the system prompt with tools, guidelines, and context.
pub fn build_system_prompt(options: &BuildSystemPromptOptions) -> String {
    let prompt_cwd = options.cwd.replace('\\', "/");
    let append_section = options
        .append_system_prompt
        .as_deref()
        .map(|text| format!("\n\n{text}"))
        .unwrap_or_default();

    let tools: Vec<String> = match options.selected_tools.as_ref() {
        Some(tools) => tools.clone(),
        None => DEFAULT_TOOLS.iter().map(|name| name.to_string()).collect(),
    };
    let has_skill = tools.iter().any(|name| name == "skill");
    let mut critical_tool_policy = if tools.iter().any(|name| name == "bash") {
        format!("{BASH_CRITICAL_TOOL_POLICY}\n\n")
    } else {
        String::new()
    };
    if has_skill {
        critical_tool_policy.push_str(SKILL_CRITICAL_LOADING_POLICY);
        critical_tool_policy.push_str("\n\n");
    }

    if let Some(custom_prompt) = options.custom_prompt.as_deref() {
        let mut prompt = format!("{critical_tool_policy}{custom_prompt}");

        if !append_section.is_empty() {
            prompt.push_str(&append_section);
        }

        append_project_context(&mut prompt, &options.context_files);

        if has_skill && !options.skills.is_empty() {
            prompt.push_str(&format_skills_for_prompt(&options.skills));
        }

        prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}\n"));

        return prompt;
    }

    // A tool appears in the reference only when the caller supplies a
    // one-line snippet for it.
    let visible_tools: Vec<&String> = tools
        .iter()
        .filter(|name| options.snippet(name).is_some())
        .collect();
    let tools_list = if visible_tools.is_empty() {
        "(none)".to_string()
    } else {
        visible_tools
            .iter()
            .map(|name| format!("- {}: {}", name, options.snippet(name).unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Guidelines are deduplicated in insertion order: a tool contributing the
    // same advice as another must not make it appear twice.
    let mut guidelines_list: Vec<String> = Vec::new();
    let add_guideline = |guideline: String, list: &mut Vec<String>| {
        if list.iter().any(|existing| existing == &guideline) {
            return;
        }
        list.push(guideline);
    };

    for guideline in &options.prompt_guidelines {
        let normalized = guideline.trim();
        if !normalized.is_empty() {
            add_guideline(normalized.to_string(), &mut guidelines_list);
        }
    }

    add_guideline(
        "Be concise in your responses".to_string(),
        &mut guidelines_list,
    );
    // Some models decorate every heading and list item with one. In a terminal
    // that is noise at best, and at worst it breaks alignment: an emoji is two
    // columns wide in some terminals and one in others, so a line that contains
    // one cannot be laid out reliably.
    add_guideline(
        "Never use emoji, anywhere: not in replies, headings, lists, commit messages, code, or comments".to_string(),
        &mut guidelines_list,
    );
    add_guideline(
        "Show file paths clearly when working with files".to_string(),
        &mut guidelines_list,
    );

    let guidelines = guidelines_list
        .iter()
        .map(|guideline| format!("- {guideline}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut prompt = format!(
        "{critical_tool_policy}You are an expert coding assistant operating inside notagent, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

Tool reference:
{tools_list}

Only tools attached to the current request are available to call. This reference may include tools unavailable in the current mode or subagent; apply tool-specific instructions only when that tool is attached. Project-specific tools may also be attached.

Operating modes:
A user message may begin with a <mode> block. It is injected by the system when the operating mode changes and is not part of what the user wrote. It names the mode and the shell that bounds which tools you have. The most recent <mode> block is the one in force; every earlier one is superseded and no longer applies. Follow the active block without replying to it, mentioning it, or quoting it back.

Task intent:
- Distinguish discussion, diagnosis, and change requests.
- Discussion requests authorize analysis only.
- Diagnosis requests authorize investigation and reporting, but not implementation.
- Only an explicit change request authorizes modifying the project.

Implementation discipline:
- Understand the task and trace the affected flow end to end before changing code.
- For an authorized change, carry the work through understanding, minimal implementation, and relevant verification.
- First decide whether any code change is needed at all.
- Reuse existing helpers and patterns; prefer the standard library, native platform features, and installed dependencies over new code or dependencies.
- Fix the root cause in the shared implementation instead of patching individual symptoms.
- Inspect the relevant callers so the change holds across the actual flow.
- Do not introduce unrequested abstractions, dependencies, compatibility paths, or adjacent refactors.
- Make the smallest correct change in the fewest files; if one clear line is enough, use it.
- Prefer deletion over addition and straightforward code over clever code.

Guidelines:
{guidelines}

Notagent documentation (read only when the user asks about notagent itself, its SDK, extensions, themes, skills, or TUI):
- Public documentation: {DOCUMENTATION_URL}
- When working on notagent topics, consult the public documentation before implementing",
    );

    if !append_section.is_empty() {
        prompt.push_str(&append_section);
    }

    append_project_context(&mut prompt, &options.context_files);

    if has_skill && !options.skills.is_empty() {
        prompt.push_str(&format_skills_for_prompt(&options.skills));
    }

    prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}"));

    prompt
}
