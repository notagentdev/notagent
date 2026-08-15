//! Port of the settings-facing half of
//! `packages/coding-agent/src/core/http-dispatcher.ts` (111 LOC).
//!
//! Ownership handed to workstream A with interface request O-6, which covers
//! `HTTP_IDLE_TIMEOUT_CHOICES` and `formatHttpIdleTimeoutMs` — the two items
//! the settings selector reads. `DEFAULT_HTTP_IDLE_TIMEOUT_MS` and
//! `parseHttpIdleTimeoutMs` were pulled forward into
//! `core::settings_manager` with workstream C's task 5 (the settings manager
//! cannot load without them) and are re-exported here so the module mirrors
//! the public surface of the TypeScript file.
//!
//! The dispatcher half itself (`configureHttpDispatcher`,
//! `applyHttpProxySettings`, the undici plumbing) stays with workstream C: it
//! configures the HTTP client of the process, not a setting.

pub use crate::core::settings_manager::{DEFAULT_HTTP_IDLE_TIMEOUT_MS, parse_http_idle_timeout_ms};

/// One entry of `HTTP_IDLE_TIMEOUT_CHOICES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpIdleTimeoutChoice {
    pub label: &'static str,
    pub timeout_ms: u64,
}

/// The timeouts the settings selector offers, in the order it shows them.
pub const HTTP_IDLE_TIMEOUT_CHOICES: [HttpIdleTimeoutChoice; 5] = [
    HttpIdleTimeoutChoice {
        label: "30 sec",
        timeout_ms: 30_000,
    },
    HttpIdleTimeoutChoice {
        label: "1 min",
        timeout_ms: 60_000,
    },
    HttpIdleTimeoutChoice {
        label: "2 min",
        timeout_ms: 120_000,
    },
    HttpIdleTimeoutChoice {
        label: "5 min",
        timeout_ms: 300_000,
    },
    HttpIdleTimeoutChoice {
        label: "disabled",
        timeout_ms: 0,
    },
];

/// The label of a timeout, or `<seconds> sec` for a value no choice carries.
pub fn format_http_idle_timeout_ms(timeout_ms: u64) -> String {
    if let Some(choice) = HTTP_IDLE_TIMEOUT_CHOICES
        .iter()
        .find(|item| item.timeout_ms == timeout_ms)
    {
        return choice.label.to_string();
    }
    // `${timeoutMs / 1000} sec` — JavaScript division, so a value that is not a
    // multiple of a second keeps its fraction and an integer prints without one.
    format!(
        "{} sec",
        notagent_ai::utils::js_number::to_js_string(timeout_ms as f64 / 1000.0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_the_offered_timeouts() {
        // Values from `formatHttpIdleTimeoutMs` in http-dispatcher.ts.
        assert_eq!(format_http_idle_timeout_ms(30_000), "30 sec");
        assert_eq!(format_http_idle_timeout_ms(60_000), "1 min");
        assert_eq!(format_http_idle_timeout_ms(120_000), "2 min");
        assert_eq!(format_http_idle_timeout_ms(300_000), "5 min");
        assert_eq!(format_http_idle_timeout_ms(0), "disabled");
    }

    #[test]
    fn falls_back_to_seconds_for_an_unlisted_timeout() {
        assert_eq!(format_http_idle_timeout_ms(45_000), "45 sec");
        assert_eq!(format_http_idle_timeout_ms(1_500), "1.5 sec");
        assert_eq!(format_http_idle_timeout_ms(1), "0.001 sec");
    }
}
