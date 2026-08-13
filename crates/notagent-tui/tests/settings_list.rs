//! Port of `packages/tui/test/settings-list.test.ts` (58 LOC).

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::settings_list::{
    SettingItem, SettingsList, SettingsListOptions, SettingsListTheme,
};
use notagent_tui::tui::Component;

fn test_theme() -> SettingsListTheme {
    SettingsListTheme {
        label: Rc::new(|text, _| text.to_string()),
        value: Rc::new(|text, _| text.to_string()),
        description: Rc::new(|text| text.to_string()),
        cursor: "> ".to_string(),
        hint: Rc::new(|text| text.to_string()),
    }
}

fn items() -> Vec<SettingItem> {
    vec![SettingItem {
        id: "tui-mode".to_string(),
        label: "TUI mode".to_string(),
        description: None,
        current_value: "regular".to_string(),
        values: Some(vec!["regular".to_string(), "fullscreen".to_string()]),
        submenu: None,
    }]
}

type Changes = Rc<RefCell<Vec<(String, String)>>>;

fn settings_list(changes: &Changes) -> SettingsList {
    let sink = Rc::clone(changes);
    SettingsList::new(
        items(),
        10,
        test_theme(),
        Box::new(move |id, value| {
            sink.borrow_mut().push((id.to_string(), value.to_string()));
        }),
        Box::new(|| {}),
        SettingsListOptions {
            enable_search: true,
        },
    )
}

#[test]
fn includes_spaces_in_an_active_search_instead_of_changing_the_selected_setting() {
    let changes: Changes = Rc::new(RefCell::new(Vec::new()));
    let mut list = settings_list(&changes);

    for character in "TUI mode".chars() {
        list.handle_input(&character.to_string());
    }

    assert!(changes.borrow().is_empty());
    assert!(
        list.render(80)[0].contains("TUI mode"),
        "search input shows the query"
    );

    list.handle_input("\r");
    assert_eq!(
        changes.borrow().as_slice(),
        [("tui-mode".to_string(), "fullscreen".to_string())]
    );
}

#[test]
fn keeps_space_as_a_change_shortcut_before_a_search_query_is_entered() {
    let changes: Changes = Rc::new(RefCell::new(Vec::new()));
    let mut list = settings_list(&changes);

    list.handle_input(" ");

    assert_eq!(
        changes.borrow().as_slice(),
        [("tui-mode".to_string(), "fullscreen".to_string())]
    );
}
