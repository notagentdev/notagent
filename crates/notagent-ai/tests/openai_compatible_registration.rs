//! The three server-backed providers must reach the built-in set with an
//! api-key login, which is what puts them in the login list.

use notagent_ai::models::Provider;
use notagent_ai::providers::all::builtin_providers;

#[test]
fn the_server_backed_providers_are_built_in_and_offer_a_login() {
    let providers = builtin_providers();
    for id in ["ollama", "lmstudio", "custom"] {
        let provider = providers
            .iter()
            .find(|provider| provider.id() == id)
            .unwrap_or_else(|| panic!("{id} is missing from the built-in set"));
        assert!(
            provider.auth().api_key.is_some(),
            "{id} offers no api-key login, so it cannot appear in the login list"
        );
        assert!(
            provider.auth().api_key.as_ref().is_some_and(|api_key| {
                let interaction = ();
                let _ = interaction;
                !api_key.name().is_empty()
            }),
            "{id} has no login name"
        );
    }
}
