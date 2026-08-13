//! Differenztest gegen die TS-Vorlage.
//!
//! `tests/fixtures/utils-oracle.json` enthält die Ausgaben von
//! `packages/tui/src/utils.ts` (via Node) für ein breites Korpus aus ASCII,
//! CJK, Indic, Thai/Lao, Myanmar, RGI-Emoji, ANSI-/OSC-Sequenzen und
//! pseudozufälligen Mischungen. Erzeugt von `tools/gen-utils-oracle.mjs`
//! (Master-Plan, Risiko 1: Fixture-Generierung aus dem TS-Repo).

use notagent_tui::{truncate_to_width, truncate_to_width_opts, visible_width, wrap_text_with_ansi};
use serde_json::Value;

fn oracle() -> Value {
    let raw = include_str!("fixtures/utils-oracle.json");
    serde_json::from_str(raw).expect("fixture is valid JSON")
}

#[test]
fn visible_width_matches_typescript() {
    let data = oracle();
    let mut mismatches = Vec::new();
    for case in data["cases"].as_array().expect("cases array") {
        let input = case["input"].as_str().expect("input string");
        let expected = case["visibleWidth"].as_u64().expect("width") as usize;
        let actual = visible_width(input);
        if actual != expected {
            mismatches.push(format!("{input:?}: TS {expected} vs Rust {actual}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} von {} Fällen weichen ab:\n{}",
        mismatches.len(),
        data["cases"].as_array().expect("cases").len(),
        mismatches
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn wrap_text_with_ansi_matches_typescript() {
    let data = oracle();
    let widths: Vec<usize> = data["widths"]
        .as_array()
        .expect("widths")
        .iter()
        .map(|w| w.as_u64().expect("width") as usize)
        .collect();
    let mut mismatches = Vec::new();
    for case in data["cases"].as_array().expect("cases array") {
        let input = case["input"].as_str().expect("input string");
        for (index, width) in widths.iter().enumerate() {
            let expected: Vec<String> = case["wrapped"][index]
                .as_array()
                .expect("wrapped lines")
                .iter()
                .map(|l| l.as_str().expect("line").to_string())
                .collect();
            let actual = wrap_text_with_ansi(input, *width);
            if actual != expected {
                mismatches.push(format!(
                    "{input:?} @ {width}:\n  TS   {expected:?}\n  Rust {actual:?}"
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} Abweichungen:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn truncate_to_width_matches_typescript() {
    let data = oracle();
    let widths: Vec<usize> = data["widths"]
        .as_array()
        .expect("widths")
        .iter()
        .map(|w| w.as_u64().expect("width") as usize)
        .collect();
    let mut mismatches = Vec::new();
    for case in data["cases"].as_array().expect("cases array") {
        let input = case["input"].as_str().expect("input string");
        for (index, width) in widths.iter().enumerate() {
            let expected = case["truncated"][index].as_str().expect("truncated");
            let actual = truncate_to_width(input, *width);
            if actual != expected {
                mismatches.push(format!(
                    "{input:?} @ {width}: TS {expected:?} vs Rust {actual:?}"
                ));
            }
            let expected_pad = case["truncatedEllipsisPad"][index]
                .as_str()
                .expect("truncated pad");
            let actual_pad = truncate_to_width_opts(input, *width, "…", true);
            if actual_pad != expected_pad {
                mismatches.push(format!(
                    "{input:?} @ {width} (…, pad): TS {expected_pad:?} vs Rust {actual_pad:?}"
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} Abweichungen:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn slice_with_width_matches_typescript() {
    let data = oracle();
    let configs: Vec<(usize, usize, bool)> = data["slices"]
        .as_array()
        .expect("slices")
        .iter()
        .map(|c| {
            (
                c[0].as_u64().expect("start") as usize,
                c[1].as_u64().expect("len") as usize,
                c[2].as_bool().expect("strict"),
            )
        })
        .collect();
    let mut mismatches = Vec::new();
    for case in data["cases"].as_array().expect("cases array") {
        let input = case["input"].as_str().expect("input string");
        for (index, (start, len, strict)) in configs.iter().enumerate() {
            let expected_text = case["slices"][index]["text"].as_str().expect("text");
            let expected_width = case["slices"][index]["width"].as_u64().expect("width") as usize;
            let actual = notagent_tui::slice_with_width(input, *start, *len, *strict);
            if actual.text != expected_text || actual.width != expected_width {
                mismatches.push(format!(
                    "{input:?} @ ({start},{len},{strict}):\n  TS   {expected_text:?}/{expected_width}\n  Rust {:?}/{}",
                    actual.text, actual.width
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} Abweichungen:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn extract_segments_matches_typescript() {
    let data = oracle();
    let configs: Vec<(usize, usize, usize, bool)> = data["segments"]
        .as_array()
        .expect("segments")
        .iter()
        .map(|c| {
            (
                c[0].as_u64().expect("beforeEnd") as usize,
                c[1].as_u64().expect("afterStart") as usize,
                c[2].as_u64().expect("afterLen") as usize,
                c[3].as_bool().expect("strictAfter"),
            )
        })
        .collect();
    let mut mismatches = Vec::new();
    for case in data["cases"].as_array().expect("cases array") {
        let input = case["input"].as_str().expect("input string");
        for (index, (before_end, after_start, after_len, strict)) in configs.iter().enumerate() {
            let expected = &case["segments"][index];
            let actual = notagent_tui::extract_segments(
                input,
                *before_end,
                *after_start,
                *after_len,
                *strict,
            );
            let ok = actual.before == expected["before"].as_str().expect("before")
                && actual.before_width == expected["beforeWidth"].as_u64().expect("bw") as usize
                && actual.after == expected["after"].as_str().expect("after")
                && actual.after_width == expected["afterWidth"].as_u64().expect("aw") as usize;
            if !ok {
                mismatches.push(format!(
                    "{input:?} @ ({before_end},{after_start},{after_len},{strict}):\n  TS   {:?}/{} {:?}/{}\n  Rust {:?}/{} {:?}/{}",
                    expected["before"].as_str().expect("before"),
                    expected["beforeWidth"],
                    expected["after"].as_str().expect("after"),
                    expected["afterWidth"],
                    actual.before,
                    actual.before_width,
                    actual.after,
                    actual.after_width
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} Abweichungen:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
