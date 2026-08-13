//! Port of `packages/tui/test/fuzzy.test.ts` (112 LOC).

use notagent_tui::fuzzy::{fuzzy_filter, fuzzy_match};

// describe("fuzzyMatch")

#[test]
fn empty_query_matches_everything_with_score_0() {
    let result = fuzzy_match("", "anything");
    assert!(result.matches);
    assert_eq!(result.score, 0.0);
}

#[test]
fn query_longer_than_text_does_not_match() {
    assert!(!fuzzy_match("longquery", "short").matches);
}

#[test]
fn exact_match_has_good_score() {
    let result = fuzzy_match("test", "test");
    assert!(result.matches);
    // Negative because of the consecutive bonuses.
    assert!(result.score < 0.0);
}

#[test]
fn characters_must_appear_in_order() {
    assert!(fuzzy_match("abc", "aXbXc").matches);
    assert!(!fuzzy_match("abc", "cba").matches);
}

#[test]
fn case_insensitive_matching() {
    assert!(fuzzy_match("ABC", "abc").matches);
    assert!(fuzzy_match("abc", "ABC").matches);
}

#[test]
fn consecutive_matches_score_better_than_scattered_matches() {
    let consecutive = fuzzy_match("foo", "foobar");
    let scattered = fuzzy_match("foo", "f_o_o_bar");

    assert!(consecutive.matches);
    assert!(scattered.matches);
    assert!(consecutive.score < scattered.score);
}

#[test]
fn word_boundary_matches_score_better() {
    let at_boundary = fuzzy_match("fb", "foo-bar");
    let not_at_boundary = fuzzy_match("fb", "afbx");

    assert!(at_boundary.matches);
    assert!(not_at_boundary.matches);
    assert!(at_boundary.score < not_at_boundary.score);
}

#[test]
fn matches_swapped_alpha_numeric_tokens() {
    assert!(fuzzy_match("codex52", "gpt-5.2-codex").matches);
}

// describe("fuzzyFilter")

#[test]
fn empty_query_returns_all_items_unchanged() {
    let items = vec!["apple", "banana", "cherry"];
    assert_eq!(fuzzy_filter(&items, "", |item| (*item).to_string()), items);
}

#[test]
fn filters_out_non_matching_items() {
    let items = vec!["apple", "banana", "cherry"];
    let result = fuzzy_filter(&items, "an", |item| (*item).to_string());
    assert!(result.contains(&"banana"));
    assert!(!result.contains(&"apple"));
    assert!(!result.contains(&"cherry"));
}

#[test]
fn sorts_results_by_match_quality() {
    let items = vec!["a_p_p", "app", "application"];
    let result = fuzzy_filter(&items, "app", |item| (*item).to_string());
    // "app" is first (exact consecutive match at the start).
    assert_eq!(result[0], "app");
}

#[test]
fn prioritizes_exact_matches_over_longer_prefix_matches() {
    let items = vec!["clone", "cl"];
    let result = fuzzy_filter(&items, "cl", |item| (*item).to_string());
    assert_eq!(result, ["cl", "clone"]);
}

#[test]
fn works_with_custom_get_text_function() {
    #[derive(Clone, PartialEq, Debug)]
    struct Item {
        name: &'static str,
        id: u32,
    }
    let items = vec![
        Item { name: "foo", id: 1 },
        Item { name: "bar", id: 2 },
        Item {
            name: "foobar",
            id: 3,
        },
    ];
    let result = fuzzy_filter(&items, "foo", |item| item.name.to_string());

    assert_eq!(result.len(), 2);
    assert!(result.iter().any(|item| item.name == "foo"));
    assert!(result.iter().any(|item| item.name == "foobar"));
}

#[test]
fn matches_slash_separated_provider_model_queries_against_reordered_text() {
    #[derive(Clone, PartialEq, Debug)]
    struct Model {
        id: &'static str,
        provider: &'static str,
    }
    let item = Model {
        id: "gpt-5.5",
        provider: "openai-codex",
    };
    let result = fuzzy_filter(
        std::slice::from_ref(&item),
        "openai-codex/gpt-5.5",
        |model| format!("{} {}", model.id, model.provider),
    );
    assert_eq!(result, [item]);
}
