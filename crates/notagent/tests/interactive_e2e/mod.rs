//! The six end-to-end scenarios of gate G3, one module each, plus the harness
//! they share.
//!
//! Status (2026-08-16): the harness runs — `harness_check` drives every driver
//! method against a real `TuiMainScreen` over the virtual terminal, and
//! `edit_no_full_redraw` is a ported TS suite that needs the render loop but
//! not the interactive mode, so it runs today. The
//! scenarios themselves wait for the entry point of the interactive mode
//! (C, plan task 13; seam requested in A-23) and are `#[ignore]`d with that
//! reason, as O-11 prescribes.

pub mod harness;

mod edit_no_full_redraw;
mod harness_check;
mod prompt_roundtrip;
mod resize;
mod selectors;
mod startup;
mod theme_switch;
mod tool_display;
