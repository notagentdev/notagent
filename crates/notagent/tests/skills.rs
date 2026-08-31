use std::path::{Path, PathBuf};

use notagent::core::skills::{
    BUILTIN_SKILLS, LoadSkillsOptions, Skill, format_skills_for_prompt, load_skills,
    load_skills_from_dir, materialize_builtin_skills,
};
use notagent::core::source_info::{SyntheticSourceInfoOptions, create_synthetic_source_info};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills")
}

fn collision_fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills-collision")
}

fn fixture(name: &str) -> String {
    fixtures_dir().join(name).to_string_lossy().into_owned()
}

struct TestSkill {
    name: &'static str,
    description: &'static str,
    file_path: &'static str,
    base_dir: &'static str,
    disable_model_invocation: bool,
}

fn create_test_skill(options: TestSkill) -> Skill {
    Skill {
        name: options.name.to_string(),
        description: options.description.to_string(),
        file_path: options.file_path.to_string(),
        base_dir: options.base_dir.to_string(),
        source_info: create_synthetic_source_info(
            options.file_path,
            SyntheticSourceInfoOptions::new("test"),
        ),
        disable_model_invocation: options.disable_model_invocation,
    }
}

fn skill(
    name: &'static str,
    description: &'static str,
    path: &'static str,
    dir: &'static str,
) -> Skill {
    create_test_skill(TestSkill {
        name,
        description,
        file_path: path,
        base_dir: dir,
        disable_model_invocation: false,
    })
}

// ---------------------------------------------------------------- loadSkillsFromDir

#[test]
fn loads_a_valid_skill() {
    let result = load_skills_from_dir(&fixture("valid-skill"), "test");

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "valid-skill");
    assert_eq!(
        result.skills[0].description,
        "A valid skill for testing purposes."
    );
    assert_eq!(result.skills[0].source_info.source, "test");
    assert!(result.diagnostics.is_empty());
}

#[test]
fn allows_names_that_do_not_match_the_parent_directory() {
    let result = load_skills_from_dir(&fixture("name-mismatch"), "test");

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "different-name");
    assert!(!result.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("does not match parent directory")
    }));
}

#[test]
fn warns_when_the_name_contains_invalid_characters() {
    let result = load_skills_from_dir(&fixture("invalid-name-chars"), "test");

    assert_eq!(result.skills.len(), 1);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("invalid characters"))
    );
}

#[test]
fn warns_when_the_name_exceeds_64_characters() {
    let result = load_skills_from_dir(&fixture("long-name"), "test");

    assert_eq!(result.skills.len(), 1);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("exceeds 64 characters"))
    );
}

#[test]
fn warns_and_skips_the_skill_when_the_description_is_missing() {
    let result = load_skills_from_dir(&fixture("missing-description"), "test");

    assert!(result.skills.is_empty());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("description is required"))
    );
}

#[test]
fn ignores_unknown_frontmatter_fields() {
    let result = load_skills_from_dir(&fixture("unknown-field"), "test");

    assert_eq!(result.skills.len(), 1);
    assert!(result.diagnostics.is_empty());
}

#[test]
fn loads_nested_skills_recursively() {
    let result = load_skills_from_dir(&fixture("nested"), "test");

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "child-skill");
    assert!(result.diagnostics.is_empty());
}

#[test]
fn prefers_a_root_skill_file_over_nested_ones() {
    let result = load_skills_from_dir(&fixture("root-skill-preferred"), "test");

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "root-skill-preferred");
    assert_eq!(result.skills[0].description, "Root skill should win.");
    assert!(result.diagnostics.is_empty());
}

#[test]
fn skips_files_without_frontmatter() {
    let result = load_skills_from_dir(&fixture("no-frontmatter"), "test");

    // no-frontmatter has no description, so it is skipped.
    assert!(result.skills.is_empty());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("description is required"))
    );
}

#[test]
fn warns_and_skips_the_skill_when_the_yaml_is_invalid() {
    let result = load_skills_from_dir(&fixture("invalid-yaml"), "test");

    assert!(result.skills.is_empty());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("at line"))
    );
}

#[test]
fn preserves_multiline_descriptions_from_yaml() {
    let result = load_skills_from_dir(&fixture("multiline-description"), "test");

    assert_eq!(result.skills.len(), 1);
    assert!(result.skills[0].description.contains('\n'));
    assert!(
        result.skills[0]
            .description
            .contains("This is a multiline description.")
    );
    assert!(result.diagnostics.is_empty());
}

#[test]
fn warns_when_the_name_contains_consecutive_hyphens() {
    let result = load_skills_from_dir(&fixture("consecutive-hyphens"), "test");

    assert_eq!(result.skills.len(), 1);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("consecutive hyphens"))
    );
}

#[test]
fn loads_every_skill_in_the_fixture_directory() {
    let result = load_skills_from_dir(&fixtures_dir().to_string_lossy(), "test");

    // Every skill that has a description loads, warnings and all:
    // valid-skill, name-mismatch, invalid-name-chars, long-name, unknown-field,
    // nested/child-skill, consecutive-hyphens — but not missing-description or
    // no-frontmatter.
    assert!(result.skills.len() >= 6);
}

#[test]
fn returns_nothing_for_a_directory_that_does_not_exist() {
    let result = load_skills_from_dir("/non/existent/path", "test");

    assert!(result.skills.is_empty());
    assert!(result.diagnostics.is_empty());
}

#[test]
fn uses_the_parent_directory_name_when_the_frontmatter_has_none() {
    let result = load_skills_from_dir(&fixture("valid-skill"), "test");

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "valid-skill");
}

#[test]
fn parses_the_disable_model_invocation_field() {
    let result = load_skills_from_dir(&fixture("disable-model-invocation"), "test");

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "disable-model-invocation");
    assert!(result.skills[0].disable_model_invocation);
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown frontmatter field"))
    );
}

#[test]
fn defaults_disable_model_invocation_to_false() {
    let result = load_skills_from_dir(&fixture("valid-skill"), "test");

    assert_eq!(result.skills.len(), 1);
    assert!(!result.skills[0].disable_model_invocation);
}

// -------------------------------------------------------------- formatSkillsForPrompt

#[test]
fn formats_no_skills_as_the_empty_string() {
    assert_eq!(format_skills_for_prompt(&[]), "");
}

#[test]
fn formats_skills_as_xml() {
    let skills = vec![skill(
        "test-skill",
        "A test skill.",
        "/path/to/skill/SKILL.md",
        "/path/to/skill",
    )];

    let result = format_skills_for_prompt(&skills);

    assert!(result.contains("<available_skills>"));
    assert!(result.contains("</available_skills>"));
    assert!(result.contains("<skill>"));
    assert!(result.contains("<name>test-skill</name>"));
    assert!(result.contains("<description>A test skill.</description>"));
    assert!(result.contains("<location>/path/to/skill/SKILL.md</location>"));
}

#[test]
fn puts_the_intro_text_before_the_xml() {
    let skills = vec![skill(
        "test-skill",
        "A test skill.",
        "/path/to/skill/SKILL.md",
        "/path/to/skill",
    )];

    let result = format_skills_for_prompt(&skills);
    let xml_start = result.find("<available_skills>").expect("xml start");
    let intro = &result[..xml_start];

    assert!(intro.contains("The following skills provide specialized instructions"));
    assert!(intro.contains("Use the read tool to load a skill's file"));
}

#[test]
fn escapes_xml_special_characters() {
    let skills = vec![skill(
        "test-skill",
        "A skill with <special> & \"characters\".",
        "/path/to/skill/SKILL.md",
        "/path/to/skill",
    )];

    let result = format_skills_for_prompt(&skills);

    assert!(result.contains("&lt;special&gt;"));
    assert!(result.contains("&amp;"));
    assert!(result.contains("&quot;characters&quot;"));
}

#[test]
fn formats_multiple_skills() {
    let skills = vec![
        skill(
            "skill-one",
            "First skill.",
            "/path/one/SKILL.md",
            "/path/one",
        ),
        skill(
            "skill-two",
            "Second skill.",
            "/path/two/SKILL.md",
            "/path/two",
        ),
    ];

    let result = format_skills_for_prompt(&skills);

    assert!(result.contains("<name>skill-one</name>"));
    assert!(result.contains("<name>skill-two</name>"));
    assert_eq!(result.matches("<skill>").count(), 2);
}

#[test]
fn excludes_skills_marked_disable_model_invocation() {
    let skills = vec![
        skill(
            "visible-skill",
            "A visible skill.",
            "/path/visible/SKILL.md",
            "/path/visible",
        ),
        create_test_skill(TestSkill {
            name: "hidden-skill",
            description: "A hidden skill.",
            file_path: "/path/hidden/SKILL.md",
            base_dir: "/path/hidden",
            disable_model_invocation: true,
        }),
    ];

    let result = format_skills_for_prompt(&skills);

    assert!(result.contains("<name>visible-skill</name>"));
    assert!(!result.contains("<name>hidden-skill</name>"));
    assert_eq!(result.matches("<skill>").count(), 1);
}

#[test]
fn formats_nothing_when_every_skill_is_hidden_from_the_model() {
    let skills = vec![create_test_skill(TestSkill {
        name: "hidden-skill",
        description: "A hidden skill.",
        file_path: "/path/hidden/SKILL.md",
        base_dir: "/path/hidden",
        disable_model_invocation: true,
    })];

    assert_eq!(format_skills_for_prompt(&skills), "");
}

// ------------------------------------------------------------------ loadSkills

fn empty_agent_dir() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/empty-agent")
        .to_string_lossy()
        .into_owned()
}

fn empty_cwd() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/empty-cwd")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn loads_from_explicit_skill_paths() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd(),
        agent_dir: Some(empty_agent_dir()),
        skill_paths: vec![fixture("valid-skill")],
        include_defaults: true,
    });

    assert_eq!(result.skills.len(), 1);
    assert_eq!(
        result.skills[0].source_info.scope,
        notagent::core::source_info::SourceScope::Temporary
    );
    assert!(result.diagnostics.is_empty());
}

#[test]
fn warns_when_a_skill_path_does_not_exist() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd(),
        agent_dir: Some(empty_agent_dir()),
        skill_paths: vec!["/non/existent/path".to_string()],
        include_defaults: true,
    });

    assert!(result.skills.is_empty());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("does not exist"))
    );
}

#[test]
fn expands_a_leading_tilde_in_skill_paths() {
    let home = dirs::home_dir().expect("home directory");
    let home_skills_dir = home.join(".notagent/agent/skills");

    let with_tilde = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd(),
        agent_dir: Some(empty_agent_dir()),
        skill_paths: vec!["~/.notagent/agent/skills".to_string()],
        include_defaults: true,
    });
    let without_tilde = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd(),
        agent_dir: Some(empty_agent_dir()),
        skill_paths: vec![home_skills_dir.to_string_lossy().into_owned()],
        include_defaults: true,
    });

    assert_eq!(with_tilde.skills.len(), without_tilde.skills.len());
}

#[test]
fn keeps_the_first_skill_of_a_name_collision() {
    let first = load_skills_from_dir(
        &collision_fixtures_dir().join("first").to_string_lossy(),
        "first",
    );
    let second = load_skills_from_dir(
        &collision_fixtures_dir().join("second").to_string_lossy(),
        "second",
    );

    // The same shape `loadSkills` produces, exercised directly on the two halves.
    let mut skill_map: Vec<Skill> = Vec::new();
    let mut collision_warnings: Vec<String> = Vec::new();

    for skill in first.skills {
        skill_map.push(skill);
    }

    for skill in second.skills {
        match skill_map
            .iter()
            .find(|existing| existing.name == skill.name)
        {
            Some(existing) => collision_warnings.push(format!(
                "name collision: \"{}\" already loaded from {}",
                skill.name, existing.file_path
            )),
            None => skill_map.push(skill),
        }
    }

    assert_eq!(skill_map.len(), 1);
    assert_eq!(
        skill_map
            .iter()
            .find(|skill| skill.name == "calendar")
            .map(|skill| skill.source_info.source.as_str()),
        Some("first")
    );
    assert_eq!(collision_warnings.len(), 1);
    assert!(collision_warnings[0].contains("name collision"));
}

/// simulates: the loser is reported as a diagnostic and the winner survives.
#[test]
fn reports_a_collision_diagnostic_and_keeps_the_first_skill() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd(),
        agent_dir: Some(empty_agent_dir()),
        skill_paths: vec![
            collision_fixtures_dir()
                .join("first")
                .to_string_lossy()
                .into_owned(),
            collision_fixtures_dir()
                .join("second")
                .to_string_lossy()
                .into_owned(),
        ],
        include_defaults: true,
    });

    assert_eq!(result.skills.len(), 1);
    assert!(result.skills[0].file_path.contains("first"));
    let collisions: Vec<_> = result
        .diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.collision.as_ref())
        .collect();
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].name, "calendar");
    assert!(collisions[0].winner_path.contains("first"));
    assert!(collisions[0].loser_path.contains("second"));
}

// ------------------------------------------------------------ built-in skills

fn temp_agent_dir() -> PathBuf {
    let directory = tempfile::Builder::new()
        .prefix("builtin-skills-")
        .tempdir()
        .expect("temp dir");
    let path = directory.path().to_path_buf();
    let _ = directory.keep();
    path
}

#[test]
fn materialises_missing_builtin_skills_and_loads_them() {
    let agent_dir = temp_agent_dir();
    let agent_dir_text = agent_dir.to_string_lossy().into_owned();

    let diagnostics = materialize_builtin_skills(&agent_dir_text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd(),
        agent_dir: Some(agent_dir_text),
        skill_paths: Vec::new(),
        include_defaults: true,
    });
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    for (name, _) in BUILTIN_SKILLS {
        let skill = result
            .skills
            .iter()
            .find(|skill| skill.name == *name)
            .unwrap_or_else(|| panic!("built-in skill {name} not loaded"));
        assert!(
            !skill.description.trim().is_empty(),
            "built-in skill {name} has no description"
        );
    }

    let _ = std::fs::remove_dir_all(&agent_dir);
}

#[test]
fn never_touches_an_existing_skill_file() {
    let agent_dir = temp_agent_dir();
    let agent_dir_text = agent_dir.to_string_lossy().into_owned();
    let edited_dir = agent_dir.join("skills").join("create-plan");
    std::fs::create_dir_all(&edited_dir).expect("creates");
    let edited_file = edited_dir.join("SKILL.md");
    let edited_content = "---\nname: create-plan\ndescription: the user's own\n---\nEdited.";
    std::fs::write(&edited_file, edited_content).expect("writes");

    let diagnostics = materialize_builtin_skills(&agent_dir_text);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let kept = std::fs::read_to_string(&edited_file).expect("reads");
    assert_eq!(
        kept, edited_content,
        "materialisation must not overwrite an existing skill file"
    );
    // The other shipped skills still arrive beside the edited one.
    for (name, _) in BUILTIN_SKILLS {
        assert!(
            agent_dir
                .join("skills")
                .join(name)
                .join("SKILL.md")
                .exists(),
            "built-in skill {name} was not materialised"
        );
    }

    let _ = std::fs::remove_dir_all(&agent_dir);
}

#[test]
fn builtin_skill_bodies_carry_no_code_fences_and_no_emoji() {
    for (name, content) in BUILTIN_SKILLS {
        assert!(
            !content.contains("```"),
            "built-in skill {name} contains a code fence"
        );
        assert!(
            content.chars().all(|character| (character as u32) < 0x2190),
            "built-in skill {name} contains a symbol or emoji character"
        );
    }
}
