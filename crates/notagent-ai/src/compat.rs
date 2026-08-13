//! Legacy surfaces kept for compatibility.
//!
//! `compat.ts` and `legacy-api-aliases.ts` are excluded (see PARITY.md); only the
//! extension OAuth types that `index.ts` re-exports are ported here, because the
//! coding-agent extension API is typed against them.

pub mod extension_oauth_types;
