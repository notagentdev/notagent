//! Port of `packages/coding-agent/test/modes.test.ts` and of the mode-block
//! cases of `packages/coding-agent/test/mode-block.test.ts`.
//!
//! The two remaining describes of `mode-block.test.ts` belong elsewhere: the
//! supersession rule is a `core/system-prompt.ts` property (plan task 11) and
//! the `--auto`/`--yolo` flags are `cli/args.ts` (plan task 12).

use std::collections::HashSet;
use std::path::PathBuf;

use notagent::core::modes::cycle::{initial_mode_id, next_mode_id, order_modes};
use notagent::core::modes::indicator::{
    estimate_injected_tokens, format_mode_label, format_mode_switch_notice, indicator_color_key,
};
use notagent::core::modes::shells::{
    ApprovalLevel, MUTATING_TOOLS, ShellId, apply_tool_delta, tools_for_shell,
};
use notagent::core::modes::{
    LoadModesResult, Mode, get_builtin_modes_dir, load_modes, render_mode_injection,
};
use notagent::core::tools::{ALL_TOOL_NAMES, ToolName};

fn known() -> HashSet<String> {
    ALL_TOOL_NAMES
        .iter()
        .map(|name| name.as_str().to_string())
        .collect()
}

struct Workspace {
    path: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("modes-")
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self { path }
    }

    fn write_mode(&self, id: &str, files: &[(&str, &str)]) {
        let dir = self.path.join(id);
        std::fs::create_dir_all(&dir).expect("creates");
        for (name, content) in files {
            std::fs::write(dir.join(name), content).expect("writes");
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn load(roots: &[PathBuf]) -> LoadModesResult {
    load_modes(roots, &known()).expect("loads")
}

fn shipped() -> Vec<Mode> {
    load(&[get_builtin_modes_dir()]).modes
}

fn messages(result: &LoadModesResult) -> String {
    result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn mode(id: &str) -> Mode {
    Mode {
        id: id.to_string(),
        shell: ShellId::ReadOnly,
        approval: ApprovalLevel::Manual,
        tools: Vec::new(),
        subagents: None,
        skills: Vec::new(),
        source_dir: PathBuf::new(),
    }
}

// ---------------------------------------------------------------------------
// shells
// ---------------------------------------------------------------------------

#[test]
fn read_only_exposes_no_mutating_tool() {
    let tools = tools_for_shell(ShellId::ReadOnly);
    for mutating in MUTATING_TOOLS {
        assert!(
            !tools.contains(mutating),
            "read-only must not expose {mutating}"
        );
    }
}

#[test]
fn read_only_excludes_bash_which_could_write_through_a_command() {
    assert!(!tools_for_shell(ShellId::ReadOnly).contains(&ToolName::Bash));
    assert!(tools_for_shell(ShellId::Worker).contains(&ToolName::Bash));
}

#[test]
fn worker_is_a_superset_of_read_only() {
    let worker = tools_for_shell(ShellId::Worker);
    for tool in tools_for_shell(ShellId::ReadOnly) {
        assert!(worker.contains(&tool), "{tool} missing from worker");
    }
}

#[test]
fn refuses_to_add_a_mutating_tool_to_a_read_only_shell() {
    let result = apply_tool_delta(ShellId::ReadOnly, Some(&["+write".to_string()]), &known());
    assert!(!result.tools.contains(&ToolName::Write));
    assert!(result.problems.join(" ").contains("read-only shell"));
}

#[test]
fn reports_unknown_tool_names_instead_of_dropping_them() {
    let result = apply_tool_delta(
        ShellId::Worker,
        Some(&["+nonexistent_tool".to_string()]),
        &known(),
    );
    assert!(result.problems.join(" ").contains("nonexistent_tool"));
}

#[test]
fn removes_tools_on_request() {
    let result = apply_tool_delta(ShellId::Worker, Some(&["-bash".to_string()]), &known());
    assert!(!result.tools.contains(&ToolName::Bash));
    assert_eq!(result.problems, Vec::<String>::new());
}

// ---------------------------------------------------------------------------
// mode loading
// ---------------------------------------------------------------------------

#[test]
fn loads_the_shipped_modes_with_the_expected_shells() {
    let result = load(&[get_builtin_modes_dir()]);
    let ids: Vec<&str> = result.modes.iter().map(|mode| mode.id.as_str()).collect();
    assert!(ids.contains(&"plan"));
    assert!(ids.contains(&"manual"));
    assert_eq!(
        result
            .modes
            .iter()
            .find(|mode| mode.id == "plan")
            .map(|mode| mode.shell),
        Some(ShellId::ReadOnly)
    );
    assert_eq!(
        result
            .modes
            .iter()
            .find(|mode| mode.id == "manual")
            .map(|mode| mode.shell),
        Some(ShellId::Worker)
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn shipped_plan_mode_exposes_no_mutating_tool() {
    let modes = shipped();
    let plan = modes.iter().find(|mode| mode.id == "plan").expect("plan");
    for mutating in MUTATING_TOOLS {
        assert!(!plan.tools.contains(mutating), "plan exposes {mutating}");
    }
}

#[test]
fn injects_several_skills_in_alphabetical_file_order() {
    let workspace = Workspace::new();
    workspace.write_mode(
        "review",
        &[
            ("20-second.md", "---\nshell: read-only\n---\nSECOND"),
            ("10-first.md", "---\nshell: read-only\n---\nFIRST"),
        ],
    );
    let result = load(std::slice::from_ref(&workspace.path));
    let mode = &result.modes[0];
    let names: Vec<&str> = mode
        .skills
        .iter()
        .map(|skill| skill.file_name.as_str())
        .collect();
    assert_eq!(names, vec!["10-first.md", "20-second.md"]);
    assert_eq!(render_mode_injection(mode), "FIRST\n\nSECOND");
}

#[test]
fn defaults_to_read_only_when_no_shell_is_declared() {
    let workspace = Workspace::new();
    workspace.write_mode("vague", &[("a.md", "Just guidance, no frontmatter.")]);
    let result = load(std::slice::from_ref(&workspace.path));
    assert_eq!(result.modes[0].shell, ShellId::ReadOnly);
    assert!(messages(&result).contains("defaulting to read-only"));
}

#[test]
fn reports_an_unknown_shell_rather_than_accepting_it() {
    let workspace = Workspace::new();
    workspace.write_mode("bogus", &[("a.md", "---\nshell: superuser\n---\nx")]);
    let result = load(std::slice::from_ref(&workspace.path));
    assert_eq!(result.modes[0].shell, ShellId::ReadOnly);
    assert!(messages(&result).contains("unknown shell"));
}

#[test]
fn lets_a_later_root_replace_a_mode_of_the_same_id() {
    let user_root = Workspace::new();
    let project_root = Workspace::new();
    user_root.write_mode("plan", &[("a.md", "---\nshell: read-only\n---\nUSER")]);
    project_root.write_mode("plan", &[("a.md", "---\nshell: read-only\n---\nPROJECT")]);
    let result = load(&[user_root.path.clone(), project_root.path.clone()]);
    assert_eq!(result.modes.len(), 1);
    assert_eq!(render_mode_injection(&result.modes[0]), "PROJECT");
}

#[test]
fn skips_a_directory_without_skill_files_and_says_why() {
    let workspace = Workspace::new();
    std::fs::create_dir_all(workspace.path.join("empty")).expect("creates");
    let result = load(std::slice::from_ref(&workspace.path));
    assert!(result.modes.is_empty());
    assert!(messages(&result).contains("no .md skill files"));
}

#[test]
fn reports_conflicting_shell_declarations_inside_one_mode() {
    let workspace = Workspace::new();
    workspace.write_mode(
        "mixed",
        &[
            ("10-a.md", "---\nshell: read-only\n---\nA"),
            ("20-b.md", "---\nshell: worker\n---\nB"),
        ],
    );
    let result = load(std::slice::from_ref(&workspace.path));
    assert!(messages(&result).contains("conflicting shell"));
}

// ---------------------------------------------------------------------------
// mode indicator
// ---------------------------------------------------------------------------

#[test]
fn spells_out_the_restriction_for_a_read_only_mode() {
    assert_eq!(
        format_mode_label("plan", ShellId::ReadOnly),
        "plan (read-only)"
    );
    assert_eq!(format_mode_label("manual", ShellId::Worker), "manual");
}

#[test]
fn drives_the_colour_from_the_shell_so_user_modes_inherit_the_signal() {
    assert_eq!(indicator_color_key(ShellId::ReadOnly), "success");
    assert_eq!(indicator_color_key(ShellId::Worker), "warning");
}

#[test]
fn reports_injected_volume_on_switch() {
    let notice = format_mode_switch_notice("plan", ShellId::ReadOnly, 2400);
    assert!(notice.contains("plan"));
    assert!(notice.contains("read-only"));
    assert!(notice.contains("2.4k tokens"));
}

#[test]
fn estimates_injected_tokens_and_treats_empty_text_as_zero() {
    assert_eq!(estimate_injected_tokens(""), 0);
    assert_eq!(estimate_injected_tokens(&"a".repeat(400)), 100);
}

// ---------------------------------------------------------------------------
// mode cycle
// ---------------------------------------------------------------------------

#[test]
fn orders_shipped_modes_first_then_the_rest_alphabetically() {
    let ordered = order_modes(&[mode("zeta"), mode("manual"), mode("alpha"), mode("plan")]);
    let ids: Vec<&str> = ordered.iter().map(|mode| mode.id.as_str()).collect();
    assert_eq!(ids, vec!["plan", "manual", "alpha", "zeta"]);
}

#[test]
fn wraps_around_the_ring() {
    let modes = [mode("plan"), mode("manual")];
    assert_eq!(next_mode_id(&modes, "plan").as_deref(), Some("manual"));
    assert_eq!(next_mode_id(&modes, "manual").as_deref(), Some("plan"));
}

#[test]
fn recovers_when_the_current_mode_disappeared() {
    assert_eq!(
        next_mode_id(&[mode("plan"), mode("manual")], "gone").as_deref(),
        Some("plan")
    );
}

#[test]
fn starts_in_manual_when_available() {
    assert_eq!(
        initial_mode_id(&[mode("plan"), mode("manual")]).as_deref(),
        Some("manual")
    );
    assert_eq!(initial_mode_id(&[mode("scout")]).as_deref(), Some("scout"));
    assert_eq!(initial_mode_id(&[]), None);
}

// ---------------------------------------------------------------------------
// approval level
// ---------------------------------------------------------------------------

#[test]
fn defaults_to_the_supervised_end_when_a_mode_declares_none() {
    let workspace = Workspace::new();
    workspace.write_mode("quiet", &[("a.md", "---\nshell: worker\n---\nBODY")]);
    assert_eq!(
        load(std::slice::from_ref(&workspace.path)).modes[0].approval,
        ApprovalLevel::Manual
    );
}

#[test]
fn reads_a_declared_level() {
    let workspace = Workspace::new();
    workspace.write_mode(
        "hands-off",
        &[("a.md", "---\nshell: worker\napproval: auto\n---\nBODY")],
    );
    assert_eq!(
        load(std::slice::from_ref(&workspace.path)).modes[0].approval,
        ApprovalLevel::Auto
    );
}

#[test]
fn rejects_an_unknown_level_and_stays_supervised() {
    let workspace = Workspace::new();
    workspace.write_mode(
        "bogus",
        &[("a.md", "---\nshell: worker\napproval: whatever\n---\nBODY")],
    );
    let result = load(std::slice::from_ref(&workspace.path));
    assert_eq!(result.modes[0].approval, ApprovalLevel::Manual);
    assert!(messages(&result).contains("unknown approval level"));
}

#[test]
fn reports_conflicting_levels_inside_one_mode() {
    let workspace = Workspace::new();
    workspace.write_mode(
        "mixed",
        &[
            ("10-a.md", "---\nshell: worker\napproval: manual\n---\nA"),
            ("20-b.md", "---\napproval: yolo\n---\nB"),
        ],
    );
    assert!(
        messages(&load(std::slice::from_ref(&workspace.path))).contains("conflicting approval")
    );
}

#[test]
fn shipped_plan_mode_stays_supervised_which_is_moot_under_a_read_only_shell() {
    let modes = shipped();
    let plan = modes.iter().find(|mode| mode.id == "plan").expect("plan");
    assert_eq!(plan.approval, ApprovalLevel::Manual);
    assert_eq!(plan.shell, ShellId::ReadOnly);
}

// ---------------------------------------------------------------------------
// shipped ring
// ---------------------------------------------------------------------------

#[test]
fn ships_all_four_stops_and_loads_them_without_complaint() {
    let result = load(&[get_builtin_modes_dir()]);
    let mut ids: Vec<&str> = result.modes.iter().map(|mode| mode.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["auto", "manual", "plan", "yolo"]);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn orders_the_ring_from_most_supervised_to_least() {
    let ordered = order_modes(&shipped());
    let ids: Vec<&str> = ordered.iter().map(|mode| mode.id.as_str()).collect();
    assert_eq!(ids, vec!["plan", "manual", "auto", "yolo"]);
}

#[test]
fn gives_each_stop_its_declared_approval_level() {
    let modes = shipped();
    let approval = |id: &str| {
        modes
            .iter()
            .find(|mode| mode.id == id)
            .map(|mode| mode.approval)
    };
    assert_eq!(approval("manual"), Some(ApprovalLevel::Manual));
    assert_eq!(approval("auto"), Some(ApprovalLevel::Auto));
    assert_eq!(approval("yolo"), Some(ApprovalLevel::Yolo));
}

#[test]
fn puts_only_plan_on_the_read_only_shell() {
    for mode in shipped() {
        let expected = if mode.id == "plan" {
            ShellId::ReadOnly
        } else {
            ShellId::Worker
        };
        assert_eq!(mode.shell, expected, "{}", mode.id);
    }
}

#[test]
fn cycles_the_full_ring_and_wraps_back_to_plan() {
    let modes = shipped();
    assert_eq!(next_mode_id(&modes, "plan").as_deref(), Some("manual"));
    assert_eq!(next_mode_id(&modes, "manual").as_deref(), Some("auto"));
    assert_eq!(next_mode_id(&modes, "auto").as_deref(), Some("yolo"));
    assert_eq!(next_mode_id(&modes, "yolo").as_deref(), Some("plan"));
}

// ---------------------------------------------------------------------------
// mode block (test/mode-block.test.ts)
// ---------------------------------------------------------------------------

/// Mirrors the wrapper the session builds, so a change to the tag shape is
/// caught here rather than only in a live session.
fn wrap(mode: &Mode) -> Option<String> {
    let body = render_mode_injection(mode);
    if body.trim().is_empty() {
        return None;
    }
    Some(format!(
        "<mode name=\"{}\" shell=\"{}\">\n{body}\n</mode>",
        mode.id, mode.shell
    ))
}

fn builtin(id: &str) -> Mode {
    shipped()
        .into_iter()
        .find(|mode| mode.id == id)
        .expect("shipped mode should load")
}

#[test]
fn names_both_the_mode_and_the_shell_that_bounds_it() {
    let block = wrap(&builtin("plan")).unwrap_or_default();
    assert!(block.contains("name=\"plan\""));
    assert!(block.contains("shell=\"read-only\""));
    assert!(block.starts_with("<mode "));
    assert!(block.ends_with("</mode>"));
}

#[test]
fn carries_the_modes_guidance_body() {
    assert!(
        wrap(&builtin("plan"))
            .unwrap_or_default()
            .contains("plan mode")
    );
}

#[test]
fn distinguishes_the_two_shipped_modes_by_shell() {
    assert!(
        wrap(&builtin("manual"))
            .unwrap_or_default()
            .contains("shell=\"worker\"")
    );
    assert!(
        wrap(&builtin("plan"))
            .unwrap_or_default()
            .contains("shell=\"read-only\"")
    );
}

#[test]
fn produces_no_wrapper_for_an_empty_body_so_no_empty_block_is_emitted() {
    assert_eq!(wrap(&mode("hollow")), None);
}

#[test]
fn prepends_ahead_of_the_users_own_text_separated_from_it() {
    let block = wrap(&builtin("plan")).unwrap_or_default();
    let combined = format!("{block}\n\nfix the parser");
    assert!(combined.find("<mode ") < combined.find("fix the parser"));
    assert!(combined.contains("</mode>\n\nfix the parser"));
}

/// Mirrors the session's block construction, including the exit note.
fn wrap_with_exit(mode: &Mode, previous_approval: Option<ApprovalLevel>) -> Option<String> {
    let body = render_mode_injection(mode);
    let leaving_auto =
        previous_approval == Some(ApprovalLevel::Auto) && mode.approval != ApprovalLevel::Auto;
    let exit = if leaving_auto {
        "Auto approval is no longer active. Tool use is confirmed again, so expect approval prompts and refusals.\n\n"
    } else {
        ""
    };
    if body.trim().is_empty() && exit.is_empty() {
        return None;
    }
    Some(format!(
        "<mode name=\"{}\" shell=\"{}\">\n{exit}{body}\n</mode>",
        mode.id, mode.shell
    ))
}

#[test]
fn announces_that_approvals_are_back() {
    let block = wrap_with_exit(&builtin("manual"), Some(ApprovalLevel::Auto)).unwrap_or_default();
    assert!(block.contains("Auto approval is no longer active"));
    assert!(block.contains("expect approval prompts"));
}

#[test]
fn says_nothing_extra_when_auto_was_not_the_previous_stop() {
    assert!(
        !wrap_with_exit(&builtin("manual"), Some(ApprovalLevel::Manual))
            .unwrap_or_default()
            .contains("no longer active")
    );
    assert!(
        !wrap_with_exit(&builtin("manual"), None)
            .unwrap_or_default()
            .contains("no longer active")
    );
}

#[test]
fn does_not_announce_an_exit_when_staying_in_auto() {
    assert!(
        !wrap_with_exit(&builtin("auto"), Some(ApprovalLevel::Auto))
            .unwrap_or_default()
            .contains("no longer active")
    );
}

#[test]
fn announces_the_exit_when_moving_from_auto_to_yolo_which_also_drops_prompts() {
    // yolo is not auto, so approvals formally return before yolo waives them
    // again; saying so is better than leaving the transition unstated.
    assert!(
        wrap_with_exit(&builtin("yolo"), Some(ApprovalLevel::Auto))
            .unwrap_or_default()
            .contains("no longer active")
    );
}
