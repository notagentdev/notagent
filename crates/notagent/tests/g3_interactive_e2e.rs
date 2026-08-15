//! Gate G3 — the end-to-end scenarios over the virtual terminal.
//!
//! The master plan asks of G3: "interactive mode fully wired […], end-to-end
//! scenarios over the virtual terminal green"
//! (`plans/2026-08-13-rust-port-master-v1.md`, section Gates). The scenarios it
//! names are startup, prompt round-trip, tool display, selector operation,
//! theme switch and resize; each has a module under `tests/interactive_e2e/`.
//!
//! Everything below the scenario is the production path: the app runtime of the
//! headless G2 suites (real services, real session, only the provider scripted)
//! and the virtual terminal of the TUI crate. The one piece that is not on main
//! yet is the entry point of the interactive mode (C, plan task 13) — see
//! `interactive_e2e::harness` and interface request A-23.

mod app_runtime;
mod interactive_e2e;
