/// A decoded entity and how many bytes of input it occupied, including `&` and `;`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedHtmlEntity {
    pub text: String,
    pub length: usize,
}

/// `Number.parseInt(text, radix)` for the two radixes the entity syntax uses.
/// JavaScript's `parseInt` is deliberately sloppy: it skips leading whitespace,
/// takes an optional sign, accepts an `0x` prefix for radix 16 and stops at the
/// first character that is not a digit instead of failing. `&#41.5;` therefore
fn js_parse_int(text: &str, radix: u32) -> Option<i64> {
    let rest = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let (negative, rest) = match rest.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, rest.strip_prefix('+').unwrap_or(rest)),
    };
    let rest = if radix == 16 {
        rest.strip_prefix("0x")
            .or_else(|| rest.strip_prefix("0X"))
            .unwrap_or(rest)
    } else {
        rest
    };

    let mut value: i64 = 0;
    let mut digits = 0usize;
    for digit in rest.chars().map_while(|c| c.to_digit(radix)) {
        value = value
            .saturating_mul(i64::from(radix))
            .saturating_add(i64::from(digit));
        digits += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(if negative { -value } else { value })
}

fn decode_code_point(code_point: Option<i64>) -> Option<String> {
    let code_point = code_point?;
    if !(0..=0x10ffff).contains(&code_point) {
        return None;
    }
    // `String.fromCodePoint` also accepts lone surrogates; a Rust `char` cannot
    // hold one, so those entities stay undecoded and are copied through as text.
    char::from_u32(u32::try_from(code_point).ok()?).map(String::from)
}

/// Decodes the body of an entity — what stands between `&` and `;`.
pub fn decode_html_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => return Some("&".to_string()),
        "lt" => return Some("<".to_string()),
        "gt" => return Some(">".to_string()),
        "quot" => return Some("\"".to_string()),
        "apos" => return Some("'".to_string()),
        _ => {}
    }

    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return decode_code_point(js_parse_int(hex, 16));
    }

    if let Some(decimal) = entity.strip_prefix('#') {
        return decode_code_point(js_parse_int(decimal, 10));
    }

    None
}

/// Decodes the entity starting at byte `index` (which must point at an `&`).
/// half the document for a semicolon that belongs to something else.
pub fn decode_html_entity_at(html: &str, index: usize) -> Option<DecodedHtmlEntity> {
    let after_ampersand = index + 1;
    let semicolon_index = html.get(after_ampersand..)?.find(';')? + after_ampersand;
    if semicolon_index - index > 16 {
        return None;
    }

    let entity = &html[after_ampersand..semicolon_index];
    let text = decode_html_entity(entity)?;
    Some(DecodedHtmlEntity {
        text,
        length: semicolon_index - index + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_five_named_entities() {
        assert_eq!(decode_html_entity("amp").as_deref(), Some("&"));
        assert_eq!(decode_html_entity("lt").as_deref(), Some("<"));
        assert_eq!(decode_html_entity("gt").as_deref(), Some(">"));
        assert_eq!(decode_html_entity("quot").as_deref(), Some("\""));
        assert_eq!(decode_html_entity("apos").as_deref(), Some("'"));
        assert_eq!(decode_html_entity("nbsp"), None);
    }

    #[test]
    fn decodes_numeric_entities_in_both_bases() {
        assert_eq!(decode_html_entity("#65").as_deref(), Some("A"));
        assert_eq!(decode_html_entity("#x41").as_deref(), Some("A"));
        assert_eq!(decode_html_entity("#X41").as_deref(), Some("A"));
        assert_eq!(decode_html_entity("#x1f600").as_deref(), Some("😀"));
    }

    #[test]
    fn rejects_out_of_range_and_unparsable_code_points() {
        assert_eq!(decode_html_entity("#x110000"), None);
        assert_eq!(decode_html_entity("#-1"), None);
        assert_eq!(decode_html_entity("#xzz"), None);
        assert_eq!(decode_html_entity("#"), None);
        // A lone surrogate is a valid JavaScript string but not a Rust `char`.
        assert_eq!(decode_html_entity("#xd800"), None);
    }

    #[test]
    fn parses_leading_digits_like_javascript() {
        assert_eq!(decode_html_entity("#41.5").as_deref(), Some(")"));
        assert_eq!(decode_html_entity("#x41zz").as_deref(), Some("A"));
    }

    #[test]
    fn reports_the_consumed_length() {
        let decoded = decode_html_entity_at("a&amp;b", 1).unwrap();
        assert_eq!(
            decoded,
            DecodedHtmlEntity {
                text: "&".to_string(),
                length: 5
            }
        );
    }

    #[test]
    fn gives_up_without_a_nearby_semicolon() {
        assert_eq!(decode_html_entity_at("&amp", 0), None);
        assert_eq!(decode_html_entity_at("&aaaaaaaaaaaaaaaaaa;", 0), None);
    }
}
