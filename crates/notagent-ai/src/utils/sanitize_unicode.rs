/// `sanitizeSurrogates(text)`
pub fn sanitize_surrogates(text: &str) -> String {
    text.to_string()
}

/// Same guarantee for text that is still a UTF-16 code-unit sequence, e.g. decoded from
/// a provider payload that carries lone surrogates in `\uXXXX` escapes.
pub fn sanitize_surrogates_utf16(code_units: &[u16]) -> String {
    let mut kept = Vec::with_capacity(code_units.len());
    let mut index = 0;
    while index < code_units.len() {
        let unit = code_units[index];
        if (0xD800..=0xDBFF).contains(&unit) {
            let next = code_units.get(index + 1).copied();
            if next.is_some_and(|next| (0xDC00..=0xDFFF).contains(&next)) {
                kept.push(unit);
                kept.push(next.expect("checked above"));
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if (0xDC00..=0xDFFF).contains(&unit) {
            index += 1;
            continue;
        }
        kept.push(unit);
        index += 1;
    }
    String::from_utf16(&kept).expect("unpaired surrogates were removed")
}
