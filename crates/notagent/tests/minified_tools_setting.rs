mod suite;

use suite::{HarnessOptions, create_harness};

const MINIFIED_EDITORS: [&str; 2] = ["patch_minified", "multi_patch_minified"];

#[test]
fn the_minified_tools_setting_decides_what_the_model_is_given_and_told() {
    let harness = create_harness(HarnessOptions {
        settings: Some(serde_json::json!({ "minifiedTools": false })),
        ..HarnessOptions::default()
    });

    let active = harness.session.get_active_tool_names();
    assert!(
        MINIFIED_EDITORS
            .iter()
            .all(|name| !active.contains(&name.to_string())),
        "switched off, no minified editing tool may be active: {active:?}"
    );
    assert!(
        active.contains(&"patch".to_string()),
        "patch stays: {active:?}"
    );
    let prompt = harness.session.system_prompt();
    assert!(
        !prompt.contains("patch_minified") && !prompt.contains("keep_comments"),
        "the system prompt must not mention the minified tools when they are off"
    );

    harness.settings_manager.set_minified_tools_enabled(true);
    harness.session.apply_minified_tools_setting();

    let active = harness.session.get_active_tool_names();
    assert!(
        MINIFIED_EDITORS
            .iter()
            .all(|name| active.contains(&name.to_string())),
        "switched on again, the mode's minified editing tools must come back: {active:?}"
    );
    assert!(
        harness.session.system_prompt().contains("keep_comments"),
        "the compact read guidance returns with the tools"
    );
}
