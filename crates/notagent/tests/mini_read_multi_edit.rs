use std::path::Path;

use notagent::core::mini_read::{apply_minified_edit, minify_for_path};

#[test]
fn inplace_multiline_edit_preserves_original_blank_lines() {
    // The model edits from the blank-less minified view, so its new_string has
    // no blank lines. An in-place edit (same number of non-blank lines) must
    // still keep the file's original blank lines.
    let source =
        "fn n() {\n    let a = first();\n\n    let b = second();\n\n    let c = third();\n}\n";
    let old = "let a = first();\nlet b = second();\nlet c = third();";
    let new = "let a = first();\nlet b = SECOND();\nlet c = third();";
    let actual = apply_minified_edit(Path::new("m.rs"), source, old, new, false, false)
        .expect("language should be supported")
        .expect("edit should apply");
    let expected =
        "fn n() {\n    let a = first();\n\n    let b = SECOND();\n\n    let c = third();\n}\n";
    assert_eq!(actual.content, expected);
}

#[test]
fn multiline_edit_preserves_blank_lines_from_new_string() {
    // A multi-line replacement spans a blank line in the source. The blank the
    // caller keeps in `new_string` must survive (it was previously dropped when
    // the replacement got minified).
    let source = "fn f() {\n    let a = compute();\n\n    let b = other();\n}\n";
    let actual = apply_minified_edit(
        Path::new("m.rs"),
        source,
        "let a = compute();\nlet b = other();",
        "let a = compute();\n\nlet b = other2();",
        false,
        false,
    )
    .expect("language should be supported")
    .expect("edit should apply");
    let expected = "fn f() {\n    let a = compute();\n\n    let b = other2();\n}\n";
    assert_eq!(actual.content, expected);
}

#[test]
fn single_line_edit_keeps_following_blank_line() {
    // A single-line edit must never swallow the blank line after it.
    let source = "fn f() {\n    let a = compute();\n\n    let b = other();\n}\n";
    let actual = apply_minified_edit(
        Path::new("m.rs"),
        source,
        "let a = compute();",
        "let a = compute2();",
        false,
        false,
    )
    .expect("language should be supported")
    .expect("edit should apply");
    let expected = "fn f() {\n    let a = compute2();\n\n    let b = other();\n}\n";
    assert_eq!(actual.content, expected);
}

/// Applies `edits` (`old`, `new`, `keep_comments`) in order, threading each
/// result into the next, exactly like `multi_patch_minified`. Panics with a
/// helpful message if any edit fails to match.
fn apply_edits(name: &str, source: &str, edits: &[(&str, &str, bool)]) -> String {
    let mut content = source.to_owned();
    for (index, (old, new, keep)) in edits.iter().enumerate() {
        let result = apply_minified_edit(Path::new(name), &content, old, new, false, *keep)
            .expect("language should be supported");
        match result {
            Ok(edit) => content = edit.content,
            Err(error) => panic!("edit #{index} ({old:?}) failed: {error}"),
        }
    }
    content
}

/// Every non-blank line must be indented with spaces in multiples of the
/// file's 4-space unit (no stray column-0 collapse, no half-levels) and must
/// not contain leaked control/OSC fragments.
fn assert_clean_indentation(content: &str) {
    for (line_no, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        assert!(
            !line.contains('\t'),
            "line {line_no} unexpectedly contains a tab: {line:?}"
        );
        let indent = line.bytes().take_while(|&byte| byte == b' ').count();
        assert_eq!(
            indent % 4,
            0,
            "line {line_no} has a non-4-multiple indent ({indent}): {line:?}"
        );
        assert!(
            !line.contains('\u{1b}') && !line.contains('\u{7}'),
            "line {line_no} contains a leaked escape/APC fragment: {line:?}"
        );
    }
}

/// Re-minifying must still succeed (the file parses) after all edits.
fn assert_still_parses(name: &str, content: &str) {
    assert!(
        minify_for_path(Path::new(name), content, true).is_some(),
        "edited content no longer parses"
    );
}

const FIXTURE: &str = "\
fn handle(input: &str) -> Result<String> {
    let parsed = parse(input)
        .map_err(|e| {
            // log the parse failure for diagnostics
            tracing::warn!(\"parse failed: {e}\");
            Error::Parse(e)
        })?;
    // normalize before returning
    let out = normalize(parsed);
    Ok(out)
}
";

#[test]
fn multiple_sequential_edits_stay_clean() {
    // A realistic mix: a deeply nested comment (keep on), nested code lines
    // (keep off), a comment-only edit that must auto-retry (keep off), and a
    // top-level edit — all in sequence on one file.
    let edits: &[(&str, &str, bool)] = &[
        (
            "// log the parse failure for diagnostics",
            "// record the parse failure for diagnostics",
            true,
        ),
        (
            "tracing::warn!(\"parse failed: {e}\");",
            "tracing::error!(\"parse failed: {e}\");",
            false,
        ),
        ("Error::Parse(e)", "Error::BadInput(e)", false),
        (
            "// normalize before returning",
            "// normalize the parsed value before returning",
            false,
        ),
        (
            "let out = normalize(parsed);",
            "let out = normalize(parsed)?;",
            false,
        ),
    ];

    let actual = apply_edits("main.rs", FIXTURE, edits);
    let expected = "\
fn handle(input: &str) -> Result<String> {
    let parsed = parse(input)
        .map_err(|e| {
            // record the parse failure for diagnostics
            tracing::error!(\"parse failed: {e}\");
            Error::BadInput(e)
        })?;
    // normalize the parsed value before returning
    let out = normalize(parsed)?;
    Ok(out)
}
";
    assert_eq!(actual, expected);
    assert_clean_indentation(&actual);
    assert_still_parses("main.rs", &actual);
}

#[test]
fn repeated_comment_edits_never_collapse_indentation() {
    // Edit the same nested comment three times in a row, each time copying the
    // current comment verbatim out of the keep_comments view (so the match
    // starts at the line beginning — the indentation-corruption-prone path).
    let mut content = FIXTURE.to_owned();
    // Track the unique text of the comment as it changes, so we always grab the
    // right line out of the view.
    let mut marker = "parse failure for diagnostics".to_owned();
    for replacement in [
        "// note revision one",
        "// note revision two is a bit longer",
        "// n3",
    ] {
        let view = minify_for_path(Path::new("main.rs"), &content, true).expect("supported");
        let old = view
            .lines()
            .find(|line| line.contains(&marker))
            .expect("comment present in view")
            .to_owned();
        let indent: String = old
            .chars()
            .take_while(|&character| character == ' ')
            .collect();
        let new = format!("{indent}{replacement}");
        content = apply_edits("main.rs", &content, &[(old.as_str(), new.as_str(), true)]);
        assert_clean_indentation(&content);
        marker = replacement.trim_start_matches("// ").to_owned();
    }

    // The thrice-edited comment must still sit at its original depth (12 spaces).
    assert!(
        content.contains("            // n3"),
        "final comment lost its indentation:\n{content}"
    );
    assert_still_parses("main.rs", &content);
}

#[test]
fn sequential_edits_in_python_stay_clean() {
    // Indentation-significant language: a comment-only edit (auto-retry) plus a
    // nested code edit must keep the suite indentation intact.
    let fixture = "\
def handle(value):
    if value > 0:
        # positive branch
        result = compute(value)
        return result
    return 0
";
    let edits: &[(&str, &str, bool)] = &[
        ("# positive branch", "# handle the positive branch", false),
        (
            "result = compute(value)",
            "result = compute(value) * 2",
            false,
        ),
    ];
    let actual = apply_edits("main.py", fixture, edits);
    let expected = "\
def handle(value):
    if value > 0:
        # handle the positive branch
        result = compute(value) * 2
        return result
    return 0
";
    assert_eq!(actual, expected);
    assert_clean_indentation(&actual);
    assert_still_parses("main.py", &actual);
}

// ---------------------------------------------------------------------------
// The testbench (`minified-edit-testbench/PROMPT.md`) as a deterministic run.
// ---------------------------------------------------------------------------

const EDGE_CASES: &str = include_str!("fixtures/minify/edge_cases.rs");

/// The line of the minified view that carries `marker`, as a model copies it.
fn view_line(content: &str, marker: &str, keep_comments: bool) -> String {
    let view = minify_for_path(Path::new("edge_cases.rs"), content, keep_comments)
        .expect("language should be supported");
    view.lines()
        .find(|line| line.contains(marker))
        .unwrap_or_else(|| panic!("no view line contains {marker:?}"))
        .to_owned()
}

/// One edit of the benchmark: take the view line holding `marker` and swap
/// `from` for `to` inside it, which is what the prompt asks the model to do.
/// The search text is the line without its view indentation — the form a model
/// sends when it quotes the line it wants changed. Sending it *with* the view
/// indentation is only equivalent for lines the view re-indents: a `///` doc
/// comment is copied verbatim instead, because tree-sitter-rust puts the
/// trailing newline inside the comment node, which makes the line a multiline
/// literal for the renderer. Its view indentation is therefore raw columns, and
/// feeding those back in as a minified depth over-indents the result. That is
/// reference behaviour (`minify_edit.rs`), preserved here rather than fixed.
fn edit_view_line(
    content: &str,
    marker: &str,
    from: &str,
    to: &str,
    keep_comments: bool,
) -> String {
    let line = view_line(content, marker, keep_comments);
    let old = line.trim_start_matches(' ').to_owned();
    assert!(
        old.contains(from),
        "view line {old:?} does not contain {from:?}"
    );
    let new = old.replacen(from, to, 1);
    apply_edits("edge_cases.rs", content, &[(&old, &new, keep_comments)])
}

#[test]
fn testbench_edits_keep_indentation_strings_and_comments() {
    let mut content = EDGE_CASES.to_owned();

    // 1-2: doc comments at column 0 and at one level of nesting.
    content = edit_view_line(
        &content,
        "EDIT TARGET A",
        "Severity levels for log records.",
        "Severity levels attached to log records.",
        true,
    );
    content = edit_view_line(
        &content,
        "EDIT TARGET B",
        "Retry budget; `0` disables retries.",
        "Retry budget; `0` means no retries.",
        true,
    );
    // 3: a trailing inline comment on a code line.
    content = edit_view_line(
        &content,
        "EDIT TARGET C",
        "// trailing inline comment on a field — EDIT TARGET C",
        "// default retry budget",
        true,
    );
    // 4: the second line of a comment block at depth 3 (12 columns).
    content = edit_view_line(
        &content,
        "EDIT TARGET D",
        "second line of the same nested comment block — EDIT TARGET D",
        "second line of the nested comment, now updated",
        true,
    );
    // 5: a comment-only line, edited with comments hidden — this is the case
    // that has to auto-enable keep_comments.
    content = apply_edits(
        "edge_cases.rs",
        &content,
        &[(
            "// guard against empty results — EDIT TARGET E (comment-only)",
            "// reject empty results early",
            false,
        )],
    );
    // 6: a comment behind a match arm.
    content = edit_view_line(
        &content,
        "EDIT TARGET F",
        "// loud — EDIT TARGET F",
        "// high priority",
        true,
    );
    // 7: a comment nested four levels deep in control flow.
    content = edit_view_line(
        &content,
        "EDIT TARGET G",
        "// only positive values contribute — EDIT TARGET G",
        "// accumulate positive values",
        true,
    );
    // 8: a comment with non-ASCII text — the byte-indexed source map has to
    // survive multi-byte characters on both sides of the edit.
    content = edit_view_line(
        &content,
        "EDIT TARGET H",
        "// unicode in a comment: café — façade — 日本語 — EDIT TARGET H",
        "// unicode: смысл — naïve — 漢字",
        true,
    );
    // 9: an ordinary code edit, with comments hidden.
    content = apply_edits(
        "edge_cases.rs",
        &content,
        &[(
            "self.tags.push(tag.to_string());",
            "self.tags.push(tag.trim().to_string());",
            false,
        )],
    );
    // 10: a comment right above string literals that look like code.
    content = edit_view_line(
        &content,
        "must survive verbatim",
        "// the strings below contain // and {} and ; and must survive verbatim",
        "// these strings must survive byte-for-byte",
        true,
    );

    // The benchmark's grading rule: every line keeps the indentation it had.
    // (`assert_clean_indentation` does not fit this file — its block comments
    // are continued with ` * `, a deliberate 5-column indent.)
    for (line_no, (before, after)) in EDGE_CASES.lines().zip(content.lines()).enumerate() {
        let indent = |line: &str| line.bytes().take_while(|&byte| byte == b' ').count();
        assert_eq!(
            indent(before),
            indent(after),
            "line {line_no} changed its indentation: {before:?} -> {after:?}"
        );
        assert!(
            !after.contains('\t') && !after.contains('\u{1b}') && !after.contains('\u{7}'),
            "line {line_no} contains a tab or a leaked escape fragment: {after:?}"
        );
    }
    assert_still_parses("edge_cases.rs", &content);

    // Every string literal of the benchmark stays byte-identical.
    for literal in [
        "\n        line one of a raw string  // this is NOT a comment, do not touch\n        line two with { braces } and ; semicolons inside the string\n    ",
        "first line\\n    second line indented inside the string\\nthird line",
        "\"// looks like a comment but is a string\".to_string(),",
        "\"fn fake() { return 0; }\".to_string(),",
        "format!(\"interpolated value is {}\", 1 + 1),",
        "Config::new(\"café\").with_tag(\"ünïcödé\")",
    ] {
        assert!(
            content.contains(literal),
            "string literal changed: {literal}"
        );
    }

    // The ten requested edits landed, and nothing else moved.
    for expected in [
        "/// Severity levels attached to log records. (doc comment — EDIT TARGET A)",
        "    /// Retry budget; `0` means no retries. (doc comment — EDIT TARGET B)",
        "            retries: 3, // default retry budget",
        "            // second line of the nested comment, now updated",
        "        // reject empty results early",
        "        Severity::Error => \"error\", // high priority",
        "                // accumulate positive values",
        "        // unicode: смысл — naïve — 漢字",
        "        self.tags.push(tag.trim().to_string());",
        "    // these strings must survive byte-for-byte",
    ] {
        assert!(content.contains(expected), "missing edit: {expected}");
    }
    let changed = EDGE_CASES
        .lines()
        .zip(content.lines())
        .filter(|(before, after)| before != after)
        .count();
    assert_eq!(changed, 10, "exactly the ten requested lines change");
    assert_eq!(
        EDGE_CASES.lines().count(),
        content.lines().count(),
        "no lines added or removed"
    );
}
