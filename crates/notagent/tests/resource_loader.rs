//! Ported from `packages/coding-agent/test/resource-loader.test.ts`.
//!
//! Not ported: everything about extensions — discovery, inline factories,
//! pre-trust reuse, and conflict detection (`plans/facts/extension-boundary.md`).
//!
//! Deviation (class 1): the auto-discovery of the two default resource
//! directories lives in the package manager, which is workstream B's
//! (interface request C-11). Where the TypeScript relies on it, the cases below
//! supply a [`PackageResources`] that enumerates the same directories with the
//! same `source: "auto"` metadata the package manager produces, so the loader's
//! own half — merging, precedence, source info, diagnostics — is still pinned.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use notagent_agent::types::BoxFuture;

use notagent::core::resource_loader::{
    DefaultResourceLoader, DefaultResourceLoaderOptions, PackageResources, ResolvedResource,
    ResolvedResources, ResourceExtensionPaths, ResourceLoader, ResourceLoaderReloadOptions,
    load_project_context_files,
};
use notagent::core::settings_manager::{SettingsManager, SettingsManagerCreateOptions};
use notagent::core::skills::Skill;
use notagent::core::source_info::{
    PathMetadata, SourceOrigin, SourceScope, SyntheticSourceInfoOptions,
    create_synthetic_source_info,
};

struct Fixture {
    _temp: tempfile::TempDir,
    temp_dir: PathBuf,
    agent_dir: PathBuf,
    cwd: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temp dir");
    let temp_dir = temp.path().to_path_buf();
    let agent_dir = temp_dir.join("agent");
    let cwd = temp_dir.join("project");
    std::fs::create_dir_all(&agent_dir).expect("create agent dir");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    Fixture {
        _temp: temp,
        temp_dir,
        agent_dir,
        cwd,
    }
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, content).expect("write file");
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Stands in for `DefaultPackageManager.resolve()`: the two default resource
/// directories, enumerated with the metadata the package manager attaches to
/// auto-discovered entries. Project entries come first, as they do there, so
/// they win name collisions.
struct AutoDiscovery {
    agent_dir: PathBuf,
    cwd: PathBuf,
    project_trusted: bool,
}

impl AutoDiscovery {
    fn new(agent_dir: &Path, cwd: &Path) -> Self {
        Self {
            agent_dir: agent_dir.to_path_buf(),
            cwd: cwd.to_path_buf(),
            project_trusted: true,
        }
    }

    fn untrusted(mut self) -> Self {
        self.project_trusted = false;
        self
    }

    fn entries(dir: &Path, metadata: &PathMetadata, extension: &str) -> Vec<ResolvedResource> {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut entries: Vec<(String, PathBuf)> = read_dir
            .flatten()
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                )
            })
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
            .into_iter()
            .filter(|(name, path)| {
                path.is_dir() || (name.ends_with(extension) && !name.starts_with('.'))
            })
            .map(|(_, path)| ResolvedResource {
                path: text(&path),
                metadata: metadata.clone(),
                enabled: true,
            })
            .collect()
    }
}

impl PackageResources for AutoDiscovery {
    fn resolve(&self) -> BoxFuture<'_, ResolvedResources> {
        Box::pin(async move {
            let user = PathMetadata {
                source: "auto".to_string(),
                scope: SourceScope::User,
                origin: SourceOrigin::TopLevel,
                base_dir: Some(text(&self.agent_dir)),
            };
            let project_base = self.cwd.join(".notagent");
            let project = PathMetadata {
                source: "auto".to_string(),
                scope: SourceScope::Project,
                origin: SourceOrigin::TopLevel,
                base_dir: Some(text(&project_base)),
            };

            let mut resolved = ResolvedResources::default();
            for (kind, extension) in [("skills", ".md"), ("prompts", ".md"), ("themes", ".json")] {
                let mut list = Vec::new();
                if self.project_trusted {
                    list.extend(Self::entries(
                        &project_base.join(kind),
                        &project,
                        extension,
                    ));
                }
                list.extend(Self::entries(
                    &self.agent_dir.join(kind),
                    &user,
                    extension,
                ));
                match kind {
                    "skills" => resolved.skills = list,
                    "prompts" => resolved.prompts = list,
                    _ => resolved.themes = list,
                }
            }
            resolved
        })
    }
}

fn options(fixture: &Fixture) -> DefaultResourceLoaderOptions {
    DefaultResourceLoaderOptions {
        cwd: text(&fixture.cwd),
        agent_dir: text(&fixture.agent_dir),
        ..DefaultResourceLoaderOptions::default()
    }
}

fn with_auto_discovery(fixture: &Fixture) -> DefaultResourceLoaderOptions {
    DefaultResourceLoaderOptions {
        packages: Some(Arc::new(AutoDiscovery::new(
            &fixture.agent_dir,
            &fixture.cwd,
        ))),
        ..options(fixture)
    }
}

async fn reload(loader: &DefaultResourceLoader) {
    loader.reload(ResourceLoaderReloadOptions::default()).await;
}

// ------------------------------------------------------------------- reload

#[tokio::test]
async fn starts_out_empty_before_the_first_reload() {
    let fixture = fixture();
    let loader = DefaultResourceLoader::new(options(&fixture));

    assert!(loader.get_skills().0.is_empty());
    assert!(loader.get_prompts().0.is_empty());
    assert!(loader.get_themes().0.is_empty());
}

#[tokio::test]
async fn discovers_skills_from_the_agent_directory() {
    let fixture = fixture();
    write(
        &fixture.agent_dir.join("skills/test-skill.md"),
        "---\nname: test-skill\ndescription: A test skill\n---\nSkill content here.",
    );

    let loader = DefaultResourceLoader::new(with_auto_discovery(&fixture));
    reload(&loader).await;

    assert!(
        loader
            .get_skills()
            .0
            .iter()
            .any(|skill| skill.name == "test-skill")
    );
}

#[tokio::test]
async fn ignores_extra_markdown_files_next_to_a_skill_file() {
    let fixture = fixture();
    let skill_dir = fixture.agent_dir.join("skills/notagent-skills/browser-tools");
    write(
        &skill_dir.join("SKILL.md"),
        "---\nname: browser-tools\ndescription: Browser tools\n---\nSkill content here.",
    );
    write(&skill_dir.join("EFFICIENCY.md"), "No frontmatter here");

    let loader = DefaultResourceLoader::new(with_auto_discovery(&fixture));
    reload(&loader).await;

    let (skills, diagnostics) = loader.get_skills();
    assert!(skills.iter().any(|skill| skill.name == "browser-tools"));
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic
            .path
            .as_deref()
            .is_some_and(|path| path.ends_with("EFFICIENCY.md")))
    );
}

#[tokio::test]
async fn discovers_prompts_from_the_agent_directory() {
    let fixture = fixture();
    write(
        &fixture.agent_dir.join("prompts/test-prompt.md"),
        "---\ndescription: A test prompt\n---\nPrompt content.",
    );

    let loader = DefaultResourceLoader::new(with_auto_discovery(&fixture));
    reload(&loader).await;

    assert!(
        loader
            .get_prompts()
            .0
            .iter()
            .any(|prompt| prompt.name == "test-prompt")
    );
}

#[tokio::test]
async fn prefers_project_resources_over_user_ones_on_a_name_collision() {
    let fixture = fixture();
    let user_prompt = fixture.agent_dir.join("prompts/commit.md");
    let project_prompt = fixture.cwd.join(".notagent/prompts/commit.md");
    write(&user_prompt, "User prompt");
    write(&project_prompt, "Project prompt");

    let loader = DefaultResourceLoader::new(with_auto_discovery(&fixture));
    reload(&loader).await;

    let (prompts, diagnostics) = loader.get_prompts();
    let commit = prompts
        .iter()
        .find(|prompt| prompt.name == "commit")
        .expect("commit prompt");
    assert_eq!(commit.file_path, text(&project_prompt));
    assert_eq!(commit.content, "Project prompt");
    let collision = diagnostics
        .iter()
        .find_map(|diagnostic| diagnostic.collision.as_ref())
        .expect("collision diagnostic");
    assert_eq!(collision.winner_path, text(&project_prompt));
    assert_eq!(collision.loser_path, text(&user_prompt));
}

// ------------------------------------------------------------ context files

#[tokio::test]
async fn discovers_agents_context_files() {
    let fixture = fixture();
    write(
        &fixture.cwd.join("AGENTS.md"),
        "# Project Guidelines\n\nBe helpful.",
    );

    let loader = DefaultResourceLoader::new(options(&fixture));
    reload(&loader).await;

    assert!(
        loader
            .get_agents_files()
            .iter()
            .any(|file| file.path.contains("AGENTS.md"))
    );
}

#[tokio::test]
async fn prefers_an_override_file_per_directory_while_keeping_ancestor_layering() {
    let fixture = fixture();
    let nested = fixture.cwd.join("service");
    std::fs::create_dir_all(&nested).expect("create nested");
    write(&fixture.agent_dir.join("AGENTS.md"), "global instructions");
    write(
        &fixture.agent_dir.join("AGENTS.override.md"),
        "global override",
    );
    write(&fixture.cwd.join("AGENTS.md"), "project instructions");
    write(&nested.join("AGENTS.md"), "service instructions");
    write(&nested.join("AGENTS.override.md"), "service override");

    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        cwd: text(&nested),
        ..options(&fixture)
    });
    reload(&loader).await;

    let files = loader.get_agents_files();
    assert_eq!(
        files
            .iter()
            .map(|file| (file.path.clone(), file.content.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                text(&fixture.agent_dir.join("AGENTS.override.md")),
                "global override".to_string()
            ),
            (
                text(&fixture.cwd.join("AGENTS.md")),
                "project instructions".to_string()
            ),
            (
                text(&nested.join("AGENTS.override.md")),
                "service override".to_string()
            ),
        ]
    );
}

#[tokio::test]
async fn ignores_context_file_candidates_that_are_directories() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.cwd.join("AGENTS.override.md")).expect("create dir");
    std::fs::create_dir_all(fixture.cwd.join("AGENTS.md")).expect("create dir");
    write(&fixture.cwd.join("CLAUDE.md"), "Fallback instructions");

    let loader = DefaultResourceLoader::new(options(&fixture));
    reload(&loader).await;

    assert!(loader.get_agents_files().iter().any(|file| {
        file.path == text(&fixture.cwd.join("CLAUDE.md")) && file.content == "Fallback instructions"
    }));
}

#[tokio::test]
async fn skips_context_files_entirely_when_asked_to() {
    let fixture = fixture();
    write(
        &fixture.cwd.join("AGENTS.override.md"),
        "# Override Guidelines",
    );
    write(&fixture.cwd.join("AGENTS.md"), "# Project Guidelines");
    write(&fixture.cwd.join("CLAUDE.md"), "# Claude Guidelines");

    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        no_context_files: true,
        ..options(&fixture)
    });
    reload(&loader).await;

    assert!(loader.get_agents_files().is_empty());
}

// ----------------------------------------------------------- system prompts

#[tokio::test]
async fn discovers_a_project_system_prompt() {
    let fixture = fixture();
    let path = fixture.cwd.join(".notagent/SYSTEM.md");
    write(&path, "You are a helpful assistant.");

    let loader = DefaultResourceLoader::new(options(&fixture));
    reload(&loader).await;

    assert_eq!(
        loader.get_system_prompt().as_deref(),
        Some("You are a helpful assistant.")
    );
    assert_eq!(loader.get_system_prompt_source(), Some(text(&path)));
}

#[tokio::test]
async fn discovers_a_global_system_prompt() {
    let fixture = fixture();
    let path = fixture.agent_dir.join("SYSTEM.md");
    write(&path, "Global system prompt.");

    let loader = DefaultResourceLoader::new(options(&fixture));
    reload(&loader).await;

    assert_eq!(
        loader.get_system_prompt().as_deref(),
        Some("Global system prompt.")
    );
    assert_eq!(loader.get_system_prompt_source(), Some(text(&path)));
}

#[tokio::test]
async fn does_not_report_literal_system_prompt_text_as_a_source() {
    let fixture = fixture();
    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        system_prompt: Some("Literal system prompt.".to_string()),
        ..options(&fixture)
    });
    reload(&loader).await;

    assert_eq!(
        loader.get_system_prompt().as_deref(),
        Some("Literal system prompt.")
    );
    assert_eq!(loader.get_system_prompt_source(), None);
}

#[tokio::test]
async fn reports_a_file_backed_system_prompt_option_as_a_source() {
    let fixture = fixture();
    let path = fixture.temp_dir.join("custom-system.md");
    write(&path, "Custom system prompt.");

    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        system_prompt: Some(text(&path)),
        ..options(&fixture)
    });
    reload(&loader).await;

    assert_eq!(
        loader.get_system_prompt().as_deref(),
        Some("Custom system prompt.")
    );
    assert_eq!(loader.get_system_prompt_source(), Some(text(&path)));
}

#[tokio::test]
async fn discovers_an_append_system_prompt() {
    let fixture = fixture();
    let path = fixture.cwd.join(".notagent/APPEND_SYSTEM.md");
    write(&path, "Project append prompt.");

    let loader = DefaultResourceLoader::new(options(&fixture));
    reload(&loader).await;

    assert_eq!(
        loader.get_append_system_prompt(),
        vec!["Project append prompt.".to_string()]
    );
    assert_eq!(
        loader.get_append_system_prompt_sources(),
        vec![text(&path)]
    );
}

#[tokio::test]
async fn does_not_report_literal_append_prompt_text_as_a_source() {
    let fixture = fixture();
    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        append_system_prompt: Some(vec!["Literal append prompt.".to_string()]),
        ..options(&fixture)
    });
    reload(&loader).await;

    assert_eq!(
        loader.get_append_system_prompt(),
        vec!["Literal append prompt.".to_string()]
    );
    assert!(loader.get_append_system_prompt_sources().is_empty());
}

#[tokio::test]
async fn reports_only_the_file_backed_append_prompt_options_as_sources() {
    let fixture = fixture();
    let path = fixture.temp_dir.join("custom-append.md");
    write(&path, "Custom append prompt.");

    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        append_system_prompt: Some(vec![text(&path), "Literal append prompt.".to_string()]),
        ..options(&fixture)
    });
    reload(&loader).await;

    assert_eq!(
        loader.get_append_system_prompt(),
        vec![
            "Custom append prompt.".to_string(),
            "Literal append prompt.".to_string()
        ]
    );
    assert_eq!(
        loader.get_append_system_prompt_sources(),
        vec![text(&path)]
    );
}

#[tokio::test]
async fn skips_project_resources_that_require_trust_when_the_project_is_not_trusted() {
    let fixture = fixture();
    let project_config = fixture.cwd.join(".notagent");
    write(&project_config.join("SYSTEM.md"), "Project system prompt.");
    write(&fixture.agent_dir.join("SYSTEM.md"), "Global system prompt.");
    write(&fixture.agent_dir.join("AGENTS.md"), "Global instructions");
    write(&fixture.cwd.join("AGENTS.md"), "Project instructions");
    write(
        &project_config.join("skills/project-skill/SKILL.md"),
        "---\nname: project-skill\ndescription: Project skill\n---\nProject skill content",
    );
    write(&project_config.join("prompts/project.md"), "Project prompt");

    let settings_manager = Arc::new(SettingsManager::create(
        &fixture.cwd,
        Some(&fixture.agent_dir),
        SettingsManagerCreateOptions {
            project_trusted: Some(false),
        },
    ));
    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        settings_manager: Some(settings_manager),
        packages: Some(Arc::new(
            AutoDiscovery::new(&fixture.agent_dir, &fixture.cwd).untrusted(),
        )),
        ..options(&fixture)
    });
    reload(&loader).await;

    assert_eq!(
        loader.get_system_prompt().as_deref(),
        Some("Global system prompt.")
    );
    // Context files are not gated on trust — both still load.
    let agents_files = loader.get_agents_files();
    assert!(
        agents_files
            .iter()
            .any(|file| file.path == text(&fixture.agent_dir.join("AGENTS.md")))
    );
    assert!(
        agents_files
            .iter()
            .any(|file| file.path == text(&fixture.cwd.join("AGENTS.md")))
    );
    assert!(
        !loader
            .get_skills()
            .0
            .iter()
            .any(|skill| skill.name == "project-skill")
    );
    assert!(
        !loader
            .get_prompts()
            .0
            .iter()
            .any(|prompt| prompt.name == "project")
    );
}

// ---------------------------------------------------------- extendResources

#[tokio::test]
async fn loads_skills_and_prompts_handed_over_after_the_reload() {
    let fixture = fixture();
    let extra_skill_dir = fixture.temp_dir.join("extra-skills/extra-skill");
    write(
        &extra_skill_dir.join("SKILL.md"),
        "---\nname: extra-skill\ndescription: Extra skill\n---\nExtra content",
    );
    let extra_prompt_dir = fixture.temp_dir.join("extra-prompts");
    write(
        &extra_prompt_dir.join("extra.md"),
        "---\ndescription: Extra prompt\n---\nExtra prompt content",
    );

    let loader = DefaultResourceLoader::new(options(&fixture));
    reload(&loader).await;

    let metadata = PathMetadata {
        source: "package:extra".to_string(),
        scope: SourceScope::Temporary,
        origin: SourceOrigin::TopLevel,
        base_dir: Some(text(&fixture.temp_dir)),
    };
    loader.extend_resources(ResourceExtensionPaths {
        skill_paths: vec![(text(&extra_skill_dir), metadata.clone())],
        prompt_paths: vec![(text(&extra_prompt_dir), metadata.clone())],
        theme_paths: Vec::new(),
    });

    let skill = loader
        .get_skills()
        .0
        .into_iter()
        .find(|skill| skill.name == "extra-skill")
        .expect("extra skill");
    assert_eq!(skill.source_info.source, "package:extra");
    assert_eq!(skill.source_info.scope, SourceScope::Temporary);

    let prompt = loader
        .get_prompts()
        .0
        .into_iter()
        .find(|prompt| prompt.name == "extra")
        .expect("extra prompt");
    assert_eq!(prompt.source_info.source, "package:extra");
}

// ------------------------------------------------------------------- flags

#[tokio::test]
async fn skips_skill_discovery_when_asked_to() {
    let fixture = fixture();
    write(
        &fixture.agent_dir.join("skills/test-skill.md"),
        "---\nname: test-skill\ndescription: A test skill\n---\nContent",
    );

    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        no_skills: true,
        ..with_auto_discovery(&fixture)
    });
    reload(&loader).await;

    assert!(loader.get_skills().0.is_empty());
}

#[tokio::test]
async fn still_loads_explicit_skill_paths_when_discovery_is_off() {
    let fixture = fixture();
    let custom = fixture.temp_dir.join("custom-skills");
    write(
        &custom.join("custom.md"),
        "---\nname: custom\ndescription: Custom skill\n---\nContent",
    );

    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        no_skills: true,
        additional_skill_paths: vec![text(&custom)],
        ..with_auto_discovery(&fixture)
    });
    reload(&loader).await;

    assert!(
        loader
            .get_skills()
            .0
            .iter()
            .any(|skill| skill.name == "custom")
    );
}

#[tokio::test]
async fn reports_an_explicit_skill_path_that_does_not_exist() {
    let fixture = fixture();
    let missing = fixture.temp_dir.join("missing-skills");

    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        additional_skill_paths: vec![text(&missing)],
        ..options(&fixture)
    });
    reload(&loader).await;

    assert!(
        loader
            .get_skills()
            .1
            .iter()
            .any(|diagnostic| diagnostic.path.as_deref() == Some(text(&missing).as_str()))
    );
}

// ---------------------------------------------------------------- overrides

#[tokio::test]
async fn applies_a_skills_override() {
    let fixture = fixture();
    let injected = Skill {
        name: "injected".to_string(),
        description: "Injected skill".to_string(),
        file_path: "/fake/path".to_string(),
        base_dir: "/fake".to_string(),
        source_info: create_synthetic_source_info(
            "/fake/path",
            SyntheticSourceInfoOptions::new("custom"),
        ),
        disable_model_invocation: false,
    };
    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        skills_override: Some(Arc::new(move |_base| {
            notagent::core::skills::LoadSkillsResult {
                skills: vec![injected.clone()],
                diagnostics: Vec::new(),
            }
        })),
        ..options(&fixture)
    });
    reload(&loader).await;

    let skills = loader.get_skills().0;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "injected");
}

#[tokio::test]
async fn applies_a_system_prompt_override() {
    let fixture = fixture();
    let loader = DefaultResourceLoader::new(DefaultResourceLoaderOptions {
        system_prompt_override: Some(Arc::new(|_base| Some("Custom system prompt".to_string()))),
        ..options(&fixture)
    });
    reload(&loader).await;

    assert_eq!(
        loader.get_system_prompt().as_deref(),
        Some("Custom system prompt")
    );
}

// ------------------------------------ loadProjectContextFiles: worktree dedup

/// Builds a linked-worktree skeleton without needing a git binary: the main
/// repository's `.git/worktrees/<name>/` holds `HEAD` plus a `commondir`
/// pointing back at the main `.git`, and the worktree carries a `.git` *file*
/// whose `gitdir:` resolves to it.
fn link_worktree(main_dir: &Path, worktree_dir: &Path, name: &str) {
    let git_dir = main_dir.join(".git/worktrees").join(name);
    std::fs::create_dir_all(&git_dir).expect("create worktree gitdir");
    write(&main_dir.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&git_dir.join("HEAD"), "ref: refs/heads/feat\n");
    write(&git_dir.join("commondir"), "../..");
    write(
        &worktree_dir.join(".git"),
        &format!("gitdir: {}\n", text(&git_dir)),
    );
}

struct NestedWorktree {
    outer: PathBuf,
    main: PathBuf,
    worktree: PathBuf,
    worktree_src: PathBuf,
}

fn setup_nested_worktree(temp_dir: &Path) -> NestedWorktree {
    let outer = temp_dir.join("outer");
    let main = outer.join("main");
    let worktree = main.join("worktrees/feat");
    let worktree_src = worktree.join("src");
    std::fs::create_dir_all(&worktree_src).expect("create worktree src");
    link_worktree(&main, &worktree, "feat");
    NestedWorktree {
        outer,
        main,
        worktree,
        worktree_src,
    }
}

fn contents(cwd: &Path, agent_dir: &Path) -> Vec<String> {
    load_project_context_files(&text(cwd), &text(agent_dir))
        .into_iter()
        .map(|file| file.content)
        .collect()
}

#[test]
fn skips_the_main_repos_duplicate_when_the_worktree_root_has_its_own_context() {
    let fixture = fixture();
    let tree = setup_nested_worktree(&fixture.temp_dir);
    write(&tree.main.join("AGENTS.md"), "main repo instructions");
    write(&tree.worktree.join("AGENTS.md"), "worktree instructions");

    assert_eq!(
        contents(&tree.worktree_src, &fixture.agent_dir),
        vec!["worktree instructions".to_string()]
    );
}

#[test]
fn still_inherits_the_main_repos_context_when_the_worktree_root_has_none() {
    let fixture = fixture();
    let tree = setup_nested_worktree(&fixture.temp_dir);
    write(&tree.main.join("AGENTS.md"), "main repo instructions");

    assert_eq!(
        contents(&tree.worktree_src, &fixture.agent_dir),
        vec!["main repo instructions".to_string()]
    );
}

#[test]
fn only_skips_the_same_filename_not_a_differently_named_context_file() {
    let fixture = fixture();
    let tree = setup_nested_worktree(&fixture.temp_dir);
    write(&tree.main.join("CLAUDE.md"), "main repo instructions");
    write(&tree.worktree.join("AGENTS.md"), "worktree instructions");

    assert_eq!(
        contents(&tree.worktree_src, &fixture.agent_dir),
        vec![
            "main repo instructions".to_string(),
            "worktree instructions".to_string()
        ]
    );
}

#[test]
fn does_not_skip_the_containers_context_in_a_bare_layout() {
    let fixture = fixture();
    let proj = fixture.temp_dir.join("proj");
    let bare = proj.join(".bare");
    let worktree = proj.join("main");
    let worktree_git_dir = bare.join("worktrees/main");
    std::fs::create_dir_all(&worktree_git_dir).expect("create gitdir");
    std::fs::create_dir_all(&worktree).expect("create worktree");
    write(&bare.join("HEAD"), "ref: refs/heads/main\n");
    write(&worktree_git_dir.join("HEAD"), "ref: refs/heads/main\n");
    write(&worktree_git_dir.join("commondir"), "../..");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", text(&worktree_git_dir)),
    );
    write(&proj.join("AGENTS.md"), "container instructions");
    write(&worktree.join("AGENTS.md"), "worktree instructions");

    assert_eq!(
        contents(&worktree, &fixture.agent_dir),
        vec![
            "container instructions".to_string(),
            "worktree instructions".to_string()
        ]
    );
}

#[test]
fn keeps_loading_ancestors_above_the_main_repo() {
    let fixture = fixture();
    let tree = setup_nested_worktree(&fixture.temp_dir);
    write(&tree.outer.join("AGENTS.md"), "outer instructions");
    write(&tree.main.join("AGENTS.md"), "main repo instructions");
    write(&tree.worktree.join("AGENTS.md"), "worktree instructions");

    // Only the main repository root's duplicate is dropped; the unrelated
    // directory above it stays.
    assert_eq!(
        contents(&tree.worktree_src, &fixture.agent_dir),
        vec![
            "outer instructions".to_string(),
            "worktree instructions".to_string()
        ]
    );
}

#[test]
fn does_not_skip_anything_for_a_sibling_worktree() {
    let fixture = fixture();
    let outer = fixture.temp_dir.join("outer");
    let main = outer.join("main");
    let sibling = outer.join("sib-feat");
    let sibling_src = sibling.join("src");
    std::fs::create_dir_all(&sibling_src).expect("create sibling src");
    std::fs::create_dir_all(&main).expect("create main");
    write(&outer.join("AGENTS.md"), "outer instructions");
    write(&sibling.join("AGENTS.md"), "sibling worktree instructions");
    link_worktree(&main, &sibling, "sib");

    assert_eq!(
        contents(&sibling_src, &fixture.agent_dir),
        vec![
            "outer instructions".to_string(),
            "sibling worktree instructions".to_string()
        ]
    );
}

#[test]
fn does_not_skip_the_superprojects_context_from_inside_a_submodule() {
    let fixture = fixture();
    let superproject = fixture.temp_dir.join("super");
    let submodule = superproject.join("vendor/lib");
    let submodule_src = submodule.join("src");
    std::fs::create_dir_all(&submodule_src).expect("create submodule src");
    write(&superproject.join("AGENTS.md"), "superproject instructions");
    write(&submodule.join("AGENTS.md"), "submodule instructions");
    let submodule_git_dir = superproject.join(".git/modules/vendor/lib");
    write(&submodule_git_dir.join("HEAD"), "ref: refs/heads/main\n");
    write(
        &submodule.join(".git"),
        &format!("gitdir: {}\n", text(&submodule_git_dir)),
    );

    assert_eq!(
        contents(&submodule_src, &fixture.agent_dir),
        vec![
            "superproject instructions".to_string(),
            "submodule instructions".to_string()
        ]
    );
}

#[test]
fn keeps_climbing_past_an_ordinary_repo_root() {
    let fixture = fixture();
    let outer = fixture.temp_dir.join("outer");
    let repo = outer.join("repo");
    let leaf = repo.join("src");
    std::fs::create_dir_all(&leaf).expect("create leaf");
    write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&outer.join("AGENTS.md"), "outer instructions");
    write(&repo.join("AGENTS.md"), "repo instructions");
    write(&leaf.join("AGENTS.md"), "leaf instructions");

    assert_eq!(
        contents(&leaf, &fixture.agent_dir),
        vec![
            "outer instructions".to_string(),
            "repo instructions".to_string(),
            "leaf instructions".to_string()
        ]
    );
}

#[test]
fn climbs_normally_when_the_gitdir_target_does_not_exist() {
    let fixture = fixture();
    let repo = fixture.temp_dir.join("corrupt");
    let src = repo.join("src");
    std::fs::create_dir_all(&src).expect("create src");
    write(&repo.join(".git"), "gitdir: /nonexistent/path/worktrees/feat\n");
    write(&repo.join("AGENTS.md"), "repo instructions");
    write(&src.join("AGENTS.md"), "src instructions");

    assert_eq!(
        contents(&src, &fixture.agent_dir),
        vec![
            "repo instructions".to_string(),
            "src instructions".to_string()
        ]
    );
}
