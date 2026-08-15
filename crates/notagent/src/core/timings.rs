//! Port of `packages/coding-agent/src/core/timings.ts`.
//!
//! Startup profiling, off unless `NOTAGENT_TIMING=1`. The measurements go to
//! standard error, never to standard out — a `-p` run pipes its answer onward
//! and a timing table in that stream would corrupt it.
//!
//! Deviation (class 2): the TypeScript knows two namespaces, `main` and
//! `extensions`; the second one measured extension loading and goes with the
//! extension system (`plans/facts/extension-boundary.md`).
//!
//! Deviation (class 1): the enabled flag is read at first use rather than at
//! module load, because Rust has no import-time side effects. The environment
//! is fixed by then in every path the app takes.

use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The timing namespaces. Only `main` is left; see the module note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingNamespaceLabel {
    Main,
}

impl TimingNamespaceLabel {
    fn as_str(self) -> &'static str {
        match self {
            TimingNamespaceLabel::Main => "main",
        }
    }
}

struct TimingNamespace {
    timings: Vec<(String, i64)>,
    last_time: i64,
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("NOTAGENT_TIMING").as_deref() == Ok("1"))
}

fn namespaces() -> &'static Mutex<Vec<(TimingNamespaceLabel, TimingNamespace)>> {
    static NAMESPACES: OnceLock<Mutex<Vec<(TimingNamespaceLabel, TimingNamespace)>>> =
        OnceLock::new();
    NAMESPACES.get_or_init(|| Mutex::new(Vec::new()))
}

/// `Date.now()` in milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

pub fn reset_timings() {
    reset_timings_in(TimingNamespaceLabel::Main);
}

pub fn reset_timings_in(namespace: TimingNamespaceLabel) {
    if !enabled() {
        return;
    }
    let mut namespaces = namespaces().lock().expect("poisoned");
    let entry = TimingNamespace {
        timings: Vec::new(),
        last_time: now_ms(),
    };
    match namespaces.iter_mut().find(|(label, _)| *label == namespace) {
        Some((_, existing)) => *existing = entry,
        None => namespaces.push((namespace, entry)),
    }
}

pub fn time(label: &str) {
    time_in(label, TimingNamespaceLabel::Main);
}

pub fn time_in(label: &str, namespace: TimingNamespaceLabel) {
    if !enabled() {
        return;
    }
    let now = now_ms();
    let mut namespaces = namespaces().lock().expect("poisoned");
    if !namespaces
        .iter()
        .any(|(existing, _)| *existing == namespace)
    {
        namespaces.push((
            namespace,
            TimingNamespace {
                timings: Vec::new(),
                last_time: now,
            },
        ));
    }
    let (_, timing_namespace) = namespaces
        .iter_mut()
        .find(|(existing, _)| *existing == namespace)
        .expect("just inserted");
    timing_namespace
        .timings
        .push((label.to_owned(), now - timing_namespace.last_time));
    timing_namespace.last_time = now;
}

fn print_timing_group(title: &str, timings: &[(String, i64)]) {
    let printable: Vec<&(String, i64)> = timings.iter().filter(|(_, ms)| *ms >= 0).collect();
    if printable.is_empty() {
        return;
    }
    eprintln!("\n--- {title} ---");
    for (label, ms) in &printable {
        eprintln!("  {label}: {ms}ms");
    }
    let total: i64 = printable.iter().map(|(_, ms)| *ms).sum();
    eprintln!("  TOTAL: {total}ms");
    eprintln!("{}\n", "-".repeat(title.len() + 8));
}

pub fn print_timings() {
    if !enabled() {
        return;
    }
    let namespaces = namespaces().lock().expect("poisoned");
    for (label, timing_namespace) in namespaces.iter() {
        print_timing_group(
            &format!("Startup Timings: {}", label.as_str()),
            &timing_namespace.timings,
        );
    }
}
