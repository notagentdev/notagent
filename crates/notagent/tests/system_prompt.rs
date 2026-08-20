//! Ported from `packages/coding-agent/test/system-prompt.test.ts`.

use notagent::core::system_prompt::{BuildSystemPromptOptions, build_system_prompt};

fn cwd() -> String {
    std::env::current_dir()
        .expect("cwd")
        .to_string_lossy()
        .into_owned()
}

fn options() -> BuildSystemPromptOptions {
    BuildSystemPromptOptions {
        cwd: cwd(),
        ..BuildSystemPromptOptions::default()
    }
}

fn tools(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|name| name.to_string()).collect())
}

fn snippets(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(name, snippet)| (name.to_string(), snippet.to_string()))
        .collect()
}

#[test]
fn shows_none_for_an_empty_tool_list() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        selected_tools: tools(&[]),
        ..options()
    });

    assert!(prompt.contains("Available tools:\n(none)"));
}

#[test]
fn shows_the_file_path_guideline_even_with_no_tools() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        selected_tools: tools(&[]),
        ..options()
    });

    assert!(prompt.contains("Show file paths clearly"));
}

#[test]
fn lists_every_default_tool_that_has_a_snippet() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        tool_snippets: snippets(&[
            ("read", "Read file contents"),
            ("bash", "Execute bash commands"),
            ("patch", "Make surgical edits"),
            ("write", "Create or overwrite files"),
        ]),
        ..options()
    });

    assert!(prompt.contains("- read:"));
    assert!(prompt.contains("- bash:"));
    assert!(prompt.contains("- patch:"));
    assert!(prompt.contains("- write:"));
}

#[test]
fn tells_the_model_where_the_notagent_docs_live() {
    let prompt = build_system_prompt(&options());

    assert!(prompt.contains(
        "- When reading notagent docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory"
    ));
    assert!(prompt.contains("environment variables (docs/environment-variables.md)"));
}

#[test]
fn lists_a_custom_tool_when_it_supplies_a_snippet() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        selected_tools: tools(&["read", "dynamic_tool"]),
        tool_snippets: snippets(&[("dynamic_tool", "Run dynamic test behavior")]),
        ..options()
    });

    assert!(prompt.contains("- dynamic_tool: Run dynamic test behavior"));
}

#[test]
fn omits_a_custom_tool_that_supplies_no_snippet() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        selected_tools: tools(&["read", "dynamic_tool"]),
        ..options()
    });

    assert!(!prompt.contains("dynamic_tool"));
}

#[test]
fn appends_prompt_guidelines_to_the_defaults() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        selected_tools: tools(&["read", "dynamic_tool"]),
        prompt_guidelines: vec!["Use dynamic_tool for project summaries.".to_string()],
        ..options()
    });

    assert!(prompt.contains("- Use dynamic_tool for project summaries."));
}

#[test]
fn deduplicates_and_trims_prompt_guidelines() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        selected_tools: tools(&["read", "dynamic_tool"]),
        prompt_guidelines: vec![
            "Use dynamic_tool for summaries.".to_string(),
            "  Use dynamic_tool for summaries.  ".to_string(),
            "   ".to_string(),
        ],
        ..options()
    });

    assert_eq!(
        prompt.matches("- Use dynamic_tool for summaries.").count(),
        1
    );
}
