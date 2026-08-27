//! The six end-to-end scenarios of gate G3, one module each, plus the harness
//! they share.
//!
//! Status (2026-08-16): eight of the eleven scenarios run against C's first
//! slice of the interactive mode — startup, the prompt round-trip, the tool
//! rows and both resizes. The three that drive a slash command (`/model`
//! twice, `/settings` once) stay `#[ignore]`d until the slash-command slice
//! lands; the ignore reason names it. `harness_check` keeps every driver
//! method under test, and `edit_no_full_redraw` is a ported TS suite that
//! needs the render loop but not the interactive mode.

pub mod harness;

mod bash_filter;
mod edit_no_full_redraw;
mod goal;
mod harness_check;
mod leases;
mod mcp;
mod prompt_roundtrip;
mod resize;
mod selectors;
mod side_question;
mod startup;
mod theme_switch;
mod todo_hide;
mod tool_display;
