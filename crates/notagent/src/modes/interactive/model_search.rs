/// The subset of a model entry the search text is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSearchItem {
    pub id: String,
    pub provider: String,
    pub name: Option<String>,
}

impl ModelSearchItem {
    pub fn new(id: impl Into<String>, provider: impl Into<String>, name: Option<String>) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            name,
        }
    }
}

pub fn get_model_search_text(item: &ModelSearchItem) -> String {
    let ModelSearchItem { id, provider, .. } = item;
    let name = item
        .name
        .as_ref()
        .map_or_else(String::new, |name| format!(" {name}"));
    format!("{id} {provider} {provider}/{id} {provider} {id}{name}")
}

/// The `/model` selector search should rank exact provider-prefixed queries before proxy-provider IDs
/// like openrouter/openai/gpt-5, so keep the bare model ID out of the leading position.
pub fn get_model_selector_search_text(item: &ModelSearchItem) -> String {
    let ModelSearchItem { id, provider, .. } = item;
    let name = item
        .name
        .as_ref()
        .map_or_else(String::new, |name| format!(" {name}"));
    format!("{provider} {provider}/{id} {provider} {id}{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_the_provider_and_appends_the_display_name() {
        let item = ModelSearchItem::new("gpt-5", "openai", Some("GPT-5".to_string()));
        assert_eq!(
            get_model_search_text(&item),
            "gpt-5 openai openai/gpt-5 openai gpt-5 GPT-5"
        );
        assert_eq!(
            get_model_selector_search_text(&item),
            "openai openai/gpt-5 openai gpt-5 GPT-5"
        );
    }

    #[test]
    fn omits_the_name_segment_when_there_is_no_name() {
        let item = ModelSearchItem::new("openai/gpt-5", "openrouter", None);
        assert_eq!(
            get_model_search_text(&item),
            "openai/gpt-5 openrouter openrouter/openai/gpt-5 openrouter openai/gpt-5"
        );
        assert_eq!(
            get_model_selector_search_text(&item),
            "openrouter openrouter/openai/gpt-5 openrouter openai/gpt-5"
        );
    }
}
