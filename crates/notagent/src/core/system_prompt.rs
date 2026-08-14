//! Port of `packages/coding-agent/src/core/system-prompt.ts`.
//!
//! Assembles the system prompt. The order of the sections is the contract: a
//! provider caches on prefixes, so anything that changes between turns has to
//! sit behind everything that does not. Tools, guidelines and the documentation
//! pointers are stable for a session; the appended prompt, the project context
//! files, the skills index and finally the working directory follow.
//!
//! A custom prompt replaces the whole built-in body but keeps the same tail, so
//! a user who supplies one still gets project context, skills and the cwd.

use crate::config::{get_docs_path, get_examples_path, get_readme_path};
use crate::core::skills::{Skill, format_skills_for_prompt};

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
    /// Tools to include in the prompt. Default: the seven built-in editing tools.
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
const DEFAULT_TOOLS: [&str; 7] = [
    "read",
    "read_minified",
    "bash",
    "edit",
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

    if let Some(custom_prompt) = options.custom_prompt.as_deref() {
        let mut prompt = custom_prompt.to_string();

        if !append_section.is_empty() {
            prompt.push_str(&append_section);
        }

        append_project_context(&mut prompt, &options.context_files);

        // Skills are only listed when the model has a way to read them.
        let custom_prompt_has_read = match options.selected_tools.as_ref() {
            None => true,
            Some(tools) => tools.iter().any(|name| name == "read"),
        };
        if custom_prompt_has_read && !options.skills.is_empty() {
            prompt.push_str(&format_skills_for_prompt(&options.skills));
        }

        prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}\n"));

        return prompt;
    }

    let readme_path = get_readme_path();
    let docs_path = get_docs_path();
    let examples_path = get_examples_path();

    // A tool appears in "Available tools" only when the caller supplies a
    // one-line snippet for it.
    let tools: Vec<String> = match options.selected_tools.as_ref() {
        Some(tools) => tools.clone(),
        None => DEFAULT_TOOLS.iter().map(|name| name.to_string()).collect(),
    };
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

    let has = |name: &str| tools.iter().any(|tool| tool == name);
    let has_bash = has("bash");
    let has_grep = has("grep");
    let has_find = has("find");
    let has_ls = has("ls");
    let has_read = has("read");

    if has_bash && !has_grep && !has_find && !has_ls {
        add_guideline(
            "Use bash for file operations like ls, rg, find".to_string(),
            &mut guidelines_list,
        );
    }

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
        "You are an expert coding assistant operating inside notagent, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

Available tools:
{tools_list}

In addition to the tools above, you may have access to other custom tools depending on the project.

Operating modes:
A user message may begin with a <mode> block. It is injected by the system when the operating mode changes and is not part of what the user wrote. It names the mode and the shell that bounds which tools you have. The most recent <mode> block is the one in force; every earlier one is superseded and no longer applies. Follow the active block without replying to it, mentioning it, or quoting it back.

Guidelines:
{guidelines}

Notagent documentation (read only when the user asks about notagent itself, its SDK, extensions, themes, skills, or TUI):
- Main documentation: {readme}
- Additional docs: {docs}
- Examples: {examples} (extensions, custom tools, SDK)
- When reading notagent docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory
- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), notagent packages (docs/packages.md), environment variables (docs/environment-variables.md)
- When working on notagent topics, read the docs and examples, and follow .md cross-references before implementing
- Always read notagent .md files completely and follow links to related docs (e.g., tui.md for TUI API details)",
        readme = readme_path.display(),
        docs = docs_path.display(),
        examples = examples_path.display(),
    );

    if !append_section.is_empty() {
        prompt.push_str(&append_section);
    }

    append_project_context(&mut prompt, &options.context_files);

    if has_read && !options.skills.is_empty() {
        prompt.push_str(&format_skills_for_prompt(&options.skills));
    }

    prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}"));

    prompt
}
