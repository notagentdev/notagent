use notagent::core::prompt_templates::{
    LoadPromptTemplatesOptions, PromptTemplate, expand_prompt_template, load_prompt_templates,
    parse_command_args, substitute_args,
};
use notagent::core::source_info::{
    SourceOrigin, SourceScope, SyntheticSourceInfoOptions, create_synthetic_source_info,
};

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn substitute(content: &str, values: &[&str]) -> String {
    substitute_args(content, &args(values))
}

// ------------------------------------------------------------- substituteArgs

#[test]
fn replaces_arguments_and_at_with_every_argument() {
    assert_eq!(
        substitute("Test: $ARGUMENTS", &["a", "b", "c"]),
        "Test: a b c"
    );
    assert_eq!(substitute("Test: $@", &["a", "b", "c"]), "Test: a b c");
    assert_eq!(
        substitute("Test: $@", &["foo", "bar", "baz"]),
        substitute("Test: $ARGUMENTS", &["foo", "bar", "baz"])
    );
}

/// The critical one: an argument that looks like a placeholder stays literal.
#[test]
fn never_substitutes_recursively_into_argument_values() {
    assert_eq!(
        substitute("$ARGUMENTS", &["$1", "$ARGUMENTS"]),
        "$1 $ARGUMENTS"
    );
    assert_eq!(substitute("$@", &["$100", "$1"]), "$100 $1");
    assert_eq!(substitute("$ARGUMENTS", &["$100", "$1"]), "$100 $1");
}

#[test]
fn mixes_positional_and_wildcard_placeholders() {
    assert_eq!(
        substitute("$1: $ARGUMENTS", &["prefix", "a", "b"]),
        "prefix: prefix a b"
    );
    assert_eq!(
        substitute("$1: $@", &["prefix", "a", "b"]),
        "prefix: prefix a b"
    );
}

#[test]
fn handles_an_empty_argument_list() {
    assert_eq!(substitute("Test: $ARGUMENTS", &[]), "Test: ");
    assert_eq!(substitute("Test: $@", &[]), "Test: ");
    assert_eq!(substitute("Test: $1", &[]), "Test: ");
}

#[test]
fn handles_repeated_placeholders() {
    assert_eq!(
        substitute("$ARGUMENTS and $ARGUMENTS", &["a", "b"]),
        "a b and a b"
    );
    assert_eq!(substitute("$@ and $@", &["a", "b"]), "a b and a b");
    assert_eq!(substitute("$@ and $ARGUMENTS", &["a", "b"]), "a b and a b");
}

#[test]
fn handles_special_characters_in_arguments() {
    assert_eq!(
        substitute("$1 $2: $ARGUMENTS", &["arg100", "@user"]),
        "arg100 @user: arg100 @user"
    );
}

#[test]
fn turns_out_of_range_placeholders_into_empty_strings() {
    assert_eq!(substitute("$1 $2 $3 $4 $5", &["a", "b"]), "a b   ");
}

#[test]
fn handles_unicode_arguments() {
    assert_eq!(
        substitute("$ARGUMENTS", &["日本語", "🎉", "café"]),
        "日本語 🎉 café"
    );
}

#[test]
fn preserves_newlines_and_tabs_in_argument_values() {
    assert_eq!(
        substitute("$1 $2", &["line1\nline2", "tab\tthere"]),
        "line1\nline2 tab\tthere"
    );
}

#[test]
fn handles_consecutive_dollar_patterns() {
    assert_eq!(substitute("$1$2", &["a", "b"]), "ab");
}

#[test]
fn handles_quoted_arguments_with_spaces() {
    assert_eq!(
        substitute("$ARGUMENTS", &["first arg", "second arg"]),
        "first arg second arg"
    );
}

#[test]
fn handles_a_single_argument() {
    assert_eq!(substitute("Test: $ARGUMENTS", &["only"]), "Test: only");
    assert_eq!(substitute("Test: $@", &["only"]), "Test: only");
}

#[test]
fn treats_dollar_zero_as_out_of_range() {
    assert_eq!(substitute("$0", &["a", "b"]), "");
}

#[test]
fn matches_only_the_integer_part_of_a_decimal() {
    assert_eq!(substitute("$1.5", &["a"]), "a.5");
}

#[test]
fn substitutes_placeholders_inside_words() {
    assert_eq!(substitute("pre$ARGUMENTS", &["a", "b"]), "prea b");
    assert_eq!(substitute("pre$@", &["a", "b"]), "prea b");
}

#[test]
fn keeps_empty_and_padded_arguments_as_written() {
    assert_eq!(substitute("$ARGUMENTS", &["a", "", "c"]), "a  c");
    assert_eq!(
        substitute("$ARGUMENTS", &["  leading  ", "trailing  "]),
        "  leading   trailing  "
    );
}

#[test]
fn leaves_partial_and_unknown_patterns_alone() {
    assert_eq!(
        substitute("Prefix $ARGUMENTS suffix", &["ARGUMENTS"]),
        "Prefix ARGUMENTS suffix"
    );
    assert_eq!(substitute("$A $$ $ $ARGS", &["a"]), "$A $$ $ $ARGS");
    assert_eq!(
        substitute("$arguments $Arguments $ARGUMENTS", &["a", "b"]),
        "$arguments $Arguments a b"
    );
}

#[test]
fn treats_the_two_wildcard_spellings_identically() {
    let values = ["x", "y", "z"];
    let first = substitute("$@ and $ARGUMENTS", &values);
    let second = substitute("$ARGUMENTS and $@", &values);
    assert_eq!(first, second);
    assert_eq!(first, "x y z and x y z");
}

#[test]
fn handles_very_long_argument_lists() {
    let values: Vec<String> = (0..100).map(|index| format!("arg{index}")).collect();
    assert_eq!(substitute_args("$ARGUMENTS", &values), values.join(" "));
}

#[test]
fn handles_multi_digit_placeholders() {
    assert_eq!(substitute("$1 $2 $3", &["a", "b", "c"]), "a b c");
    let values: Vec<String> = (0..15).map(|index| format!("val{index}")).collect();
    assert_eq!(substitute_args("$10 $12 $15", &values), "val9 val11 val14");
}

/// There is no escape mechanism: the backslash is literal and `$100` is the
/// hundredth argument.
#[test]
fn has_no_escape_mechanism_for_dollar_signs() {
    assert_eq!(substitute("Price: \\$100", &[]), "Price: \\");
}

#[test]
fn combines_numbered_and_wildcard_placeholders() {
    assert_eq!(
        substitute("$1: $@ ($ARGUMENTS)", &["first", "second", "third"]),
        "first: first second third (first second third)"
    );
    assert_eq!(
        substitute("Just plain text", &["a", "b"]),
        "Just plain text"
    );
    assert_eq!(substitute("$1 $2 $@", &["a", "b", "c"]), "a b a b c");
}

// -------------------------------------------------- substituteArgs: defaults

#[test]
fn uses_a_default_when_the_positional_argument_is_missing() {
    assert_eq!(
        substitute("List exactly ${1:-7} next steps", &[]),
        "List exactly 7 next steps"
    );
    assert_eq!(
        substitute("List exactly ${1:-7} next steps", &["3"]),
        "List exactly 3 next steps"
    );
}

#[test]
fn supports_defaults_for_all_arguments() {
    let template = "${@:-default}\n${ARGUMENTS:-default}";
    assert_eq!(substitute(template, &[]), "default\ndefault");
    assert_eq!(
        substitute(template, &["This", "would", "be", "the", "arguments"]),
        "This would be the arguments\nThis would be the arguments"
    );
}

#[test]
fn uses_a_default_when_the_positional_argument_is_empty() {
    assert_eq!(substitute("Mode: ${1:-brief}", &[""]), "Mode: brief");
}

#[test]
fn supports_several_positional_defaults() {
    assert_eq!(substitute("${1:-7} ${2:-brief}", &[]), "7 brief");
    assert_eq!(substitute("${1:-7} ${2:-brief}", &["3"]), "3 brief");
    assert_eq!(
        substitute("${1:-7} ${2:-brief}", &["3", "verbose"]),
        "3 verbose"
    );
}

#[test]
fn never_substitutes_recursively_into_defaults_or_their_arguments() {
    assert_eq!(substitute("${1:-7}", &["$ARGUMENTS"]), "$ARGUMENTS");
    assert_eq!(substitute("${1:-7}", &["$1"]), "$1");
    assert_eq!(substitute("${1:-$ARGUMENTS}", &["a", "b"]), "a");
    assert_eq!(substitute("${3:-$ARGUMENTS}", &["a", "b"]), "$ARGUMENTS");
}

#[test]
fn supports_defaults_with_spaces_and_out_of_range_positions() {
    assert_eq!(substitute("${1:-seven steps}", &[]), "seven steps");
    assert_eq!(substitute("${3:-fallback}", &["a", "b"]), "fallback");
    assert_eq!(substitute("$1 ${2:-x} $ARGUMENTS", &["a"]), "a x a");
}

// --------------------------------------------------- substituteArgs: slicing

#[test]
fn slices_from_an_index() {
    assert_eq!(substitute("${@:2}", &["a", "b", "c", "d"]), "b c d");
    assert_eq!(substitute("${@:1}", &["a", "b", "c"]), "a b c");
    assert_eq!(substitute("${@:3}", &["a", "b", "c", "d"]), "c d");
}

#[test]
fn slices_with_a_length() {
    assert_eq!(substitute("${@:2:2}", &["a", "b", "c", "d"]), "b c");
    assert_eq!(substitute("${@:1:1}", &["a", "b", "c"]), "a");
    assert_eq!(substitute("${@:3:1}", &["a", "b", "c", "d"]), "c");
    assert_eq!(substitute("${@:2:3}", &["a", "b", "c", "d", "e"]), "b c d");
}

#[test]
fn handles_out_of_range_and_zero_length_slices() {
    assert_eq!(substitute("${@:99}", &["a", "b"]), "");
    assert_eq!(substitute("${@:5}", &["a", "b"]), "");
    assert_eq!(substitute("${@:10:5}", &["a", "b"]), "");
    assert_eq!(substitute("${@:2:0}", &["a", "b", "c"]), "");
    assert_eq!(substitute("${@:1:0}", &["a", "b"]), "");
}

#[test]
fn clamps_a_length_that_exceeds_the_argument_list() {
    assert_eq!(substitute("${@:2:99}", &["a", "b", "c"]), "b c");
    assert_eq!(substitute("${@:1:10}", &["a", "b"]), "a b");
}

#[test]
fn processes_a_slice_before_a_bare_wildcard() {
    assert_eq!(substitute("${@:2} vs $@", &["a", "b", "c"]), "b c vs a b c");
    assert_eq!(
        substitute("First: ${@:1:1}, All: $@", &["x", "y", "z"]),
        "First: x, All: x y z"
    );
}

#[test]
fn never_substitutes_slice_patterns_found_in_arguments() {
    assert_eq!(substitute("${@:1}", &["${@:2}", "test"]), "${@:2} test");
    assert_eq!(substitute("${@:2}", &["a", "${@:3}", "c"]), "${@:3} c");
}

#[test]
fn mixes_slices_with_positional_arguments() {
    assert_eq!(
        substitute("$1: ${@:2}", &["cmd", "arg1", "arg2"]),
        "cmd: arg1 arg2"
    );
    assert_eq!(substitute("$1 $2 ${@:3}", &["a", "b", "c", "d"]), "a b c d");
}

#[test]
fn treats_slice_index_zero_as_all_arguments() {
    assert_eq!(substitute("${@:0}", &["a", "b", "c"]), "a b c");
}

#[test]
fn handles_slices_of_empty_and_single_argument_lists() {
    assert_eq!(substitute("${@:2}", &[]), "");
    assert_eq!(substitute("${@:1}", &[]), "");
    assert_eq!(substitute("${@:1}", &["only"]), "only");
    assert_eq!(substitute("${@:2}", &["only"]), "");
}

#[test]
fn handles_slices_embedded_in_text() {
    assert_eq!(
        substitute("Process ${@:2} with $1", &["tool", "file1", "file2"]),
        "Process file1 file2 with tool"
    );
    assert_eq!(
        substitute("${@:1:1} and ${@:2}", &["a", "b", "c"]),
        "a and b c"
    );
    assert_eq!(
        substitute("${@:1:2} vs ${@:3:2}", &["a", "b", "c", "d", "e"]),
        "a b vs c d"
    );
    assert_eq!(
        substitute("prefix${@:2}suffix", &["a", "b", "c"]),
        "prefixb csuffix"
    );
}

#[test]
fn handles_quoted_special_and_unicode_arguments_in_slices() {
    assert_eq!(
        substitute("${@:2}", &["cmd", "first arg", "second arg"]),
        "first arg second arg"
    );
    assert_eq!(
        substitute("${@:2}", &["cmd", "$100", "@user", "#tag"]),
        "$100 @user #tag"
    );
    assert_eq!(
        substitute("${@:1}", &["日本語", "🎉", "café"]),
        "日本語 🎉 café"
    );
}

#[test]
fn combines_positional_slice_and_wildcard_placeholders() {
    assert_eq!(
        substitute(
            "Run $1 on ${@:2:2}, then process $@",
            &["eslint", "file1.ts", "file2.ts", "file3.ts"]
        ),
        "Run eslint on file1.ts file2.ts, then process eslint file1.ts file2.ts file3.ts"
    );
}

#[test]
fn handles_large_slice_lengths_gracefully() {
    let values: Vec<String> = (1..=10).map(|index| format!("arg{index}")).collect();
    assert_eq!(
        substitute_args("${@:5:100}", &values),
        "arg5 arg6 arg7 arg8 arg9 arg10"
    );
}

// ----------------------------------------------------------- parseCommandArgs

#[test]
fn parses_space_separated_arguments() {
    assert_eq!(parse_command_args("a b c"), args(&["a", "b", "c"]));
}

#[test]
fn parses_quoted_arguments() {
    assert_eq!(
        parse_command_args("\"first arg\" second"),
        args(&["first arg", "second"])
    );
    assert_eq!(
        parse_command_args("'first arg' second"),
        args(&["first arg", "second"])
    );
    assert_eq!(
        parse_command_args("\"double\" 'single' \"double again\""),
        args(&["double", "single", "double again"])
    );
}

#[test]
fn parses_an_empty_string_as_no_arguments() {
    assert!(parse_command_args("").is_empty());
}

#[test]
fn collapses_runs_of_whitespace() {
    assert_eq!(parse_command_args("a  b   c"), args(&["a", "b", "c"]));
    assert_eq!(parse_command_args("a\tb\tc"), args(&["a", "b", "c"]));
    assert_eq!(parse_command_args("a\n\n\tb  c"), args(&["a", "b", "c"]));
    assert_eq!(parse_command_args("a b c   "), args(&["a", "b", "c"]));
    assert_eq!(parse_command_args("   a b c"), args(&["a", "b", "c"]));
}

/// Empty quotes contribute nothing; a quoted space is an argument.
#[test]
fn skips_empty_quotes_but_keeps_a_quoted_space() {
    assert_eq!(parse_command_args("\"\" \" \""), args(&[" "]));
}

#[test]
fn keeps_special_and_unicode_characters() {
    assert_eq!(
        parse_command_args("$100 @user #tag"),
        args(&["$100", "@user", "#tag"])
    );
    assert_eq!(
        parse_command_args("日本語 🎉 café"),
        args(&["日本語", "🎉", "café"])
    );
}

#[test]
fn keeps_newlines_inside_quotes_and_splits_on_them_outside() {
    assert_eq!(
        parse_command_args("\"line1\nline2\" second"),
        args(&["line1\nline2", "second"])
    );
    assert_eq!(
        parse_command_args("label-2\n\nHere is some description #2."),
        args(&["label-2", "Here", "is", "some", "description", "#2."])
    );
}

/// There is no backslash escape: the backslash is literal and the quote it
/// precedes still closes the run.
#[test]
fn does_not_treat_a_backslash_as_an_escape() {
    assert_eq!(
        parse_command_args("\"quoted \\\"text\\\"\""),
        args(&["quoted \\text\\"])
    );
}

// -------------------------------------------------------- expandPromptTemplate

fn template(name: &str, content: &str) -> PromptTemplate {
    PromptTemplate {
        name: name.to_string(),
        description: "test".to_string(),
        argument_hint: None,
        content: content.to_string(),
        source_info: create_synthetic_source_info(
            "/tmp/arg-test.md",
            SyntheticSourceInfoOptions {
                source: "local".to_string(),
                scope: Some(SourceScope::Temporary),
                origin: Some(SourceOrigin::TopLevel),
                base_dir: None,
            },
        ),
        file_path: "/tmp/arg-test.md".to_string(),
    }
}

#[test]
fn splits_template_arguments_on_unquoted_newlines() {
    let templates = vec![template("arg-test", "- arg1: $1\n- rest: ${@:2}")];
    let result = expand_prompt_template(
        "/arg-test label-2\n\nHere is some description #2.",
        &templates,
    );
    assert_eq!(
        result,
        "- arg1: label-2\n- rest: Here is some description #2."
    );
}

#[test]
fn accepts_a_newline_between_the_command_and_its_arguments() {
    let templates = vec![template("arg-test", "arg1: $1")];
    assert_eq!(
        expand_prompt_template("/arg-test\nlabel-2", &templates),
        "arg1: label-2"
    );
}

#[test]
fn leaves_text_that_names_no_template_alone() {
    let templates = vec![template("arg-test", "arg1: $1")];
    assert_eq!(
        expand_prompt_template("plain text", &templates),
        "plain text"
    );
    assert_eq!(
        expand_prompt_template("/unknown a b", &templates),
        "/unknown a b"
    );
}

// ------------------------------------------------------------- integration

#[test]
fn parses_and_substitutes_together() {
    let parsed = parse_command_args("Button \"onClick handler\" \"disabled support\"");
    assert_eq!(
        substitute_args("Create component $1 with features: $ARGUMENTS", &parsed),
        "Create component Button with features: Button onClick handler disabled support"
    );
    assert_eq!(
        substitute_args(
            "Create a React component named $1 with features: $ARGUMENTS",
            &parsed
        ),
        "Create a React component named Button with features: Button onClick handler disabled support"
    );

    let parsed = parse_command_args("feature1 feature2 feature3");
    assert_eq!(
        substitute_args("Implement: $@", &parsed),
        substitute_args("Implement: $ARGUMENTS", &parsed)
    );
}

// ------------------------------------------------- loadPromptTemplates: hints

fn load_from(dir: &std::path::Path) -> Vec<PromptTemplate> {
    load_prompt_templates(&LoadPromptTemplatesOptions {
        cwd: std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .into_owned(),
        agent_dir: notagent::config::get_agent_dir()
            .to_string_lossy()
            .into_owned(),
        prompt_paths: vec![dir.to_string_lossy().into_owned()],
        include_defaults: false,
    })
}

fn write_template(dir: &std::path::Path, name: &str, content: &str) {
    std::fs::create_dir_all(dir).expect("create dir");
    std::fs::write(dir.join(format!("{name}.md")), content).expect("write template");
}

#[test]
fn reads_the_argument_hint_from_the_frontmatter() {
    let dir = tempfile::tempdir().expect("temp dir");
    write_template(
        dir.path(),
        "pr",
        "---\ndescription: Review PRs from URLs with structured issue and code analysis\nargument-hint: \"<PR-URL>\"\n---\nYou are given one or more GitHub PR URLs: $@",
    );
    write_template(
        dir.path(),
        "wr",
        "---\ndescription: Finish the current task end-to-end with changelog, commit, and push\nargument-hint: \"[instructions]\"\n---\nWrap it. Additional instructions: $ARGUMENTS",
    );
    write_template(
        dir.path(),
        "cl",
        "---\ndescription: Audit changelog entries before release\n---\nAudit changelog entries for all commits since the last release.",
    );
    write_template(
        dir.path(),
        "empty-hint",
        "---\ndescription: A command with empty hint\nargument-hint: \"\"\n---\nDo something",
    );
    write_template(
        dir.path(),
        "is",
        "---\ndescription: Analyze GitHub issues (bugs or feature requests)\nargument-hint: \"<issue>\"\n---\nAnalyze GitHub issue(s): $ARGUMENTS",
    );

    let templates = load_from(dir.path());
    let find = |name: &str| {
        templates
            .iter()
            .find(|template| template.name == name)
            .unwrap_or_else(|| panic!("template {name}"))
    };

    let pr = find("pr");
    assert_eq!(pr.argument_hint.as_deref(), Some("<PR-URL>"));
    assert_eq!(
        pr.description,
        "Review PRs from URLs with structured issue and code analysis"
    );

    let wr = find("wr");
    assert_eq!(wr.argument_hint.as_deref(), Some("[instructions]"));
    assert_eq!(
        wr.description,
        "Finish the current task end-to-end with changelog, commit, and push"
    );

    assert_eq!(find("cl").argument_hint, None);
    assert_eq!(find("empty-hint").argument_hint, None);
    assert_eq!(find("is").argument_hint.as_deref(), Some("<issue>"));
}
