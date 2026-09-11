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
fn skills_are_listed_only_when_the_skill_tool_is_available_in_either_prompt_path() {
    let skills = notagent::core::skills::load_skills_from_dir(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/skills/valid-skill"
        ),
        "test",
    )
    .skills;
    assert_eq!(skills.len(), 1, "the fixture must provide a visible skill");
    for custom_prompt in [None, Some("Custom instructions".to_string())] {
        for (selected_tools, listed) in [
            (None, false),
            (tools(&[]), false),
            (tools(&["read"]), false),
            (tools(&["skill"]), true),
            (tools(&["read", "skill"]), true),
        ] {
            let prompt = build_system_prompt(&BuildSystemPromptOptions {
                custom_prompt: custom_prompt.clone(),
                selected_tools: selected_tools.clone(),
                skills: skills.clone(),
                ..options()
            });
            assert_eq!(
                prompt.contains("<available_skills>"),
                listed,
                "skill availability must control the catalog: custom={custom_prompt:?}, tools={selected_tools:?}"
            );
            assert_eq!(
                prompt.contains("Use the skill tool with the skill's name"),
                listed,
                "listed skills must be loaded through the skill tool: {prompt}"
            );
            assert!(
                !prompt.contains("Use the read tool to load a skill"),
                "the prompt must not offer a conflicting loading path: {prompt}"
            );
        }
    }
}

#[test]
fn shows_none_for_an_empty_tool_list() {
    let prompt = build_system_prompt(&BuildSystemPromptOptions {
        selected_tools: tools(&[]),
        ..options()
    });

    assert!(prompt.contains("Tool reference:\n(none)"));
}

#[test]
fn tool_guidance_is_scoped_to_the_tools_attached_to_each_request() {
    let prompt = build_system_prompt(&options());
    for rule in [
        "Only tools attached to the current request are available to call",
        "tools unavailable in the current mode or subagent",
        "apply tool-specific instructions only when that tool is attached",
    ] {
        assert!(
            prompt.contains(rule),
            "missing tool availability rule: {rule}"
        );
    }
    assert!(
        !prompt.contains("Available tools:"),
        "the reference must not claim current availability: {prompt}"
    );
}

#[test]
fn automatic_approval_does_not_forbid_material_clarification() {
    let auto = include_str!("../src/core/modes/builtin/auto/10-auto.md");
    assert!(
        auto.contains("ask when an unresolved choice would materially change the work"),
        "automatic approval must still allow necessary clarification: {auto}"
    );
    assert!(
        !auto.contains("Do not ask the user to choose"),
        "auto mode must not categorically forbid clarification: {auto}"
    );
}

#[test]
fn enabled_minified_tools_are_required_and_plain_patch_is_only_a_fallback() {
    use notagent::core::tools::{ToolName, create_tool_definition};

    for minified_enabled in [false, true] {
        let mut names = vec![ToolName::Read, ToolName::Edit];
        if minified_enabled {
            names.extend([
                ToolName::ReadMinified,
                ToolName::PatchMinified,
                ToolName::MultiPatchMinified,
            ]);
        }
        let definitions: Vec<_> = names
            .into_iter()
            .map(|name| create_tool_definition(name, &cwd(), None))
            .collect();
        let prompt = build_system_prompt(&BuildSystemPromptOptions {
            selected_tools: Some(
                definitions
                    .iter()
                    .map(|tool| tool.name().to_owned())
                    .collect(),
            ),
            prompt_guidelines: definitions
                .iter()
                .flat_map(|tool| tool.prompt_guidelines())
                .collect(),
            ..options()
        });
        assert!(
            prompt.contains("patch is a fallback only")
                && prompt.contains("when no minified editing tool is attached"),
            "patch must remain a fallback and account for disabled minified tools: {prompt}"
        );
        for tool in definitions
            .iter()
            .filter(|tool| tool.name().ends_with("_minified"))
        {
            assert!(
                prompt.contains(&format!(
                    "When {} is attached, you must use it",
                    tool.name()
                )),
                "the assembled prompt must require the enabled tool {}: {prompt}",
                tool.name()
            );
            assert!(
                tool.description().contains("mandatory, not a preference"),
                "tool descriptions must also require minified use: {}",
                tool.description()
            );
        }
        assert_eq!(
            prompt.contains("mandatory, not a preference"),
            minified_enabled,
            "disabled minified tools must not contribute mandatory-use instructions: {prompt}"
        );
        assert!(
            !prompt.contains("Prefer read_minified")
                && !prompt.contains("Use patch for precise changes"),
            "the prompt must not also recommend the plain path: {prompt}"
        );
    }
}

#[test]
fn planning_instructions_preserve_history_and_require_file_authorization() {
    let guidance = notagent::core::tools::plan_create::PLAN_CREATE_TOOL_SYSTEM_PROMPT_CONTRIBUTION
        .guidelines
        .join("\n");
    let skill = include_str!("../src/core/skills/builtin/create-plan/SKILL.md");
    for text in [&guidance, skill] {
        assert!(
            text.contains("user requests a plan file"),
            "file creation needs authorization: {text}"
        );
        assert!(
            text.contains("plans in saved conversations remain part of the session history"),
            "planning guidance must acknowledge persisted replies: {text}"
        );
        assert!(
            !text.contains("gone when the session ends")
                && !text.contains("lost when the session ends"),
            "planning guidance must not claim replies are lost: {text}"
        );
    }
    assert!(skill.contains("Loading this skill does not by itself authorize creating a file"));
}

#[test]
fn goal_guidance_accounts_for_budget_limits_and_user_pauses() {
    use notagent::core::tools::goal::{
        create_goal_tool_definition, create_update_goal_tool_definition,
    };
    use notagent::core::tools::tool_definition::ToolDefinition;

    let tool = create_update_goal_tool_definition(None);
    for text in [
        tool.description().to_owned(),
        create_goal_tool_definition(None)
            .prompt_guidelines()
            .join("\n"),
    ] {
        assert!(
            text.contains("budget limit") && text.contains("user pauses"),
            "goal continuation has external stop conditions: {text}"
        );
        assert!(
            !text.contains("only way to stop") && !text.contains("only update_goal stops"),
            "goal completion must not be confused with external stop conditions: {text}"
        );
    }
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
fn points_the_model_at_public_notagent_documentation() {
    let prompt = build_system_prompt(&options());

    assert!(prompt.contains("https://github.com/notagentdev/notagent#readme"));
    assert!(!prompt.contains("Additional docs:"));
}

#[test]
fn default_prompt_separates_discussion_diagnosis_and_changes() {
    let prompt = build_system_prompt(&options());

    for rule in [
        "Distinguish discussion, diagnosis, and change requests",
        "Discussion requests authorize analysis only",
        "Diagnosis requests authorize investigation and reporting, but not implementation",
        "Only an explicit change request authorizes modifying the project",
    ] {
        assert!(prompt.contains(rule), "missing task intent rule: {rule}");
    }
}

#[test]
fn default_prompt_requires_small_root_cause_changes() {
    let prompt = build_system_prompt(&options());

    for rule in [
        "trace the affected flow end to end",
        "carry the work through understanding, minimal implementation, and relevant verification",
        "whether any code change is needed at all",
        "Reuse existing helpers and patterns",
        "Fix the root cause in the shared implementation",
        "Inspect the relevant callers",
        "Do not introduce unrequested abstractions",
        "smallest correct change in the fewest files",
        "Prefer deletion over addition",
    ] {
        assert!(prompt.contains(rule), "missing implementation rule: {rule}");
    }
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
