//! Header conversions.
//!
//! 1:1 port of `packages/ai/src/utils/headers.ts` (18 LOC).

use std::collections::BTreeMap;

use crate::types::ProviderHeaders;

/// `headersToRecord(headers)`
pub fn headers_to_record(headers: &[(String, String)]) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

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
