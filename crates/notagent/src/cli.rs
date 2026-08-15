//! Port of `packages/coding-agent/src/cli/` and of `packages/coding-agent/src/cli.ts`.
//!
//! The two share a name in Rust: `src/cli.rs` is both the module of the `cli/`
//! directory and the place the process setup of `cli.ts` lives. That setup is
//! everything that has to be true before the first argument is read — the
//! marker environment variables other tools look for, and the process title.
//!
//! Deviation (class 3): `process.emitWarning` is silenced in TypeScript to keep
//! Node's deprecation notices off standard error; Rust emits none. The undici
//! dispatcher configuration goes with the Node HTTP stack (`reqwest` is
//! configured per client in the model runtime).

pub mod args;

pub mod auth_check;

pub mod auth_command;

pub mod credential_print;

pub mod file_processor;

pub mod initial_message;

pub mod list_models;

pub mod startup_ui;

/// Sets the process-wide markers of `cli.ts` and runs the app.
pub async fn run(args: Vec<String>) -> i32 {
    // SAFETY: single-threaded startup, before any task reads the environment.
    unsafe {
        std::env::set_var("NOTAGENT_CODING_AGENT", "true");
        std::env::set_var("AI_AGENT", crate::config::APP_NAME);
    }
    crate::main_app::main(args).await
}
