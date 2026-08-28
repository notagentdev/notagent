pub mod args;

pub mod auth_check;

pub mod auth_command;

pub mod config_selector;
pub mod credential_print;

pub mod file_processor;

pub mod initial_message;

pub mod list_models;

pub mod session_picker;
pub mod startup_ui;

pub async fn run(args: Vec<String>) -> i32 {
    // SAFETY: single-threaded startup, before any task reads the environment.
    unsafe {
        std::env::set_var("NOTAGENT_CODING_AGENT", "true");
        std::env::set_var("AI_AGENT", crate::config::APP_NAME);
    }
    crate::main_app::main(args).await
}
