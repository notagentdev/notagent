use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use notagent::config::USER_CONFIG_DIR_NAME;
use notagent::core::keybindings::KeybindingsManager;
use notagent::core::resource_loader::{ResolvedResource, ResolvedResources};
use notagent::core::settings_manager::{
    InMemorySettingsStorage, PackageSource, PackageSourceFilter, SettingsManager,
    SettingsManagerCreateOptions,
};
use notagent::core::source_info::{PathMetadata, SourceOrigin, SourceScope};
use notagent::modes::interactive::components::config_selector::{
    ConfigSelectorComponent, ConfigWriteScope, ScopedResolvedPaths,
};
use notagent::modes::interactive::theme::theme::init_theme;
use notagent::utils::ansi::strip_ansi;
use notagent_tui::keybindings::set_keybindings;
use notagent_tui::tui::Component;

/// The theme and the keybindings registry are process globals.
fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    init_theme(Some("dark"), false);
    set_keybindings(KeybindingsManager::default().to_tui());
    guard
}

const AGENT_DIR: &str = "/home/u/.notagent/agent";
const CWD: &str = "/proj";
const DOWN: &str = "\x1b[B";
const PAGE_DOWN: &str = "\x1b[6~";
const TAB: &str = "\t";

fn res(
    path: &str,
    enabled: bool,
    source: &str,
    scope: SourceScope,
    origin: SourceOrigin,
    base_dir: Option<&str>,
) -> ResolvedResource {
    ResolvedResource {
        path: path.to_string(),
        enabled,
        metadata: PathMetadata {
            source: source.to_string(),
            scope,
            origin,
            base_dir: base_dir.map(str::to_string),
        },
    }
}

fn manager(trusted: bool) -> Arc<SettingsManager> {
    Arc::new(SettingsManager::from_storage(
        Arc::new(InMemorySettingsStorage::default()),
        SettingsManagerCreateOptions {
            project_trusted: Some(trusted),
        },
    ))
}

struct Harness {
    selector: ConfigSelectorComponent,
    settings: Arc<SettingsManager>,
    renders: Rc<RefCell<usize>>,
}

impl Harness {
    fn new(
        resolved: ScopedResolvedPaths,
        settings: Arc<SettingsManager>,
        write_scope: ConfigWriteScope,
        project_mode_available: bool,
    ) -> Self {
        let renders = Rc::new(RefCell::new(0));
        let counter = Rc::clone(&renders);
        let selector = ConfigSelectorComponent::new(
            &resolved,
            Arc::clone(&settings),
            CWD,
            AGENT_DIR,
            Box::new(|| {}),
            Box::new(|| {}),
            Rc::new(move || *counter.borrow_mut() += 1),
            Some(24),
            write_scope,
            project_mode_available,
        );
        Self {
            selector,
            settings,
            renders,
        }
    }

    fn lines(&mut self) -> Vec<String> {
        self.selector
            .render(80)
            .into_iter()
            .map(|line| strip_ansi(&line))
            .collect()
    }

    fn key(&mut self, data: &str) {
        self.selector.handle_input(data);
    }
}

fn empty() -> ResolvedResources {
    ResolvedResources::default()
}

#[test]
fn groups_resources_by_source_and_sorts_packages_first() {
    let _guard = test_lock();
    let global = ResolvedResources {
        skills: vec![
            res(
                "/home/u/.notagent/agent/skills/zed/SKILL.md",
                true,
                "auto",
                SourceScope::User,
                SourceOrigin::TopLevel,
                None,
            ),
            res(
                "/home/u/.notagent/agent/skills/alpha.md",
                false,
                "auto",
                SourceScope::User,
                SourceOrigin::TopLevel,
                None,
            ),
            res(
                "/pkg/wolf/skills/w.md",
                true,
                "wolf-pack",
                SourceScope::User,
                SourceOrigin::Package,
                Some("/pkg/wolf"),
            ),
        ],
        prompts: vec![res(
            "/home/u/.notagent/agent/prompts/p.md",
            true,
            "auto",
            SourceScope::User,
            SourceOrigin::TopLevel,
            None,
        )],
        themes: vec![res(
            "/pkg/wolf/themes/t.json",
            false,
            "wolf-pack",
            SourceScope::User,
            SourceOrigin::Package,
            Some("/pkg/wolf"),
        )],
    };
    let project = ResolvedResources {
        skills: vec![res(
            "/proj/.notagent/skills/local.md",
            true,
            "auto",
            SourceScope::Project,
            SourceOrigin::TopLevel,
            None,
        )],
        ..ResolvedResources::default()
    };

    let mut harness = Harness::new(
        ScopedResolvedPaths { global, project },
        manager(true),
        ConfigWriteScope::Global,
        true,
    );

    let lines = harness.lines();
    // Derived rather than spelled out: user-level state lives under its own
    // directory so this build does not share one with the original, and a
    // literal here went stale the moment that split happened.
    assert_eq!(
        &lines[3..5],
        &[
            "Global Resources                      tab switch mode · space toggle · esc close"
                .to_string(),
            format!("~/{USER_CONFIG_DIR_NAME}/agent/settings.json"),
        ]
    );
    // A skill named SKILL.md shows its folder; the package group sorts first.
    assert_eq!(
        &lines[8..],
        &[
            "  wolf-pack (user)",
            "    Skills",
            ">     [x] w.md",
            "    Themes",
            "      [ ] t.json",
            "  User (/home/u/.notagent/agent/)",
            "    Skills",
            "      [ ] alpha.md",
            "      [x] zed",
            "    Prompts",
            "      [x] p.md",
            "",
            "────────────────────────────────────────────────────────────────────────────────",
        ]
    );

    // The cursor only ever lands on items, never on a group or subgroup header.
    harness.key(DOWN);
    let lines = harness.lines();
    assert!(lines.contains(&">     [ ] t.json".to_string()));
    assert!(lines.contains(&"      [x] w.md".to_string()));
}

#[test]
fn writes_plus_and_minus_patterns_for_top_level_and_package_resources() {
    let _guard = test_lock();
    let global = ResolvedResources {
        skills: vec![
            res(
                "/home/u/.notagent/agent/skills/alpha.md",
                true,
                "auto",
                SourceScope::User,
                SourceOrigin::TopLevel,
                None,
            ),
            res(
                "/pkg/wolf/skills/w.md",
                true,
                "wolf-pack",
                SourceScope::User,
                SourceOrigin::Package,
                Some("/pkg/wolf"),
            ),
        ],
        ..ResolvedResources::default()
    };
    let settings = manager(true);
    settings.set_skill_paths(&["skills/alpha.md".to_string()]);
    settings.set_packages(&[PackageSource::Source("wolf-pack".to_string())]);

    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global,
            project: empty(),
        },
        settings,
        ConfigWriteScope::Global,
        true,
    );

    // The selection starts on the package resource of the first group.
    harness.key(" ");
    assert_eq!(
        harness.settings.get_global_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: "wolf-pack".to_string(),
            skills: Some(vec!["-skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );
    assert!(harness.lines().contains(&">     [ ] w.md".to_string()));

    harness.key(" ");
    assert_eq!(
        harness.settings.get_global_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: "wolf-pack".to_string(),
            skills: Some(vec!["+skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );

    // The top-level resource replaces its plain pattern with a signed one.
    harness.key(DOWN);
    harness.key(" ");
    assert_eq!(
        harness.settings.get_global_settings().skills.unwrap(),
        vec!["-skills/alpha.md".to_string()]
    );
    harness.key(" ");
    assert_eq!(
        harness.settings.get_global_settings().skills.unwrap(),
        vec!["+skills/alpha.md".to_string()]
    );
    assert_eq!(*harness.renders.borrow(), 4);
}

/// A package entry that loses its last filter collapses back to the plain
#[test]
fn collapses_a_package_entry_without_filters_but_keeps_extension_filters() {
    let _guard = test_lock();
    let global = ResolvedResources {
        skills: vec![res(
            "/pkg/wolf/skills/w.md",
            false,
            "wolf-pack",
            SourceScope::User,
            SourceOrigin::Package,
            Some("/pkg/wolf"),
        )],
        ..ResolvedResources::default()
    };
    let settings = manager(true);
    settings.set_packages(&[PackageSource::Filtered(PackageSourceFilter {
        source: "wolf-pack".to_string(),
        skills: Some(vec!["-skills/w.md".to_string()]),
        ..PackageSourceFilter::default()
    })]);

    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global: global.clone(),
            project: empty(),
        },
        Arc::clone(&settings),
        ConfigWriteScope::Global,
        true,
    );
    harness.key(" ");
    assert_eq!(
        harness.settings.get_global_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: "wolf-pack".to_string(),
            skills: Some(vec!["+skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );

    // With no pattern left the entry becomes the bare source again.
    let settings = manager(true);
    settings.set_packages(&[PackageSource::Filtered(PackageSourceFilter {
        source: "wolf-pack".to_string(),
        skills: Some(vec!["+skills/w.md".to_string()]),
        ..PackageSourceFilter::default()
    })]);
    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global: global.clone(),
            project: empty(),
        },
        Arc::clone(&settings),
        ConfigWriteScope::Global,
        true,
    );
    harness.key(" ");
    assert_eq!(
        harness.settings.get_global_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: "wolf-pack".to_string(),
            skills: Some(vec!["+skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );

    // An extensions filter survives the collapse check.
    let settings = manager(true);
    settings.set_packages(&[PackageSource::Filtered(PackageSourceFilter {
        source: "wolf-pack".to_string(),
        extensions: Some(vec!["+ext/a.js".to_string()]),
        skills: Some(vec!["+skills/w.md".to_string()]),
        ..PackageSourceFilter::default()
    })]);
    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global,
            project: empty(),
        },
        settings,
        ConfigWriteScope::Global,
        true,
    );
    harness.key(" ");
    assert_eq!(
        harness.settings.get_global_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: "wolf-pack".to_string(),
            extensions: Some(vec!["+ext/a.js".to_string()]),
            skills: Some(vec!["+skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );
}

/// A package the settings do not list is toggled in the display only — the
#[test]
fn a_package_missing_from_the_settings_only_changes_the_row() {
    let _guard = test_lock();
    let global = ResolvedResources {
        skills: vec![res(
            "/pkg/wolf/skills/w.md",
            true,
            "wolf-pack",
            SourceScope::User,
            SourceOrigin::Package,
            Some("/pkg/wolf"),
        )],
        ..ResolvedResources::default()
    };
    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global,
            project: empty(),
        },
        manager(true),
        ConfigWriteScope::Global,
        true,
    );
    harness.key(" ");
    assert!(harness.settings.get_global_settings().packages.is_none());
    assert!(harness.lines().contains(&">     [ ] w.md".to_string()));
}

#[test]
fn cycles_the_project_override_of_an_inherited_resource() {
    let _guard = test_lock();
    let inherited = res(
        "/home/u/.notagent/agent/skills/alpha.md",
        true,
        "auto",
        SourceScope::User,
        SourceOrigin::TopLevel,
        None,
    );
    let global = ResolvedResources {
        skills: vec![inherited.clone()],
        ..ResolvedResources::default()
    };
    let project = ResolvedResources {
        skills: vec![
            inherited,
            res(
                "/proj/.notagent/skills/local.md",
                false,
                "auto",
                SourceScope::Project,
                SourceOrigin::TopLevel,
                None,
            ),
        ],
        ..ResolvedResources::default()
    };
    let mut harness = Harness::new(
        ScopedResolvedPaths { global, project },
        manager(true),
        ConfigWriteScope::Project,
        true,
    );

    let lines = harness.lines();
    assert_eq!(
        &lines[3..5],
        &[
            "Project Local Resources    tab switch mode · space cycle inherit/+/- · esc close",
            ".notagent/settings.json · inherited global resources are dimmed",
        ]
    );
    assert_eq!(
        &lines[8..13],
        &[
            "  User (/home/u/.notagent/agent/) · inherited global",
            "    Skills",
            ">     [x] alpha.md  inherited global",
            "  Project (.notagent/)",
            "    Skills",
        ]
    );

    // inherit -> unload: the plain pattern is kept next to the signed one.
    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().skills.unwrap(),
        vec![
            "/home/u/.notagent/agent/skills/alpha.md".to_string(),
            "-/home/u/.notagent/agent/skills/alpha.md".to_string(),
        ]
    );
    assert!(
        harness
            .lines()
            .contains(&">     [-] alpha.md  project unload".to_string())
    );

    // unload -> load
    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().skills.unwrap(),
        vec![
            "/home/u/.notagent/agent/skills/alpha.md".to_string(),
            "+/home/u/.notagent/agent/skills/alpha.md".to_string(),
        ]
    );
    assert!(
        harness
            .lines()
            .contains(&">     [+] alpha.md  project load".to_string())
    );

    // load -> inherit clears both entries again.
    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().skills.unwrap(),
        Vec::<String>::new()
    );
    assert!(
        harness
            .lines()
            .contains(&">     [x] alpha.md  inherited global".to_string())
    );

    // A project-local resource that is off inherits nothing and goes to unload.
    harness.key(DOWN);
    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().skills.unwrap(),
        vec!["-skills/local.md".to_string()]
    );
    assert!(
        harness
            .lines()
            .contains(&">     [-] local.md  project unload".to_string())
    );
}

/// says a project scope exists.
#[test]
fn tab_switches_the_write_scope_when_project_mode_is_available() {
    let _guard = test_lock();
    let global = ResolvedResources {
        skills: vec![res(
            "/home/u/.notagent/agent/skills/alpha.md",
            true,
            "auto",
            SourceScope::User,
            SourceOrigin::TopLevel,
            None,
        )],
        ..ResolvedResources::default()
    };

    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global: global.clone(),
            project: empty(),
        },
        manager(true),
        ConfigWriteScope::Global,
        true,
    );
    harness.key(TAB);
    let lines = harness.lines();
    assert_eq!(
        &lines[3..5],
        &[
            "Project Local Resources    tab switch mode · space cycle inherit/+/- · esc close",
            ".notagent/settings.json · inherited global resources are dimmed",
        ]
    );
    assert_eq!(lines[8], "  No resources found");
    assert_eq!(*harness.renders.borrow(), 1);

    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global,
            project: empty(),
        },
        manager(true),
        ConfigWriteScope::Global,
        false,
    );
    harness.key(TAB);
    let lines = harness.lines();
    assert_eq!(
        lines[3],
        "Global Resources                                        space toggle · esc close"
    );
    assert_eq!(lines[10], ">     [x] alpha.md");
    assert_eq!(*harness.renders.borrow(), 0);
}

#[test]
fn the_search_keeps_the_headers_of_matching_items() {
    let _guard = test_lock();
    let global = ResolvedResources {
        skills: vec![
            res(
                "/home/u/.notagent/agent/skills/alpha.md",
                true,
                "auto",
                SourceScope::User,
                SourceOrigin::TopLevel,
                None,
            ),
            res(
                "/home/u/.notagent/agent/skills/beta.md",
                false,
                "auto",
                SourceScope::User,
                SourceOrigin::TopLevel,
                None,
            ),
        ],
        prompts: vec![res(
            "/pkg/wolf/prompts/alphabet.md",
            true,
            "wolf-pack",
            SourceScope::User,
            SourceOrigin::Package,
            Some("/pkg/wolf"),
        )],
        themes: Vec::new(),
    };
    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global,
            project: empty(),
        },
        manager(true),
        ConfigWriteScope::Global,
        true,
    );

    for character in "alpha".chars() {
        harness.key(&character.to_string());
    }
    let lines = harness.lines();
    assert_eq!(lines[6].trim_end(), "> alpha");
    assert_eq!(
        &lines[8..],
        &[
            "  wolf-pack (user)",
            "    Prompts",
            ">     [x] alphabet.md",
            "  User (/home/u/.notagent/agent/)",
            "    Skills",
            "      [x] alpha.md",
            "",
            "────────────────────────────────────────────────────────────────────────────────",
        ]
    );

    for character in "zzz".chars() {
        harness.key(&character.to_string());
    }
    let lines = harness.lines();
    assert_eq!(lines[6].trim_end(), "> alphazzz");
    assert_eq!(lines[8], "  No resources found");
}

/// counts items, not headers.
#[test]
fn scrolls_the_viewport_and_counts_only_items() {
    let _guard = test_lock();
    let skills = (0..30)
        .map(|index| {
            res(
                &format!("/home/u/.notagent/agent/skills/s{index:02}.md"),
                index % 2 == 0,
                "auto",
                SourceScope::User,
                SourceOrigin::TopLevel,
                None,
            )
        })
        .collect();
    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global: ResolvedResources {
                skills,
                ..ResolvedResources::default()
            },
            project: empty(),
        },
        manager(true),
        ConfigWriteScope::Global,
        true,
    );

    for _ in 0..12 {
        harness.key(DOWN);
    }
    let lines = harness.lines();
    assert_eq!(lines[8], "      [x] s04.md");
    assert_eq!(lines[16], ">     [x] s12.md");
    assert_eq!(lines[23], "      [ ] s19.md");
    assert_eq!(lines[24], "  (13/30)");

    harness.key(PAGE_DOWN);
    let lines = harness.lines();
    assert!(lines.contains(&">     [x] s28.md".to_string()));
    assert_eq!(lines[24], "  (29/30)");
}

/// creates a project entry with `autoload: false`, and returning to inherit
/// drops that entry again.
#[test]
fn a_project_package_override_creates_and_removes_its_entry() {
    let _guard = test_lock();
    let package = res(
        "/pkg/wolf/skills/w.md",
        true,
        "wolf-pack",
        SourceScope::User,
        SourceOrigin::Package,
        Some("/pkg/wolf"),
    );
    let settings = manager(true);
    settings.set_packages(&[PackageSource::Source("wolf-pack".to_string())]);
    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global: ResolvedResources {
                skills: vec![package.clone()],
                ..ResolvedResources::default()
            },
            project: ResolvedResources {
                skills: vec![package],
                ..ResolvedResources::default()
            },
        },
        settings,
        ConfigWriteScope::Project,
        true,
    );

    // The source is rewritten relative to the project config directory.
    let override_source = "../../home/u/.notagent/agent/wolf-pack".to_string();

    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: override_source.clone(),
            autoload: Some(false),
            skills: Some(vec!["-skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );
    assert!(
        harness
            .lines()
            .contains(&">     [-] w.md  project unload".to_string())
    );

    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: override_source,
            autoload: Some(false),
            skills: Some(vec!["+skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );

    // Back to inherit: the entry has no filter left and `autoload: false`
    // removes it instead of collapsing it to a source string.
    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().packages.unwrap(),
        Vec::<PackageSource>::new()
    );
    assert!(
        harness
            .lines()
            .contains(&">     [x] w.md  inherited global".to_string())
    );
}

/// it came from before it is written relative to the project directory.
#[test]
fn a_local_package_source_is_rewritten_relative_to_the_project_directory() {
    let _guard = test_lock();
    let package = res(
        "/proj/vendor/skills/w.md",
        true,
        "./vendor",
        SourceScope::User,
        SourceOrigin::Package,
        Some("/proj/vendor"),
    );
    let settings = manager(true);
    settings.set_packages(&[PackageSource::Source("./vendor".to_string())]);
    let mut harness = Harness::new(
        ScopedResolvedPaths {
            global: ResolvedResources {
                skills: vec![package.clone()],
                ..ResolvedResources::default()
            },
            project: ResolvedResources {
                skills: vec![package],
                ..ResolvedResources::default()
            },
        },
        settings,
        ConfigWriteScope::Project,
        true,
    );

    harness.key(" ");
    assert_eq!(
        harness.settings.get_project_settings().packages.unwrap(),
        vec![PackageSource::Filtered(PackageSourceFilter {
            source: "../../home/u/.notagent/agent/vendor".to_string(),
            autoload: Some(false),
            skills: Some(vec!["-skills/w.md".to_string()]),
            ..PackageSourceFilter::default()
        })]
    );
    assert!(
        harness
            .lines()
            .contains(&">     [-] w.md  project unload".to_string())
    );
}
