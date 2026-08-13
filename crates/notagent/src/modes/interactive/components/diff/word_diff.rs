//! Port of jsdiff 8.0.4's `diffWords` — the one function of the `diff` npm
//! package that has no counterpart in the `similar` crate the master plan
//! substitutes for unified patches.
//!
//! Sources: `node_modules/diff/libesm/diff/base.js` (253 LOC),
//! `libesm/diff/word.js` (281 LOC) and the helpers of `libesm/util/string.js`
//! (184 LOC). Only the code paths `renderDiff` reaches are ported: no
//! `Intl.Segmenter`, no `ignoreCase`, no `comparator`, no `oneChangePerToken`,
//! no `maxEditLength`, no `timeout`, no callback.
//!
//! `tests/diff_words_oracle.rs` compares the output against the real jsdiff for
//! a generated corpus.

use std::collections::HashMap;

/// One change object of the diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The text of this run of tokens.
    pub value: String,
    /// Present only in the new text.
    pub added: bool,
    /// Present only in the old text.
    pub removed: bool,
}

// --- character classes ---------------------------------------------------------

/// `\s` of a JavaScript regular expression, which is also the character set
/// `String.prototype.trim` removes.
pub fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{9}'..='\u{D}'
            | '\u{20}'
            | '\u{A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// `extendedWordChars` of `word.js`.
fn is_extended_word_char(character: char) -> bool {
    matches!(
        character,
        'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '_'
            | '\u{AD}'
            | '\u{C0}'..='\u{D6}'
            | '\u{D8}'..='\u{F6}'
            | '\u{F8}'..='\u{2C6}'
            | '\u{2C8}'..='\u{2D7}'
            | '\u{2DE}'..='\u{2FF}'
            | '\u{1E00}'..='\u{1EFF}'
    )
}

/// `value.trim()`.
fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

/// `string.match(/^\s*/)[0]`.
pub fn leading_ws(value: &str) -> &str {
    let end = value
        .char_indices()
        .find(|(_, character)| !is_js_whitespace(*character))
        .map_or(value.len(), |(index, _)| index);
    &value[..end]
}

/// The trailing whitespace run of `value`.
fn trailing_ws(value: &str) -> &str {
    let start = value
        .char_indices()
        .rev()
        .find(|(_, character)| !is_js_whitespace(*character))
        .map_or(0, |(index, character)| index + character.len_utf8());
    &value[start..]
}

/// `leadingAndTrailingWs` without a segmenter.
fn leading_and_trailing_ws(value: &str) -> (&str, &str) {
    (leading_ws(value), trailing_ws(value))
}

// --- string helpers ------------------------------------------------------------
//
// jsdiff indexes by UTF-16 units. Every string these helpers slice is either
// pure whitespace (all BMP, one unit per character) or is sliced by the length
// of such a whitespace run, so counting characters gives the same result.

fn longest_common_prefix<'a>(first: &'a str, second: &str) -> &'a str {
    let mut end = 0;
    for (left, right) in first.char_indices().zip(second.chars()) {
        if left.1 != right {
            return &first[..end];
        }
        end = left.0 + left.1.len_utf8();
    }
    &first[..end]
}

fn longest_common_suffix<'a>(first: &'a str, second: &str) -> &'a str {
    if first.is_empty() || second.is_empty() {
        return "";
    }
    let first_chars: Vec<char> = first.chars().collect();
    let second_chars: Vec<char> = second.chars().collect();
    if first_chars[first_chars.len() - 1] != second_chars[second_chars.len() - 1] {
        return "";
    }
    let mut common = 0;
    while common < first_chars.len() && common < second_chars.len() {
        if first_chars[first_chars.len() - (common + 1)]
            != second_chars[second_chars.len() - (common + 1)]
        {
            break;
        }
        common += 1;
    }
    let start = first
        .char_indices()
        .rev()
        .nth(common - 1)
        .map_or(0, |(index, _)| index);
    &first[start..]
}

fn replace_prefix(value: &str, old_prefix: &str, new_prefix: &str) -> String {
    debug_assert!(
        value.starts_with(old_prefix),
        "string {value:?} doesn't start with prefix {old_prefix:?}; this is a bug"
    );
    format!("{new_prefix}{}", &value[old_prefix.len()..])
}

fn replace_suffix(value: &str, old_suffix: &str, new_suffix: &str) -> String {
    if old_suffix.is_empty() {
        return format!("{value}{new_suffix}");
    }
    debug_assert!(
        value.ends_with(old_suffix),
        "string {value:?} doesn't end with suffix {old_suffix:?}; this is a bug"
    );
    format!("{}{new_suffix}", &value[..value.len() - old_suffix.len()])
}

fn remove_prefix(value: &str, old_prefix: &str) -> String {
    replace_prefix(value, old_prefix, "")
}

fn remove_suffix(value: &str, old_suffix: &str) -> String {
    replace_suffix(value, old_suffix, "")
}

/// `maximumOverlap`: the longest prefix of `second` that is a suffix of `first`.
fn maximum_overlap<'a>(first: &str, second: &'a str) -> &'a str {
    let overlap = overlap_count(first, second);
    let end = second
        .char_indices()
        .nth(overlap)
        .map_or(second.len(), |(index, _)| index);
    &second[..end]
}

fn overlap_count(first: &str, second: &str) -> usize {
    let a: Vec<char> = first.chars().collect();
    let b: Vec<char> = second.chars().collect();
    // Deal with cases where the strings differ in length
    let start_a = a.len().saturating_sub(b.len());
    let end_b = if a.len() < b.len() { a.len() } else { b.len() };
    if end_b == 0 {
        return 0;
    }

    // Create a back-reference for each index that should be followed in case of
    // a mismatch. We only need B to make these references:
    let mut map = vec![0usize; end_b];
    let mut k = 0usize; // Index that lags behind j
    for j in 1..end_b {
        if b[j] == b[k] {
            map[j] = map[k];
        } else {
            map[j] = k;
        }
        while k > 0 && b[j] != b[k] {
            k = map[k];
        }
        if b[j] == b[k] {
            k += 1;
        }
    }

    // Phase 2: use these references while iterating over A
    k = 0;
    for item in a.iter().skip(start_a) {
        while k > 0 && *item != b[k] {
            k = map[k];
        }
        if *item == b[k] {
            k += 1;
        }
    }
    k
}

// --- tokenizer -----------------------------------------------------------------

/// `WordDiff.tokenize` without a segmenter.
fn tokenize(value: &str) -> Vec<String> {
    // `[extendedWordChars]+|\s+|[^extendedWordChars]`, in that order.
    let chars: Vec<char> = value.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let start = index;
        if is_extended_word_char(chars[index]) {
            while index < chars.len() && is_extended_word_char(chars[index]) {
                index += 1;
            }
        } else if is_js_whitespace(chars[index]) {
            while index < chars.len() && is_js_whitespace(chars[index]) {
                index += 1;
            }
        } else {
            index += 1;
        }
        parts.push(chars[start..index].iter().collect());
    }

    // Stitch whitespace runs onto the adjacent word or punctuation token.
    let mut tokens: Vec<String> = Vec::new();
    let mut previous_part: Option<&String> = None;
    for part in &parts {
        let part_is_whitespace = part.chars().any(is_js_whitespace);
        if part_is_whitespace {
            match previous_part {
                None => tokens.push(part.clone()),
                Some(_) => {
                    let last = tokens.pop().unwrap_or_default();
                    tokens.push(last + part);
                }
            }
        } else if previous_part.is_some_and(|previous| previous.chars().any(is_js_whitespace)) {
            let previous = previous_part.expect("checked");
            if tokens.last().is_some_and(|last| last == previous) {
                let last = tokens.pop().expect("checked");
                tokens.push(last + part);
            } else {
                tokens.push(format!("{previous}{part}"));
            }
        } else {
            tokens.push(part.clone());
        }
        previous_part = Some(part);
    }
    tokens
}

/// `WordDiff.join`: only the first token keeps its leading whitespace.
fn join(tokens: &[String]) -> String {
    let mut joined = String::new();
    for (index, token) in tokens.iter().enumerate() {
        if index == 0 {
            joined.push_str(token);
        } else {
            joined.push_str(&token[leading_ws(token).len()..]);
        }
    }
    joined
}

/// `WordDiff.equals`.
fn equals(left: &str, right: &str) -> bool {
    js_trim(left) == js_trim(right)
}

// --- Myers diff (base.js) --------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    previous: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct PathEntry {
    old_pos: i64,
    last_component: Option<usize>,
}

struct Differ<'a> {
    components: Vec<Component>,
    old_tokens: &'a [String],
    new_tokens: &'a [String],
}

impl<'a> Differ<'a> {
    fn push_component(&mut self, component: Component) -> usize {
        self.components.push(component);
        self.components.len() - 1
    }

    fn add_to_path(
        &mut self,
        path: PathEntry,
        added: bool,
        removed: bool,
        old_pos_inc: i64,
    ) -> PathEntry {
        if let Some(last_index) = path.last_component {
            let last = self.components[last_index];
            if last.added == added && last.removed == removed {
                let index = self.push_component(Component {
                    count: last.count + 1,
                    added,
                    removed,
                    previous: last.previous,
                });
                return PathEntry {
                    old_pos: path.old_pos + old_pos_inc,
                    last_component: Some(index),
                };
            }
        }
        let index = self.push_component(Component {
            count: 1,
            added,
            removed,
            previous: path.last_component,
        });
        PathEntry {
            old_pos: path.old_pos + old_pos_inc,
            last_component: Some(index),
        }
    }

    fn extract_common(&mut self, base_path: &mut PathEntry, diagonal_path: i64) -> i64 {
        let new_len = self.new_tokens.len() as i64;
        let old_len = self.old_tokens.len() as i64;
        let mut old_pos = base_path.old_pos;
        let mut new_pos = old_pos - diagonal_path;
        let mut common_count = 0usize;
        while new_pos + 1 < new_len
            && old_pos + 1 < old_len
            && equals(
                &self.old_tokens[(old_pos + 1) as usize],
                &self.new_tokens[(new_pos + 1) as usize],
            )
        {
            new_pos += 1;
            old_pos += 1;
            common_count += 1;
        }
        if common_count > 0 {
            let index = self.push_component(Component {
                count: common_count,
                added: false,
                removed: false,
                previous: base_path.last_component,
            });
            base_path.last_component = Some(index);
        }
        base_path.old_pos = old_pos;
        new_pos
    }

    fn build_values(&self, last_component: Option<usize>) -> Vec<Change> {
        // Convert the linked list of components in reverse order to an array in
        // the right order.
        let mut order: Vec<usize> = Vec::new();
        let mut cursor = last_component;
        while let Some(index) = cursor {
            order.push(index);
            cursor = self.components[index].previous;
        }
        order.reverse();

        let mut changes: Vec<Change> = Vec::with_capacity(order.len());
        let mut new_pos = 0usize;
        let mut old_pos = 0usize;
        for index in order {
            let component = self.components[index];
            let value = if component.removed {
                let value = join(&self.old_tokens[old_pos..old_pos + component.count]);
                old_pos += component.count;
                value
            } else {
                let value = join(&self.new_tokens[new_pos..new_pos + component.count]);
                new_pos += component.count;
                if !component.added {
                    old_pos += component.count;
                }
                value
            };
            changes.push(Change {
                value,
                added: component.added,
                removed: component.removed,
            });
        }
        changes
    }
}

/// `Diff.prototype.diff` for the word tokenizer.
fn diff_tokens(old_tokens: &[String], new_tokens: &[String]) -> Vec<Change> {
    let mut differ = Differ {
        components: Vec::new(),
        old_tokens,
        new_tokens,
    };
    let new_len = new_tokens.len() as i64;
    let old_len = old_tokens.len() as i64;
    let max_edit_length = new_len + old_len;

    let mut best_path: HashMap<i64, PathEntry> = HashMap::new();
    let mut seed = PathEntry {
        old_pos: -1,
        last_component: None,
    };
    // Seed editLength = 0, i.e. the content starts with the same values
    let mut new_pos = differ.extract_common(&mut seed, 0);
    if seed.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        // Identity per the equality and tokenizer
        return differ.build_values(seed.last_component);
    }
    best_path.insert(0, seed);

    let mut min_diagonal_to_consider = i64::MIN;
    let mut max_diagonal_to_consider = i64::MAX;
    let mut edit_length = 1i64;

    while edit_length <= max_edit_length {
        let mut diagonal_path = min_diagonal_to_consider.max(-edit_length);
        let last_diagonal = max_diagonal_to_consider.min(edit_length);
        while diagonal_path <= last_diagonal {
            let remove_path = best_path.get(&(diagonal_path - 1)).copied();
            let add_path = best_path.get(&(diagonal_path + 1)).copied();
            if remove_path.is_some() {
                // No one else is going to attempt to use this value, clear it
                best_path.remove(&(diagonal_path - 1));
            }

            let mut can_add = false;
            if let Some(add_path) = add_path {
                // what newPos will be after we do an insertion:
                let add_path_new_pos = add_path.old_pos - diagonal_path;
                can_add = (0..new_len).contains(&add_path_new_pos);
            }
            let can_remove = remove_path.is_some_and(|path| path.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                // If this path is a terminal then prune
                best_path.remove(&diagonal_path);
                diagonal_path += 2;
                continue;
            }

            // Select the diagonal that we want to branch from. We select the
            // prior path whose position in the old string is the farthest from
            // the origin and does not pass the bounds of the diff graph
            let mut base_path = if !can_remove
                || (can_add
                    && remove_path.expect("can_remove implies a path").old_pos
                        < add_path.expect("can_add implies a path").old_pos)
            {
                differ.add_to_path(add_path.expect("can_add implies a path"), true, false, 0)
            } else {
                differ.add_to_path(
                    remove_path.expect("can_remove implies a path"),
                    false,
                    true,
                    1,
                )
            };
            new_pos = differ.extract_common(&mut base_path, diagonal_path);

            if base_path.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                // If we have hit the end of both strings, then we are done
                return differ.build_values(base_path.last_component);
            }
            if base_path.old_pos + 1 >= old_len {
                max_diagonal_to_consider = max_diagonal_to_consider.min(diagonal_path - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal_to_consider = min_diagonal_to_consider.max(diagonal_path + 1);
            }
            best_path.insert(diagonal_path, base_path);
            diagonal_path += 2;
        }
        edit_length += 1;
    }
    // jsdiff returns `undefined` when the edit length is exhausted; that cannot
    // happen without `maxEditLength`, because `newLen + oldLen` edits always
    // suffice.
    Vec::new()
}

// --- postProcess -----------------------------------------------------------------

/// `WordDiff.postProcess`.
fn post_process(changes: &mut [Change]) {
    if changes.is_empty() {
        return;
    }
    let mut last_keep: Option<usize> = None;
    // Change objects representing any insertion or deletion since the last
    // "keep" change object. There can be at most one of each.
    let mut insertion: Option<usize> = None;
    let mut deletion: Option<usize> = None;
    for index in 0..changes.len() {
        if changes[index].added {
            insertion = Some(index);
        } else if changes[index].removed {
            deletion = Some(index);
        } else {
            if insertion.is_some() || deletion.is_some() {
                // May be false at start of text
                dedupe_whitespace(changes, last_keep, deletion, insertion, Some(index));
            }
            last_keep = Some(index);
            insertion = None;
            deletion = None;
        }
    }
    if insertion.is_some() || deletion.is_some() {
        dedupe_whitespace(changes, last_keep, deletion, insertion, None);
    }
}

fn dedupe_whitespace(
    changes: &mut [Change],
    start_keep: Option<usize>,
    deletion: Option<usize>,
    insertion: Option<usize>,
    end_keep: Option<usize>,
) {
    match (deletion, insertion) {
        (Some(deletion), Some(insertion)) => {
            let deletion_value = changes[deletion].value.clone();
            let (old_ws_prefix, old_ws_suffix) = leading_and_trailing_ws(&deletion_value);
            let (old_ws_prefix, old_ws_suffix) =
                (old_ws_prefix.to_string(), old_ws_suffix.to_string());
            let insertion_value = changes[insertion].value.clone();
            let (new_ws_prefix, new_ws_suffix) = leading_and_trailing_ws(&insertion_value);
            let (new_ws_prefix, new_ws_suffix) =
                (new_ws_prefix.to_string(), new_ws_suffix.to_string());
            if let Some(start_keep) = start_keep {
                let common_ws_prefix =
                    longest_common_prefix(&old_ws_prefix, &new_ws_prefix).to_string();
                changes[start_keep].value = replace_suffix(
                    &changes[start_keep].value,
                    &new_ws_prefix,
                    &common_ws_prefix,
                );
                changes[deletion].value =
                    remove_prefix(&changes[deletion].value, &common_ws_prefix);
                changes[insertion].value =
                    remove_prefix(&changes[insertion].value, &common_ws_prefix);
            }
            if let Some(end_keep) = end_keep {
                let common_ws_suffix =
                    longest_common_suffix(&old_ws_suffix, &new_ws_suffix).to_string();
                changes[end_keep].value =
                    replace_prefix(&changes[end_keep].value, &new_ws_suffix, &common_ws_suffix);
                changes[deletion].value =
                    remove_suffix(&changes[deletion].value, &common_ws_suffix);
                changes[insertion].value =
                    remove_suffix(&changes[insertion].value, &common_ws_suffix);
            }
        }
        (_, Some(insertion)) => {
            // The whitespaces all reflect what was in the new text rather than
            // the old, so we essentially have no information about whitespace
            // insertion or deletion. We just want to dedupe the whitespace.
            if start_keep.is_some() {
                let whitespace = leading_ws(&changes[insertion].value).len();
                changes[insertion].value = changes[insertion].value[whitespace..].to_string();
            }
            if let Some(end_keep) = end_keep {
                let whitespace = leading_ws(&changes[end_keep].value).len();
                changes[end_keep].value = changes[end_keep].value[whitespace..].to_string();
            }
        }
        // otherwise we've got a deletion and no insertion
        (Some(deletion), None) => match (start_keep, end_keep) {
            (Some(start_keep), Some(end_keep)) => {
                let new_ws_full = leading_ws(&changes[end_keep].value).to_string();
                let deletion_value = changes[deletion].value.clone();
                let (del_ws_start, del_ws_end) = leading_and_trailing_ws(&deletion_value);
                let (del_ws_start, del_ws_end) = (del_ws_start.to_string(), del_ws_end.to_string());
                // Any whitespace that comes straight after startKeep in both the
                // old and new texts, assign to startKeep and remove from the
                // deletion.
                let new_ws_start = longest_common_prefix(&new_ws_full, &del_ws_start).to_string();
                changes[deletion].value = remove_prefix(&changes[deletion].value, &new_ws_start);
                // Any whitespace that comes straight before endKeep in both the
                // old and new texts, and hasn't already been assigned to
                // startKeep, assign to endKeep and remove from the deletion.
                let remaining = remove_prefix(&new_ws_full, &new_ws_start);
                let new_ws_end = longest_common_suffix(&remaining, &del_ws_end).to_string();
                changes[deletion].value = remove_suffix(&changes[deletion].value, &new_ws_end);
                changes[end_keep].value =
                    replace_prefix(&changes[end_keep].value, &new_ws_full, &new_ws_end);
                // If there's any whitespace from the new text that HASN'T
                // already been assigned, assign it to the start:
                let unassigned = new_ws_full[..new_ws_full.len() - new_ws_end.len()].to_string();
                changes[start_keep].value =
                    replace_suffix(&changes[start_keep].value, &new_ws_full, &unassigned);
            }
            (None, Some(end_keep)) => {
                // We are at the start of the text. Preserve all the whitespace
                // on endKeep, and just remove whitespace from the end of
                // deletion to the extent that it overlaps with the start of
                // endKeep.
                let end_keep_ws_prefix = leading_ws(&changes[end_keep].value).to_string();
                let deletion_ws_suffix = trailing_ws(&changes[deletion].value).to_string();
                let overlap = maximum_overlap(&deletion_ws_suffix, &end_keep_ws_prefix).to_string();
                changes[deletion].value = remove_suffix(&changes[deletion].value, &overlap);
            }
            (Some(start_keep), None) => {
                // We are at the END of the text. Preserve all the whitespace on
                // startKeep, and just remove whitespace from the start of
                // deletion to the extent that it overlaps with the end of
                // startKeep.
                let start_keep_ws_suffix = trailing_ws(&changes[start_keep].value).to_string();
                let deletion_ws_prefix = leading_ws(&changes[deletion].value).to_string();
                let overlap =
                    maximum_overlap(&start_keep_ws_suffix, &deletion_ws_prefix).to_string();
                changes[deletion].value = remove_prefix(&changes[deletion].value, &overlap);
            }
            (None, None) => {}
        },
        (None, None) => {}
    }
}

/// `Diff.diffWords(oldStr, newStr)`.
pub fn diff_words(old_str: &str, new_str: &str) -> Vec<Change> {
    let old_tokens: Vec<String> = tokenize(old_str)
        .into_iter()
        .filter(|token| !token.is_empty())
        .collect();
    let new_tokens: Vec<String> = tokenize(new_str)
        .into_iter()
        .filter(|token| !token.is_empty())
        .collect();
    let mut changes = diff_tokens(&old_tokens, &new_tokens);
    post_process(&mut changes);
    changes
}
