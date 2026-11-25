use std::collections::BTreeMap;

use crate::types::ProviderHeaders;

/// `providerHeadersToRecord(headers)` — drops `null` values, returns `None` when empty.
pub fn provider_headers_to_record(
    headers: Option<&ProviderHeaders>,
) -> Option<BTreeMap<String, String>> {
    let headers = headers?;
    let result: BTreeMap<String, String> = headers
        .iter()
        .filter_map(|(name, value)| value.as_ref().map(|value| (name.clone(), value.clone())))
        .collect();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}
