use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::modes::interactive::components::first_time_setup::{
    FirstTimeSetupComponent, FirstTimeSetupOptions, FirstTimeSetupResult,
};
use notagent::modes::interactive::components::list_selector::{
    ListSelectorComponent, ListSelectorOptions,
};
use notagent::modes::interactive::components::oauth_selector::{
    AuthSelectorMode, AuthSelectorProvider, OAuthSelectorComponent,
};
use notagent::modes::interactive::components::show_images_selector::ShowImagesSelectorComponent;
use notagent::modes::interactive::components::theme_selector::ThemeSelectorComponent;
use notagent::modes::interactive::components::thinking_selector::ThinkingSelectorComponent;
use notagent::modes::interactive::components::user_message_selector::{
    UserMessageItem, UserMessageSelectorComponent,
};
use notagent::modes::interactive::theme::theme::{TerminalTheme, init_theme};
use notagent::utils::ansi::strip_ansi;
use notagent_agent::ThinkingLevel;
use notagent_ai::auth::types::{AuthCheck, AuthType};
use notagent_tui::tui::Component;

fn theme_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

const ESCAPE: &str = "\x1b";
const ENTER: &str = "\r";
const ARROW_DOWN: &str = "\x1b[B";
const ARROW_UP: &str = "\x1b[A";

#[test]
fn moves_the_cursor_and_confirms_the_selected_option() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let selected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let cancelled = Rc::new(RefCell::new(0usize));

    let sink = Rc::clone(&selected);
    let cancel_sink = Rc::clone(&cancelled);
    let mut selector = ListSelectorComponent::new(
        "Pick one",
        vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        Box::new(move |option| sink.borrow_mut().push(option)),
        Box::new(move || *cancel_sink.borrow_mut() += 1),
        None,
    );

    let rendered = strip_ansi(&selector.render(40).join("\n"));
    assert!(rendered.contains("Pick one"), "{rendered}");
    assert!(rendered.contains("→ alpha"), "{rendered}");
    assert!(rendered.contains("  beta"), "{rendered}");

    selector.handle_input(ARROW_DOWN);
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ beta"));
    // `j`/`k` move as well.
    selector.handle_input("j");
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ gamma"));
    // The cursor clamps at both ends.
    selector.handle_input("j");
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ gamma"));
    selector.handle_input(ARROW_UP);
    selector.handle_input("k");
    selector.handle_input("k");
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("→ alpha"));

    selector.handle_input(ENTER);
    assert_eq!(*selected.borrow(), vec!["alpha"]);

    selector.handle_input(ESCAPE);
    assert_eq!(*cancelled.borrow(), 1);
}

#[test]
fn counts_the_timeout_down_in_the_title_and_cancels_at_zero() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let cancelled = Rc::new(RefCell::new(0usize));
    let cancel_sink = Rc::clone(&cancelled);

    let mut selector = ListSelectorComponent::new(
        "Confirm",
        vec!["yes".to_string()],
        Box::new(|_| {}),
        Box::new(move || *cancel_sink.borrow_mut() += 1),
        Some(ListSelectorOptions {
            timeout: Some(1000),
            on_toggle_tools_expanded: None,
        }),
    );

    assert!(strip_ansi(&selector.render(40).join("\n")).contains("Confirm (1s)"));
    assert!(selector.countdown_deadline().is_some());

    std::thread::sleep(std::time::Duration::from_millis(1050));
    assert!(selector.tick_countdown());
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("Confirm (0s)"));
    assert_eq!(*cancelled.borrow(), 1);
    assert!(selector.countdown_deadline().is_none());
}

// --- thinking selector ---------------------------------------------------------

#[test]
fn preselects_the_current_thinking_level_and_reports_the_choice() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let chosen: Rc<RefCell<Vec<ThinkingLevel>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&chosen);

    let mut selector = ThinkingSelectorComponent::new(
        ThinkingLevel::Medium,
        vec![
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ],
        Box::new(move |level| sink.borrow_mut().push(level)),
        Box::new(|| {}),
    );

    let rendered = strip_ansi(&selector.render(60).join("\n"));
    assert!(
        rendered.contains("Moderate reasoning (~8k tokens)"),
        "{rendered}"
    );
    assert_eq!(
        selector
            .get_select_list()
            .borrow()
            .get_selected_item()
            .map(|item| item.value),
        Some("medium".to_string())
    );

    selector.handle_input(ENTER);
    assert_eq!(*chosen.borrow(), vec![ThinkingLevel::Medium]);
}

// --- theme selector -------------------------------------------------------------

#[test]
fn marks_the_current_theme_and_previews_on_selection_change() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let previews: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let selected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let preview_sink = Rc::clone(&previews);
    let select_sink = Rc::clone(&selected);

    let mut selector = ThemeSelectorComponent::new(
        "dark",
        Box::new(move |name| select_sink.borrow_mut().push(name)),
        Box::new(|| {}),
        Box::new(move |name| preview_sink.borrow_mut().push(name)),
    );

    let rendered = strip_ansi(&selector.render(60).join("\n"));
    assert!(rendered.contains("dark"), "{rendered}");
    assert!(rendered.contains("(current)"), "{rendered}");
    // The built-in themes sort as dark, light.
    assert_eq!(
        selector
            .get_select_list()
            .borrow()
            .get_selected_item()
            .map(|item| item.value),
        Some("dark".to_string())
    );

    selector.handle_input(ARROW_DOWN);
    assert_eq!(*previews.borrow(), vec!["light"]);
    selector.handle_input(ENTER);
    assert_eq!(*selected.borrow(), vec!["light"]);
}

// --- show images selector --------------------------------------------------------

#[test]
fn maps_the_show_images_answer_back_to_a_boolean() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let answers: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&answers);

    let mut selector = ShowImagesSelectorComponent::new(
        false,
        Box::new(move |show| sink.borrow_mut().push(show)),
        Box::new(|| {}),
    );
    // `false` preselects "No".
    assert_eq!(
        selector
            .get_select_list()
            .borrow()
            .get_selected_item()
            .map(|item| item.value),
        Some("no".to_string())
    );
    selector.handle_input(ENTER);
    assert_eq!(*answers.borrow(), vec![false]);

    selector.handle_input(ARROW_UP);
    selector.handle_input(ENTER);
    assert_eq!(*answers.borrow(), vec![false, true]);
}

// --- user message selector --------------------------------------------------------

fn message(id: &str, text: &str) -> UserMessageItem {
    UserMessageItem {
        id: id.to_string(),
        text: text.to_string(),
        timestamp: None,
    }
}

#[test]
fn starts_at_the_newest_message_and_wraps_around() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);
    let selected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&selected);

    let mut selector = UserMessageSelectorComponent::new(
        vec![
            message("a", "first"),
            message("b", "second"),
            message("c", "third"),
        ],
        Box::new(move |id| sink.borrow_mut().push(id.to_string())),
        Box::new(|| {}),
        None,
    );

    let rendered = strip_ansi(&selector.render(40).join("\n"));
    assert!(rendered.contains("Fork from Message"), "{rendered}");
    assert!(rendered.contains("› third"), "{rendered}");
    assert!(rendered.contains("Message 3 of 3"), "{rendered}");

    // Down at the end wraps to the top.
    selector.handle_input(ARROW_DOWN);
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("› first"));
    // Up at the top wraps to the end.
    selector.handle_input(ARROW_UP);
    assert!(strip_ansi(&selector.render(40).join("\n")).contains("› third"));

    selector.handle_input(ENTER);
    assert_eq!(*selected.borrow(), vec!["c"]);
}

#[test]
fn starts_at_the_requested_message_and_normalises_newlines() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut selector = UserMessageSelectorComponent::new(
        vec![message("a", "one\ntwo"), message("b", "other")],
        Box::new(|_| {}),
        Box::new(|| {}),
        Some("a"),
    );

    let rendered = strip_ansi(&selector.render(40).join("\n"));
    assert!(rendered.contains("› one two"), "{rendered}");
    assert!(!selector.is_empty());
}

#[test]
fn reports_an_empty_message_list() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut selector =
        UserMessageSelectorComponent::new(Vec::new(), Box::new(|_| {}), Box::new(|| {}), None);
    assert!(selector.is_empty());
    assert!(
        strip_ansi(&selector.render(40).join("\n")).contains("No user messages found"),
        "{:?}",
        selector.render(40)
    );
}

// the suite's first case exercises `InteractiveMode.getLoginProviderOptions`,

fn auth_provider(
    id: &str,
    name: &str,
    auth_type: AuthType,
    status: Option<AuthCheck>,
) -> AuthSelectorProvider {
    AuthSelectorProvider {
        id: id.to_string(),
        name: name.to_string(),
        auth_type,
        method: None,
        status,
        experimental: false,
    }
}

fn auth_check(check_type: AuthType, source: &str) -> AuthCheck {
    AuthCheck {
        source: Some(source.to_string()),
        check_type,
    }
}

fn render_oauth_selector(provider: AuthSelectorProvider) -> String {
    let mut selector = OAuthSelectorComponent::new(
        AuthSelectorMode::Login,
        vec![provider],
        Box::new(|_, _| {}),
        Box::new(|| {}),
        None,
    );
    strip_ansi(&selector.render(120).join("\n"))
}

#[test]
fn renders_an_option_without_compiled_auth_status_as_unconfigured() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let output = render_oauth_selector(auth_provider("google", "Google", AuthType::ApiKey, None));
    assert!(output.contains("unconfigured"), "{output}");
    assert!(!output.contains("✓ configured"), "{output}");
}

#[test]
fn labels_an_experimental_provider_behind_its_name() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut provider = auth_provider("mtplx", "MTPLX (local)", AuthType::ApiKey, None);
    provider.experimental = true;
    let output = render_oauth_selector(provider);
    assert!(output.contains("MTPLX (local) (experimental)"), "{output}");
}

#[test]
fn shows_oauth_auth_distinctly_in_the_api_key_selector() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let output = render_oauth_selector(auth_provider(
        "anthropic",
        "Anthropic",
        AuthType::ApiKey,
        Some(auth_check(AuthType::OAuth, "OAuth")),
    ));
    assert!(output.contains("subscription configured"), "{output}");
}

#[test]
fn shows_environment_api_key_auth_as_configured() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let output = render_oauth_selector(auth_provider(
        "openai",
        "OpenAI",
        AuthType::ApiKey,
        Some(auth_check(AuthType::ApiKey, "OPENAI_API_KEY")),
    ));
    assert!(output.contains("✓ env: OPENAI_API_KEY"), "{output}");
    assert!(!output.contains("unconfigured"), "{output}");
}

#[test]
fn shows_models_json_api_key_auth_as_configured() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let output = render_oauth_selector(auth_provider(
        "local-proxy",
        "local-proxy",
        AuthType::ApiKey,
        Some(auth_check(AuthType::ApiKey, "key in models.json")),
    ));
    assert!(output.contains("✓ key in models.json"), "{output}");
}

#[test]
fn shows_models_json_command_auth_as_configured() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let output = render_oauth_selector(auth_provider(
        "op-proxy",
        "op-proxy",
        AuthType::ApiKey,
        Some(auth_check(AuthType::ApiKey, "command in models.json")),
    ));
    assert!(output.contains("✓ command in models.json"), "{output}");
}

#[test]
fn labels_the_auth_type_only_with_mixed_providers_and_filters_by_search() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let selected = Rc::new(RefCell::new(Vec::<(String, AuthType)>::new()));
    let sink = Rc::clone(&selected);
    let mut selector = OAuthSelectorComponent::new(
        AuthSelectorMode::Login,
        vec![
            auth_provider("anthropic", "Anthropic", AuthType::OAuth, None),
            auth_provider("openai", "OpenAI", AuthType::ApiKey, None),
        ],
        Box::new(move |id, auth_type| {
            sink.borrow_mut().push((id.to_string(), auth_type));
        }),
        Box::new(|| {}),
        None,
    );

    let rendered = strip_ansi(&selector.render(120).join("\n"));
    assert!(rendered.contains("Anthropic [subscription]"), "{rendered}");
    assert!(rendered.contains("OpenAI [API key]"), "{rendered}");

    // Typing filters through the search input; the selection index is clamped.
    for character in "openai".chars() {
        selector.handle_input(&character.to_string());
    }
    let rendered = strip_ansi(&selector.render(120).join("\n"));
    assert!(!rendered.contains("Anthropic"), "{rendered}");
    assert!(rendered.contains("OpenAI"), "{rendered}");

    selector.handle_input(ENTER);
    assert_eq!(
        *selected.borrow(),
        [("openai".to_string(), AuthType::ApiKey)]
    );
}

#[test]
fn reports_an_empty_provider_list_per_mode() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let mut login = OAuthSelectorComponent::new(
        AuthSelectorMode::Login,
        Vec::new(),
        Box::new(|_, _| {}),
        Box::new(|| {}),
        None,
    );
    assert!(
        strip_ansi(&login.render(60).join("\n")).contains("No providers available"),
        "{:?}",
        login.render(60)
    );

    let mut logout = OAuthSelectorComponent::new(
        AuthSelectorMode::Logout,
        Vec::new(),
        Box::new(|_, _| {}),
        Box::new(|| {}),
        None,
    );
    assert!(
        strip_ansi(&logout.render(60).join("\n"))
            .contains("No providers logged in. Use /login first."),
        "{:?}",
        logout.render(60)
    );

    let mut filtered = OAuthSelectorComponent::new(
        AuthSelectorMode::Login,
        vec![auth_provider(
            "anthropic",
            "Anthropic",
            AuthType::OAuth,
            None,
        )],
        Box::new(|_, _| {}),
        Box::new(|| {}),
        Some("zzz"),
    );
    assert!(
        strip_ansi(&filtered.render(60).join("\n")).contains("No matching providers"),
        "{:?}",
        filtered.render(60)
    );
}

#[test]
fn cancels_on_escape_and_ignores_navigation_without_providers() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let cancelled = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&cancelled);
    let mut selector = OAuthSelectorComponent::new(
        AuthSelectorMode::Login,
        Vec::new(),
        Box::new(|_, _| {}),
        Box::new(move || *sink.borrow_mut() += 1),
        None,
    );

    selector.handle_input(ARROW_DOWN);
    selector.handle_input(ARROW_UP);
    selector.handle_input(ESCAPE);
    assert_eq!(*cancelled.borrow(), 1);
}

#[test]
fn starts_on_the_detected_theme_and_previews_while_moving() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let previews = Rc::new(RefCell::new(Vec::<TerminalTheme>::new()));
    let sink = Rc::clone(&previews);
    let mut setup = FirstTimeSetupComponent::new(FirstTimeSetupOptions {
        detected_theme: TerminalTheme::Light,
        on_theme_preview: Box::new(move |theme| sink.borrow_mut().push(theme)),
        on_submit: Box::new(|_| {}),
        on_cancel: Box::new(|| {}),
    });

    let rendered = strip_ansi(&setup.render(80).join("\n"));
    assert!(
        rendered.contains("Detected system appearance: light"),
        "{rendered}"
    );
    assert!(rendered.contains("→ Light"), "{rendered}");
    assert!(rendered.contains("  Dark"), "{rendered}");

    // At the bottom edge nothing changes, so no preview fires.
    setup.handle_input(ARROW_DOWN);
    assert!(previews.borrow().is_empty());

    setup.handle_input(ARROW_UP);
    assert_eq!(*previews.borrow(), [TerminalTheme::Dark]);
    assert!(
        strip_ansi(&setup.render(80).join("\n")).contains("→ Dark"),
        "{:?}",
        setup.render(80)
    );
}

#[test]
fn confirms_the_theme_step_before_submitting_the_analytics_choice() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let submitted = Rc::new(RefCell::new(Vec::<FirstTimeSetupResult>::new()));
    let sink = Rc::clone(&submitted);
    let mut setup = FirstTimeSetupComponent::new(FirstTimeSetupOptions {
        detected_theme: TerminalTheme::Dark,
        on_theme_preview: Box::new(|_| {}),
        on_submit: Box::new(move |result| sink.borrow_mut().push(result)),
        on_cancel: Box::new(|| {}),
    });

    assert!(
        strip_ansi(&setup.render(80).join("\n")).contains("continue"),
        "{:?}",
        setup.render(80)
    );

    setup.handle_input(ENTER);
    let rendered = strip_ansi(&setup.render(80).join("\n"));
    assert!(
        rendered.contains("Opt-in to anonymous usage data sharing?"),
        "{rendered}"
    );
    assert!(
        rendered.contains("→ Share anonymous usage data"),
        "{rendered}"
    );
    assert!(rendered.contains("finish"), "{rendered}");
    assert!(submitted.borrow().is_empty());

    // `j` moves down like the arrow key.
    setup.handle_input("j");
    setup.handle_input(ENTER);
    assert_eq!(
        *submitted.borrow(),
        [FirstTimeSetupResult {
            theme: TerminalTheme::Dark,
            share_analytics: false,
        }]
    );
}

#[test]
fn skips_the_setup_on_cancel() {
    let _guard = theme_lock();
    init_theme(Some("dark"), false);

    let cancelled = Rc::new(RefCell::new(0usize));
    let sink = Rc::clone(&cancelled);
    let mut setup = FirstTimeSetupComponent::new(FirstTimeSetupOptions {
        detected_theme: TerminalTheme::Dark,
        on_theme_preview: Box::new(|_| {}),
        on_submit: Box::new(|_| {}),
        on_cancel: Box::new(move || *sink.borrow_mut() += 1),
    });

    setup.handle_input(ESCAPE);
    assert_eq!(*cancelled.borrow(), 1);
}
