//! Gate G3 — the end-to-end scenarios over the virtual terminal.
//! Interactive end-to-end scenarios over the virtual terminal: startup,
//! prompt round-trip, tool display, selector operation, theme switching, and
//! resizing.
//! Everything below the scenario is the production path: the app runtime of the
//! headless G2 suites (real services, real session, only the provider scripted)
//! and the virtual terminal of the TUI crate. The one piece that is not on main
//! yet is the entry point of the interactive mode (C, plan task 13) — see

mod app_runtime;
mod interactive_e2e;
