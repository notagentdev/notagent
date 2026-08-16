//! Interactive coding agent — library surface (port of `packages/coding-agent/src/index.ts`).
//!
//! 1:1 port of `packages/coding-agent` (see crates/notagent/PARITY.md).

pub mod cli;

// Client layer (`src/client/`), ported by workstream B under O-12.
pub mod client;

pub mod config;

pub mod core;

pub mod main_app;

pub mod migrations;

pub mod package_manager_cli;

pub mod modes;

pub mod utils;
