//! Ports of `packages/coding-agent/test/export-html-xss.test.ts` (67 LOC),
//! `export-html-whitespace.test.ts` (42) and `export-html-skill-block.test.ts`
//! (40), plus a round trip through `export_from_file`.
//!
//! The three TypeScript suites read `template.js`/`template.css` from disk and
//! assert on their text; the Rust binary embeds the same files, so the suites
//! assert on the embedded constants. The one case that drives a TUI component
//! through `createToolHtmlRenderer` uses the pure line-trimming half that stayed
//! in this crate (see `core/export_html/tool_renderer.rs`).

use notagent::core::export_html::ansi_to_html::{ansi_lines_to_html, ansi_to_html};
use notagent::core::export_html::tool_renderer::rendered_result_from_lines;
use notagent::core::export_html::{ExportOptions, TEMPLATE_CSS, TEMPLATE_JS, export_from_file};
use regex::Regex;

fn matches(haystack: &str, pattern: &str) -> bool {
    Regex::new(pattern).expect("pattern").is_match(haystack)
}

// ---------------------------------------------------------------------------
// export HTML markdown link sanitization
// ---------------------------------------------------------------------------

#[test]
fn overrides_the_marked_link_renderer_to_use_scheme_allow_list_sanitization() {
    assert!(matches(TEMPLATE_JS, r"link\s*\(\s*token\s*\)"));
    assert!(TEMPLATE_JS.contains("sanitizeMarkdownUrl(token.href)"));
    assert!(TEMPLATE_JS.contains("^(https?|mailto|tel|ftp)"));
}

#[test]
fn overrides_the_marked_image_renderer_to_use_scheme_allow_list_sanitization() {
    assert!(matches(TEMPLATE_JS, r"image\s*\(\s*token\s*\)"));
    assert!(TEMPLATE_JS.contains("sanitizeMarkdownUrl(token.href)"));
}

#[test]
fn strips_c0_controls_before_checking_and_emitting_markdown_urls() {
    assert!(TEMPLATE_JS.contains(r"replace(/[\x00-\x1f\x7f]/g, '')"));
    assert!(!matches(
        TEMPLATE_JS,
        r"(?i)\^\\s\*\(javascript\|vbscript\|data\):"
    ));
}

#[test]
fn escapes_href_attributes_in_the_custom_link_renderer() {
    // The link renderer must escape href values to prevent attribute breakout
    assert!(TEMPLATE_JS.contains("escapeHtml(href)"));
}

#[test]
fn escapes_image_mime_type_attributes() {
    // Image mimeType must be escaped to prevent attribute breakout
    assert!(!TEMPLATE_JS.contains("${img.mimeType}"));
    assert!(TEMPLATE_JS.contains("escapeHtml(img.mimeType"));
}

#[test]
fn escapes_image_data_attributes() {
    // Image data is embedded in src attributes and must not allow attribute breakout.
    assert!(!TEMPLATE_JS.contains(";base64,${img.data}\""));
    assert!(matches(
        TEMPLATE_JS,
        r#";base64,\$\{escapeHtml\(img\.data \|\| (?:''|"")\)\}""#
    ));
}

#[test]
fn escapes_entry_ids_before_inserting_them_into_attributes() {
    // Session entry IDs are embedded in id and data-entry-id attributes.
    assert!(!TEMPLATE_JS.contains("id=\"${entryId}\""));
    assert!(!TEMPLATE_JS.contains("data-entry-id=\"${entryId}\""));
    assert!(TEMPLATE_JS.contains("entry-${escapeHtml(entry.id)}"));
    assert!(TEMPLATE_JS.contains("data-entry-id=\"${escapeHtml(entryId)}\""));
}

#[test]
fn escapes_tree_metadata_rendered_from_session_fields() {
    // The tree renders session metadata via innerHTML, so dynamic fields must be escaped.
    assert!(!TEMPLATE_JS.contains("[${msg.toolName || 'tool'}]"));
    assert!(!TEMPLATE_JS.contains("[${msg.role}]"));
    assert!(!TEMPLATE_JS.contains("[model: ${entry.modelId}]"));
    assert!(!TEMPLATE_JS.contains("[thinking: ${entry.thinkingLevel}]"));
    assert!(!TEMPLATE_JS.contains("[${entry.type}]"));
    assert!(TEMPLATE_JS.contains("${escapeHtml(msg.toolName || 'tool')}"));
    assert!(TEMPLATE_JS.contains("${escapeHtml(msg.role)}"));
    assert!(TEMPLATE_JS.contains("${escapeHtml(entry.modelId)}"));
    assert!(TEMPLATE_JS.contains("${escapeHtml(entry.thinkingLevel)}"));
    assert!(TEMPLATE_JS.contains("${escapeHtml(entry.type)}"));
}

#[test]
fn escapes_model_names_in_the_exported_header() {
    // Assistant message provider/model values are collected from the session and
    // rendered with innerHTML.
    assert!(!TEMPLATE_JS.contains("${globalStats.models.join(', ') || 'unknown'}"));
    assert!(TEMPLATE_JS.contains("${escapeHtml(globalStats.models.join(', ') || 'unknown')}"));
}

// ---------------------------------------------------------------------------
// export HTML tool output whitespace
// ---------------------------------------------------------------------------

#[test]
fn preserves_whitespace_for_plain_text_tool_output_lines() {
    assert!(matches(
        TEMPLATE_CSS,
        r"(?s)\.output-preview > div:not\(\.expand-hint\),\s*\.output-full > div:not\(\.expand-hint\) \{.*?white-space:\s*pre-wrap;"
    ));
    assert!(matches(
        TEMPLATE_CSS,
        r"(?s)\.ansi-line\s*\{.*?white-space:\s*pre;"
    ));
    assert!(!matches(
        TEMPLATE_CSS,
        r"(?s)\.output-preview,\s*\.output-full\s*\{.*?white-space:\s*pre-wrap;"
    ));
}

#[test]
fn does_not_insert_source_whitespace_between_ansi_rendered_lines() {
    assert_eq!(
        ansi_lines_to_html(&["one", "two"]),
        r#"<div class="ansi-line">one</div><div class="ansi-line">two</div>"#
    );
}

#[test]
fn trims_tui_spacing_lines_from_custom_tool_result_html() {
    let lines = ["", "\u{1b}[31mone\u{1b}[0m", "two", ""];
    assert_eq!(
        rendered_result_from_lines(&lines, &lines)
            .expanded
            .as_deref(),
        Some(
            r#"<div class="ansi-line"><span style="color:#800000">one</span></div><div class="ansi-line">two</div>"#
        )
    );
}

// ---------------------------------------------------------------------------
// export HTML skill block rendering
// ---------------------------------------------------------------------------

#[test]
fn strips_skill_wrapper_xml_from_user_message_rendering() {
    // Skill commands store a structural wrapper in the raw user message:
    //   <skill name="..." location="...">\n...\n</skill>\n\nactual prompt
    // The export renderer must detect that wrapper and render only the
    // user-visible prompt, not the Notagent-generated <skill>...</skill> XML tags.
    assert!(TEMPLATE_JS.contains("parseSkillBlock"));
    assert!(TEMPLATE_JS.contains("skillBlock.userMessage"));
}

#[test]
fn renders_skill_invocation_and_user_message_as_separate_sibling_blocks() {
    assert!(TEMPLATE_JS.contains("skill-invocation"));
    assert!(TEMPLATE_JS.contains("hasUserContent"));
}

#[test]
fn renders_skill_content_as_markdown_not_raw_text() {
    assert!(TEMPLATE_JS.contains("safeMarkedParse(skillBlock.content)"));
}

#[test]
fn shows_skill_name_and_user_message_in_the_sidebar_tree() {
    assert!(TEMPLATE_JS.contains("tree-role-skill"));
}

// ---------------------------------------------------------------------------
// ANSI conversion and the export round trip
// ---------------------------------------------------------------------------

#[test]
fn converts_the_ansi_style_vocabulary() {
    assert_eq!(ansi_to_html("plain"), "plain");
    assert_eq!(
        ansi_to_html("\u{1b}[1;4;92mloud\u{1b}[0m"),
        r#"<span style="color:#00ff00;font-weight:bold;text-decoration:underline">loud</span>"#
    );
    assert_eq!(
        ansi_to_html("\u{1b}[38;5;196mcube\u{1b}[39m"),
        r#"<span style="color:#ff0000">cube</span>"#
    );
    assert_eq!(
        ansi_to_html("\u{1b}[38;2;1;2;3mtrue\u{1b}[0m"),
        r#"<span style="color:rgb(1,2,3)">true</span>"#
    );
    assert_eq!(
        ansi_to_html("\u{1b}[48;5;232mgrey\u{1b}[0m"),
        r#"<span style="background-color:#080808">grey</span>"#
    );
    assert_eq!(ansi_to_html("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&#039;");
    assert_eq!(
        ansi_lines_to_html(&[""]),
        r#"<div class="ansi-line">&nbsp;</div>"#
    );
}

#[test]
fn exports_a_session_file_to_a_self_contained_page() {
    let directory = tempfile::tempdir().expect("tempdir");
    let session_path = directory.path().join("session.jsonl");
    std::fs::write(
        &session_path,
        concat!(
            r#"{"type":"session","version":3,"id":"session-1","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp"}"#,
            "\n",
            r#"{"type":"message","id":"e1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":"hello","timestamp":1}}"#,
            "\n"
        ),
    )
    .expect("write session");
    let output_path = directory.path().join("export.html");

    let written = export_from_file(
        session_path.to_str().expect("path"),
        ExportOptions {
            output_path: Some(output_path.to_string_lossy().into_owned()),
            theme_name: Some("dark".to_owned()),
            tool_renderer: None,
        },
    )
    .expect("export");
    assert_eq!(written, output_path.to_string_lossy());

    let html = std::fs::read_to_string(&output_path).expect("read export");
    assert!(html.starts_with("<!DOCTYPE html>"));
    // No placeholder survives, and the vendored libraries are inlined.
    for placeholder in [
        "{{CSS}}",
        "{{JS}}",
        "{{SESSION_DATA}}",
        "{{MARKED_JS}}",
        "{{THEME_VARS}}",
        "{{BODY_BG}}",
        "{{CONTAINER_BG}}",
        "{{INFO_BG}}",
    ] {
        assert!(!html.contains(placeholder), "{placeholder} survived");
    }
    assert!(html.contains("--exportPageBg:"));

    // The session payload round-trips through base64.
    let payload = html
        .split_once(r#"<script id="session-data" type="application/json">"#)
        .expect("payload")
        .1
        .split_once("</script>")
        .expect("payload end")
        .0;
    let decoded = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .expect("base64")
    };
    let session_data: serde_json::Value =
        serde_json::from_slice(&decoded).expect("session data json");
    assert_eq!(session_data["header"]["id"], "session-1");
    assert_eq!(session_data["entries"][0]["message"]["content"], "hello");
    assert_eq!(session_data["leafId"], "e1");
    assert_eq!(session_data.get("systemPrompt"), None);
    assert_eq!(session_data.get("tools"), None);
    assert_eq!(session_data.get("renderedTools"), None);
}

#[test]
fn reproduces_the_dollar_expansion_of_string_replace() {
    // `String.prototype.replace` expands `$&` and `$$` in the replacement, so the
    // vendored highlight.js and the template arrive in the export mangled. The
    // port keeps that (bug compatibility) — these two markers prove it.
    let directory = tempfile::tempdir().expect("tempdir");
    let session_path = directory.path().join("session.jsonl");
    std::fs::write(
        &session_path,
        concat!(
            r#"{"type":"session","version":3,"id":"session-2","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp"}"#,
            "\n"
        ),
    )
    .expect("write session");
    let output_path = directory.path().join("export.html");
    export_from_file(
        session_path.to_str().expect("path"),
        ExportOptions {
            output_path: Some(output_path.to_string_lossy().into_owned()),
            theme_name: Some("dark".to_owned()),
            tool_renderer: None,
        },
    )
    .expect("export");
    let html = std::fs::read_to_string(&output_path).expect("read export");

    // `[-+*\/?!$&|:<=>@^~]` of the R grammar takes the matched placeholder.
    assert!(html.contains(r"[-+*\/?!{{HIGHLIGHT_JS}}|:<=>@^~]"));
    // `$${totalCost.toFixed(3)}` of the cost label loses its literal dollar.
    assert!(TEMPLATE_JS.contains("$${totalCost.toFixed(3)}"));
    assert!(html.contains("${totalCost.toFixed(3)}"));
    assert!(!html.contains("$${totalCost.toFixed(3)}"));
}
