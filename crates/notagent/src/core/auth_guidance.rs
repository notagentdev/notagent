use crate::config::get_docs_path;

const UNKNOWN_PROVIDER: &str = "unknown";

/// `getProviderLoginHelp()`
pub fn get_provider_login_help() -> String {
    let docs = get_docs_path();
    [
        "Use /login to log into a provider via OAuth or API key. See:".to_owned(),
        format!("  {}", docs.join("providers.md").display()),
        format!("  {}", docs.join("models.md").display()),
    ]
    .join("\n")
}

/// `formatNoModelsAvailableMessage()`
pub fn format_no_models_available_message() -> String {
    format!("No models available. {}", get_provider_login_help())
}

/// `formatNoModelSelectedMessage()`
pub fn format_no_model_selected_message() -> String {
    format!(
        "No model selected.\n\n{}\n\nThen use /model to select a model.",
        get_provider_login_help()
    )
}

/// `formatNoApiKeyFoundMessage(provider)`
pub fn format_no_api_key_found_message(provider: &str) -> String {
    let provider_display = if provider == UNKNOWN_PROVIDER {
        "the selected model"
    } else {
        provider
    };
    format!(
        "No API key found for {provider_display}.\n\n{}",
        get_provider_login_help()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_provider_reads_as_the_selected_model() {
        assert!(
            format_no_api_key_found_message("unknown")
                .starts_with("No API key found for the selected model.")
        );
        assert!(
            format_no_api_key_found_message("anthropic")
                .starts_with("No API key found for anthropic.")
        );
    }

    #[test]
    fn the_login_help_lists_both_docs_pages() {
        let help = get_provider_login_help();
        assert!(help.contains("providers.md"));
        assert!(help.contains("models.md"));
    }
}
