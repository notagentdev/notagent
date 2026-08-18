//! Breitenberechnung, ANSI-Parsing, Word-Wrap und Zeilen-Slicing.
//!
//! 1:1-Port von `packages/tui/src/utils.ts` (1326 LOC).
//!
//! Zwei Eigenheiten der Vorlage, die der Port beibehält:
//! - Zeilen sind Strings mit eingebetteten ANSI-Sequenzen (kein Zellpuffer).
//! - `extract_ansi_code` erkennt bei CSI **nur** die Finalbytes `m G K H J`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;

use unicode_segmentation::UnicodeSegmentation;

use crate::unicode_tables::{
    CJK_BREAK, EAW_WIDE, LEADING_NON_PRINTING, MARK_CHAR, NON_PRINTING_CHAR, RGI_BASIC_SINGLE,
    RGI_BASIC_VS16, RGI_FLAG_PAIRS, RGI_KEYCAP_BASE, RGI_MODIFIER_BASE, RGI_TAG_SEQUENCES,
    RGI_ZWJ_SEQUENCES, TERMINAL_SPACING_MARK, ZERO_WIDTH, in_ranges,
};

/// Graphem-Cluster eines Strings (entspricht `Intl.Segmenter` mit
/// `granularity: "grapheme"`; UAX #29 erweiterte Cluster).
pub fn graphemes(text: &str) -> impl DoubleEndedIterator<Item = &str> {
    UnicodeSegmentation::graphemes(text, true)
}

/// Wortsegmente eines Strings (entspricht `Intl.Segmenter` mit
/// `granularity: "word"`; UAX #29 Wortgrenzen).
pub fn word_segments_public(text: &str) -> impl Iterator<Item = &str> {
    word_segments(text)
}

pub(crate) fn word_segments(text: &str) -> impl Iterator<Item = &str> {
    text.split_word_bounds()
}

/// Schnelle Vorprüfung, ob ein Cluster überhaupt ein RGI-Emoji sein kann.
///
/// Die geprüften Unicode-Blöcke sind bewusst großzügig (`utils.ts:27-37`).
fn could_be_emoji(segment: &str) -> bool {
    let Some(cp) = segment.chars().next().map(u32::from) else {
        return false;
    };
    (0x1f000..=0x1fbff).contains(&cp)
        || (0x2300..=0x23ff).contains(&cp)
        || (0x2600..=0x27bf).contains(&cp)
        || (0x2b50..=0x2b55).contains(&cp)
        || segment.contains('\u{fe0f}')
        // TS: `segment.length > 2` zählt UTF-16-Codeeinheiten.
        || segment.encode_utf16().count() > 2
}

fn is_zero_width_cluster(segment: &str) -> bool {
    !segment.is_empty() && segment.chars().all(|c| in_ranges(ZERO_WIDTH, u32::from(c)))
}

fn is_terminal_spacing_mark_cluster(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| in_ranges(TERMINAL_SPACING_MARK, u32::from(c)))
}

fn is_mark_char(c: char) -> bool {
    in_ranges(MARK_CHAR, u32::from(c))
}

fn is_non_printing_char(c: char) -> bool {
    in_ranges(NON_PRINTING_CHAR, u32::from(c))
}

/// Entspricht `cjkBreakRegex.test(...)` — Han/Hiragana/Katakana/Hangul/Bopomofo.
pub fn is_cjk_break(text: &str) -> bool {
    text.chars().any(|c| in_ranges(CJK_BREAK, u32::from(c)))
}

/// `eastAsianWidth(cp)` aus `get-east-asian-width` (ambiguousAsWide = false).
fn east_asian_width(cp: u32) -> usize {
    if in_ranges(EAW_WIDE, cp) { 2 } else { 1 }
}

const EMOJI_MODIFIERS: [char; 5] = [
    '\u{1f3fb}',
    '\u{1f3fc}',
    '\u{1f3fd}',
    '\u{1f3fe}',
    '\u{1f3ff}',
];

fn is_regional_indicator(c: char) -> bool {
    ('\u{1f1e6}'..='\u{1f1ff}').contains(&c)
}

/// Entspricht `\p{RGI_Emoji}` (Basic_Emoji, Keycap, Flag, Tag, Modifier, ZWJ).
fn is_rgi_emoji(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let rest: Vec<char> = chars.collect();

    match rest.len() {
        0 => return in_ranges(RGI_BASIC_SINGLE, u32::from(first)),
        1 => {
            if rest[0] == '\u{fe0f}' {
                return in_ranges(RGI_BASIC_VS16, u32::from(first));
            }
            if is_regional_indicator(first) && is_regional_indicator(rest[0]) {
                let a = u32::from(first) - 0x1f1e6;
                let b = u32::from(rest[0]) - 0x1f1e6;
                let index = u16::try_from(a * 26 + b).expect("regional indicator index fits u16");
                return RGI_FLAG_PAIRS.binary_search(&index).is_ok();
            }
            if EMOJI_MODIFIERS.contains(&rest[0]) {
                return in_ranges(RGI_MODIFIER_BASE, u32::from(first));
            }
        }
        2 if rest[0] == '\u{fe0f}' && rest[1] == '\u{20e3}' => {
            return in_ranges(RGI_KEYCAP_BASE, u32::from(first));
        }
        _ => {}
    }

    if first == '\u{1f3f4}' && RGI_TAG_SEQUENCES.contains(&segment) {
        return true;
    }
    RGI_ZWJ_SEQUENCES.binary_search(&segment).is_ok()
}

// Cache für Nicht-ASCII-Strings (`utils.ts:50-52`).
const WIDTH_CACHE_SIZE: usize = 512;

thread_local! {
    static WIDTH_CACHE: RefCell<(HashMap<String, usize>, VecDeque<String>)> =
        RefCell::new((HashMap::new(), VecDeque::new()));
}

fn is_printable_ascii(text: &str) -> bool {
    text.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

struct Fragment {
    text: String,
    width: usize,
}

fn truncate_fragment_to_width(text: &str, max_width: usize) -> Fragment {
    if max_width == 0 || text.is_empty() {
        return Fragment {
            text: String::new(),
            width: 0,
        };
    }

    if is_printable_ascii(text) {
        let clipped = &text[..text.len().min(max_width)];
        return Fragment {
            text: clipped.to_string(),
            width: clipped.len(),
        };
    }

    let has_ansi = text.contains('\x1b');
    let has_tabs = text.contains('\t');
    if !has_ansi && !has_tabs {
        let mut result = String::new();
        let mut width = 0;
        for segment in graphemes(text) {
            let w = grapheme_width(segment);
            if width + w > max_width {
                break;
            }
            result.push_str(segment);
            width += w;
        }
        return Fragment {
            text: result,
            width,
        };
    }

    let mut result = String::new();
    let mut width = 0;
    let mut i = 0;
    let mut pending_ansi = String::new();
    let bytes = text.as_bytes();

    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            pending_ansi.push_str(ansi.code);
            i += ansi.length;
            continue;
        }

        if bytes[i] == b'\t' {
            if width + 3 > max_width {
                break;
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push('\t');
            width += 3;
            i += 1;
            continue;
        }

        let mut end = i;
        while end < text.len() && bytes[end] != b'\t' {
            if extract_ansi_code(text, end).is_some() {
                break;
            }
            end += 1;
        }

        for segment in graphemes(&text[i..end]) {
            let w = grapheme_width(segment);
            if width + w > max_width {
                return Fragment {
                    text: result,
                    width,
                };
            }
            if !pending_ansi.is_empty() {
                result.push_str(&pending_ansi);
                pending_ansi.clear();
            }
            result.push_str(segment);
            width += w;
        }
        i = end;
    }

    Fragment {
        text: result,
        width,
    }
}

fn finalize_truncated_result(
    prefix: &str,
    prefix_width: usize,
    ellipsis: &str,
    ellipsis_width: usize,
    max_width: usize,
    pad: bool,
) -> String {
    let reset = "\x1b[0m";
    let hyperlink_close = get_active_osc8_close(prefix);
    let visible = prefix_width + ellipsis_width;
    let result = if !ellipsis.is_empty() {
        format!("{prefix}{hyperlink_close}{reset}{ellipsis}{reset}")
    } else {
        format!("{prefix}{hyperlink_close}{reset}")
    };

    if pad {
        result + &" ".repeat(max_width.saturating_sub(visible))
    } else {
        result
    }
}

/// Terminalbreite eines einzelnen Graphem-Clusters (`utils.ts:159-217`).
fn grapheme_width(segment: &str) -> usize {
    if segment == "\t" {
        return 3;
    }

    // Manche Marks belegen auch ohne Basiszeichen Zellen.
    if is_terminal_spacing_mark_cluster(segment) {
        return segment.chars().count();
    }

    if is_zero_width_cluster(segment) {
        return 0;
    }

    if could_be_emoji(segment) && is_rgi_emoji(segment) {
        return 2;
    }

    // Sichtbaren Basis-Codepoint bestimmen.
    let base = segment.trim_start_matches(|c: char| in_ranges(LEADING_NON_PRINTING, u32::from(c)));
    let Some(first) = base.chars().next() else {
        return 0;
    };
    let cp = u32::from(first);

    // Regionalindikatoren gelten auch isoliert als 2 Zellen (Streaming-Drift).
    if (0x1f1e6..=0x1f1ff).contains(&cp) {
        return 2;
    }

    let mut width = east_asian_width(cp);

    // Nachlaufende sichtbare Codepoints, für die Terminals Zellen vergeben.
    let mut follows_mark = false;
    for c in base.chars().skip(1) {
        if in_ranges(TERMINAL_SPACING_MARK, u32::from(c)) {
            width += 1;
            follows_mark = false;
        } else if is_mark_char(c) {
            follows_mark = true;
        } else if !is_non_printing_char(c) {
            let code = u32::from(c);
            if follows_mark || (0xff00..=0xffef).contains(&code) {
                width += east_asian_width(code);
            } else if code == 0x0e33 || code == 0x0eb3 {
                width += 1;
            }
            follows_mark = false;
        }
    }

    width
}

/// Sichtbare Breite eines Strings in Terminalspalten.
pub fn visible_width(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    // Fast path: reines druckbares ASCII.
    if is_printable_ascii(text) {
        return text.len();
    }

    if let Some(cached) = WIDTH_CACHE.with(|cache| cache.borrow().0.get(text).copied()) {
        return cached;
    }

    // Normalisieren: Tabs zu drei Leerzeichen, ANSI entfernen.
    let mut clean = if text.contains('\t') {
        text.replace('\t', "   ")
    } else {
        text.to_string()
    };
    if clean.contains('\x1b') {
        let mut stripped = String::new();
        let mut i = 0;
        while i < clean.len() {
            if let Some(ansi) = extract_ansi_code(&clean, i) {
                i += ansi.length;
                continue;
            }
            let ch = clean[i..].chars().next().expect("valid char boundary");
            stripped.push(ch);
            i += ch.len_utf8();
        }
        clean = stripped;
    }

    let width = graphemes(&clean).map(grapheme_width).sum();

    WIDTH_CACHE.with(|cache| {
        let (map, order) = &mut *cache.borrow_mut();
        if map.len() >= WIDTH_CACHE_SIZE
            && let Some(first) = order.pop_front()
        {
            map.remove(&first);
        }
        map.insert(text.to_string(), width);
        order.push_back(text.to_string());
    });

    width
}

/// Entfernt ANSI-, OSC- und APC-Sequenzen, behält sichtbaren Text.
pub fn strip_terminal_sequences(text: &str) -> String {
    if !text.contains('\x1b') {
        return text.to_string();
    }
    let mut result = String::new();
    let mut i = 0;
    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            i += ansi.length;
            continue;
        }
        let ch = text[i..].chars().next().expect("valid char boundary");
        result.push(ch);
        i += ch.len_utf8();
    }
    result
}

/// Zellbereich, den das Graphem an einer sichtbaren Spalte belegt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphemeCellRange {
    pub start: usize,
    pub end: usize,
}

/// Gibt den Zellbereich des Graphems an der sichtbaren Spalte zurück.
pub fn get_grapheme_cell_range(line: &str, column: usize) -> Option<GraphemeCellRange> {
    let mut current_col = 0;
    let mut i = 0;
    while i < line.len() {
        if let Some(ansi) = extract_ansi_code(line, i) {
            i += ansi.length;
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            text_end += next_char_len(line, text_end);
        }
        for segment in graphemes(&line[i..text_end]) {
            let width = grapheme_width(segment);
            if width > 0 && column >= current_col && column < current_col + width {
                return Some(GraphemeCellRange {
                    start: current_col,
                    end: current_col + width,
                });
            }
            current_col += width;
        }
        i = text_end;
    }
    None
}

/// Länge des Zeichens an Byte-Position `pos` (Hilfsfunktion für Bytescans).
fn next_char_len(text: &str, pos: usize) -> usize {
    text[pos..].chars().next().map_or(1, char::len_utf8)
}

/// OSC-8-Hyperlink, der eine sichtbare Terminalspalte überdeckt.
pub fn get_osc8_link_at_column(line: &str, column: usize) -> Option<String> {
    let mut active_url: Option<String> = None;
    let mut current_col = 0;
    let mut i = 0;
    while i < line.len() {
        if let Some(ansi) = extract_ansi_code(line, i) {
            if let Some(url) = parse_osc8_url_for_lookup(ansi.code) {
                active_url = url;
            }
            i += ansi.length;
            continue;
        }
        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            text_end += next_char_len(line, text_end);
        }
        for segment in graphemes(&line[i..text_end]) {
            let width = if segment == "\t" {
                3
            } else {
                grapheme_width(segment)
            };
            if column >= current_col && column < current_col + width {
                return active_url;
            }
            current_col += width;
        }
        i = text_end;
    }
    None
}

/// Entspricht `/^\x1b\]8;[^;]*;([^\x07\x1b]*)(?:\x07|\x1b\\)$/` in
/// `getOsc8LinkAtColumn`: liefert `Some(None)` beim Schließen, `Some(Some(url))`
/// beim Öffnen und `None`, wenn die Sequenz kein OSC-8-Link ist.
fn parse_osc8_url_for_lookup(code: &str) -> Option<Option<String>> {
    let body = code.strip_prefix("\x1b]8;")?;
    let body = body
        .strip_suffix('\x07')
        .or_else(|| body.strip_suffix("\x1b\\"))?;
    let separator = body.find(';')?;
    let (params, url) = body.split_at(separator);
    if params.contains(';') {
        return None;
    }
    let url = &url[1..];
    if url.contains('\x07') || url.contains('\x1b') {
        return None;
    }
    Some(if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    })
}

/// Normalisiert Text für die Terminalausgabe ohne den logischen Inhalt zu ändern.
///
/// Thai-/Lao-AM-Vokale werden kompatibilitätszerlegt (gleiche Zellbreite, aber
/// keine Stale-Cell-Artefakte); sichtbare Tabs werden auf die feste Layoutbreite
/// expandiert, Tabs innerhalb von Terminalsequenzen bleiben unberührt.
///
/// In the common case — no tab, no Thai/Lao AM vowel — the input slice is
/// returned as-is. The TS original returns the same string object on that path
/// (`utils.ts:386`); the owned return the port used to have was a porting
/// artifact, not template behaviour.
pub fn normalize_terminal_output(text: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;

    let normalized: Cow<'_, str> = if text.contains('\u{0e33}') || text.contains('\u{0eb3}') {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            match c {
                '\u{0e33}' => out.push_str("\u{0e4d}\u{0e32}"),
                '\u{0eb3}' => out.push_str("\u{0ecd}\u{0eb2}"),
                _ => out.push(c),
            }
        }
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    };

    if !normalized.contains('\t') {
        return normalized;
    }

    let mut result = String::with_capacity(normalized.len());
    let mut i = 0;
    while i < normalized.len() {
        if let Some(ansi) = extract_ansi_code(&normalized, i) {
            result.push_str(ansi.code);
            i += ansi.length;
            continue;
        }
        let ch = normalized[i..].chars().next().expect("valid char boundary");
        if ch == '\t' {
            result.push_str("   ");
        } else {
            result.push(ch);
        }
        i += ch.len_utf8();
    }
    Cow::Owned(result)
}

/// Ergebnis von [`extract_ansi_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnsiCode<'a> {
    /// Die vollständige Sequenz.
    pub code: &'a str,
    /// Länge der Sequenz in Bytes.
    pub length: usize,
}

/// Extrahiert eine ANSI-Escape-Sequenz an der Byte-Position `pos`.
///
/// **CSI wird nur mit den Finalbytes `m G K H J` erkannt** — genau wie in TS
/// (`utils.ts:399-437`); andere CSI-Sequenzen zählen zur sichtbaren Breite.
pub fn extract_ansi_code(text: &str, pos: usize) -> Option<AnsiCode<'_>> {
    let bytes = text.as_bytes();
    if pos >= text.len() || bytes[pos] != 0x1b {
        return None;
    }

    let next = bytes.get(pos + 1).copied();

    // CSI: ESC [ … m/G/K/H/J
    if next == Some(b'[') {
        let mut j = pos + 2;
        while j < text.len() && !matches!(bytes[j], b'm' | b'G' | b'K' | b'H' | b'J') {
            j += 1;
        }
        if j < text.len() {
            return Some(AnsiCode {
                code: &text[pos..j + 1],
                length: j + 1 - pos,
            });
        }
        return None;
    }

    // OSC: ESC ] … BEL oder ST (ESC \), APC: ESC _ … BEL oder ST
    if next == Some(b']') || next == Some(b'_') {
        let mut j = pos + 2;
        while j < text.len() {
            if bytes[j] == 0x07 {
                return Some(AnsiCode {
                    code: &text[pos..j + 1],
                    length: j + 1 - pos,
                });
            }
            if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                return Some(AnsiCode {
                    code: &text[pos..j + 2],
                    length: j + 2 - pos,
                });
            }
            j += 1;
        }
        return None;
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Osc8Terminator {
    Bel,
    St,
}

impl Osc8Terminator {
    fn as_str(self) -> &'static str {
        match self {
            Osc8Terminator::Bel => "\x07",
            Osc8Terminator::St => "\x1b\\",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveHyperlink {
    params: String,
    url: String,
    terminator: Osc8Terminator,
}

/// `undefined` (kein OSC 8) → `None`; `null` (Schließen) → `Some(None)`.
fn parse_osc8_hyperlink(ansi_code: &str) -> Option<Option<ActiveHyperlink>> {
    if !ansi_code.starts_with("\x1b]8;") {
        return None;
    }
    let terminator = if ansi_code.ends_with('\x07') {
        Osc8Terminator::Bel
    } else {
        Osc8Terminator::St
    };
    let body = match terminator {
        Osc8Terminator::Bel => &ansi_code[4..ansi_code.len() - 1],
        Osc8Terminator::St => &ansi_code[4..ansi_code.len() - 2],
    };
    let separator = body.find(';')?;
    let params = &body[..separator];
    let url = &body[separator + 1..];
    if url.is_empty() {
        return Some(None);
    }
    Some(Some(ActiveHyperlink {
        params: params.to_string(),
        url: url.to_string(),
        terminator,
    }))
}

fn format_osc8_hyperlink(hyperlink: &ActiveHyperlink) -> String {
    format!(
        "\x1b]8;{};{}{}",
        hyperlink.params,
        hyperlink.url,
        hyperlink.terminator.as_str()
    )
}

fn format_osc8_close(terminator: Osc8Terminator) -> String {
    format!("\x1b]8;;{}", terminator.as_str())
}

fn get_active_osc8_close(prefix: &str) -> String {
    if !prefix.contains("\x1b]8;") {
        return String::new();
    }

    let mut active: Option<ActiveHyperlink> = None;
    let mut i = 0;
    while i < prefix.len() {
        if let Some(ansi) = extract_ansi_code(prefix, i) {
            if let Some(hyperlink) = parse_osc8_hyperlink(ansi.code) {
                active = hyperlink;
            }
            i += ansi.length;
        } else {
            i += next_char_len(prefix, i);
        }
    }
    active.map_or_else(String::new, |h| format_osc8_close(h.terminator))
}

/// Verfolgt aktive SGR-Codes, um Stil über Zeilenumbrüche zu erhalten.
#[derive(Debug, Default, Clone)]
pub(crate) struct AnsiCodeTracker {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    fg_color: Option<String>,
    bg_color: Option<String>,
    active_hyperlink: Option<ActiveHyperlink>,
}

impl AnsiCodeTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn process(&mut self, ansi_code: &str) {
        // OSC 8: Terminator bleibt erhalten — manche Terminals machen nur
        // BEL-terminierte Links klickbar (OAuth-Login-URLs).
        if let Some(hyperlink) = parse_osc8_hyperlink(ansi_code) {
            self.active_hyperlink = hyperlink;
            return;
        }

        if !ansi_code.ends_with('m') {
            return;
        }

        let Some(params) = ansi_code
            .strip_prefix("\x1b[")
            .and_then(|rest| rest.strip_suffix('m'))
        else {
            return;
        };
        if !params.chars().all(|c| c.is_ascii_digit() || c == ';') {
            return;
        }

        if params.is_empty() || params == "0" {
            self.reset();
            return;
        }

        let parts: Vec<&str> = params.split(';').collect();
        let mut i = 0;
        while i < parts.len() {
            let code: i64 = parts[i].parse().unwrap_or(i64::MIN);

            // 256-Farben und RGB verbrauchen mehrere Parameter.
            if code == 38 || code == 48 {
                if parts.get(i + 1) == Some(&"5") && parts.get(i + 2).is_some() {
                    let color = format!("{};{};{}", parts[i], parts[i + 1], parts[i + 2]);
                    if code == 38 {
                        self.fg_color = Some(color);
                    } else {
                        self.bg_color = Some(color);
                    }
                    i += 3;
                    continue;
                } else if parts.get(i + 1) == Some(&"2") && parts.get(i + 4).is_some() {
                    let color = format!(
                        "{};{};{};{};{}",
                        parts[i],
                        parts[i + 1],
                        parts[i + 2],
                        parts[i + 3],
                        parts[i + 4]
                    );
                    if code == 38 {
                        self.fg_color = Some(color);
                    } else {
                        self.bg_color = Some(color);
                    }
                    i += 5;
                    continue;
                }
            }

            match code {
                0 => self.reset(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 => self.blink = true,
                7 => self.inverse = true,
                8 => self.hidden = true,
                9 => self.strikethrough = true,
                21 => self.bold = false,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.inverse = false,
                28 => self.hidden = false,
                29 => self.strikethrough = false,
                39 => self.fg_color = None,
                49 => self.bg_color = None,
                _ => {
                    if (30..=37).contains(&code) || (90..=97).contains(&code) {
                        self.fg_color = Some(code.to_string());
                    } else if (40..=47).contains(&code) || (100..=107).contains(&code) {
                        self.bg_color = Some(code.to_string());
                    }
                }
            }
            i += 1;
        }
    }

    fn reset(&mut self) {
        self.bold = false;
        self.dim = false;
        self.italic = false;
        self.underline = false;
        self.blink = false;
        self.inverse = false;
        self.hidden = false;
        self.strikethrough = false;
        self.fg_color = None;
        self.bg_color = None;
        // SGR-Reset berührt den OSC-8-Zustand nicht.
    }

    #[allow(dead_code)] // 1:1-Port des gepoolten Trackers; hier lokal erzeugt.
    pub(crate) fn clear(&mut self) {
        self.reset();
        self.active_hyperlink = None;
    }

    pub(crate) fn get_active_codes(&self) -> String {
        let mut codes: Vec<&str> = Vec::new();
        if self.bold {
            codes.push("1");
        }
        if self.dim {
            codes.push("2");
        }
        if self.italic {
            codes.push("3");
        }
        if self.underline {
            codes.push("4");
        }
        if self.blink {
            codes.push("5");
        }
        if self.inverse {
            codes.push("7");
        }
        if self.hidden {
            codes.push("8");
        }
        if self.strikethrough {
            codes.push("9");
        }
        if let Some(fg) = &self.fg_color {
            codes.push(fg);
        }
        if let Some(bg) = &self.bg_color {
            codes.push(bg);
        }

        let mut result = if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        };
        if let Some(hyperlink) = &self.active_hyperlink {
            result.push_str(&format_osc8_hyperlink(hyperlink));
        }
        result
    }

    #[allow(dead_code)] // 1:1-Port; Konsument ist der Markdown-Renderer (Task 11).
    pub(crate) fn has_active_codes(&self) -> bool {
        self.bold
            || self.dim
            || self.italic
            || self.underline
            || self.blink
            || self.inverse
            || self.hidden
            || self.strikethrough
            || self.fg_color.is_some()
            || self.bg_color.is_some()
            || self.active_hyperlink.is_some()
    }

    /// Reset-Codes für Attribute, die am Zeilenende geschlossen werden müssen.
    pub(crate) fn get_line_end_reset(&self) -> String {
        let mut result = String::new();
        if self.underline {
            result.push_str("\x1b[24m");
        }
        if let Some(hyperlink) = &self.active_hyperlink {
            result.push_str(&format_osc8_close(hyperlink.terminator));
        }
        result
    }
}

fn update_tracker_from_text(text: &str, tracker: &mut AnsiCodeTracker) {
    let mut i = 0;
    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            tracker.process(ansi.code);
            i += ansi.length;
        } else {
            i += next_char_len(text, i);
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum TokenKind {
    Space,
    Word,
}

/// Zerlegt Text in Tokens, ANSI-Codes bleiben am folgenden Zeichen kleben.
fn split_into_tokens_with_ansi(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut current_kind: Option<TokenKind> = None;
    let mut i = 0;

    while i < text.len() {
        if let Some(ansi) = extract_ansi_code(text, i) {
            pending_ansi.push_str(ansi.code);
            i += ansi.length;
            continue;
        }

        let mut end = i;
        while end < text.len() && extract_ansi_code(text, end).is_none() {
            end += next_char_len(text, end);
        }

        for segment in graphemes(&text[i..end]) {
            let segment_is_space = segment == " ";
            if !segment_is_space && is_cjk_break(segment) {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                    current_kind = None;
                }
                let mut token = std::mem::take(&mut pending_ansi);
                token.push_str(segment);
                tokens.push(token);
                continue;
            }

            let segment_kind = if segment_is_space {
                TokenKind::Space
            } else {
                TokenKind::Word
            };
            if !current.is_empty() && current_kind != Some(segment_kind) {
                tokens.push(std::mem::take(&mut current));
            }

            if !pending_ansi.is_empty() {
                current.push_str(&pending_ansi);
                pending_ansi.clear();
            }

            current_kind = Some(segment_kind);
            current.push_str(segment);
        }

        i = end;
    }

    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = std::mem::take(&mut pending_ansi);
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Wie `String.prototype.trimEnd()`: Unicode-WhiteSpace plus U+FEFF.
fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// Bricht Text auf `width` sichtbare Spalten um, ANSI-Codes bleiben erhalten.
///
/// Nur Wortumbruch — **kein** Padding, **keine** Hintergrundfarben. Die
/// Ergebniszeilen sind nicht auf `width` aufgefüllt.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    // Zeilenenden einzeln behandeln, ANSI-Zustand über Zeilen mitführen.
    let input_lines = split_lines(text);
    let mut result: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();

    for input_line in input_lines {
        let prefix = if result.is_empty() {
            String::new()
        } else {
            tracker.get_active_codes()
        };
        for wrapped_line in wrap_single_line(&(prefix + input_line), width) {
            result.push(wrapped_line);
        }
        update_tracker_from_text(input_line, &mut tracker);
    }

    if result.is_empty() {
        vec![String::new()]
    } else {
        result
    }
}

/// `text.split(/\r\n|\r|\n/)`.
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        match bytes[i] {
            b'\r' => {
                lines.push(&text[start..i]);
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 2;
                } else {
                    i += 1;
                }
                start = i;
            }
            b'\n' => {
                lines.push(&text[start..i]);
                i += 1;
                start = i;
            }
            _ => i += next_char_len(text, i),
        }
    }
    lines.push(&text[start..]);
    lines
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    if visible_width(line) <= width {
        return vec![line.to_string()];
    }

    let mut wrapped: Vec<String> = Vec::new();
    let mut tracker = AnsiCodeTracker::new();
    let tokens = split_into_tokens_with_ansi(line);

    let mut current_line = String::new();
    let mut current_visible_length = 0;

    for token in &tokens {
        let token_visible_length = visible_width(token);
        let is_whitespace = token.trim().is_empty();

        // Token selbst zu lang — zeichenweise brechen.
        if token_visible_length > width && !is_whitespace {
            if !current_line.is_empty() {
                let line_end_reset = tracker.get_line_end_reset();
                if !line_end_reset.is_empty() {
                    current_line.push_str(&line_end_reset);
                }
                wrapped.push(std::mem::take(&mut current_line));
            }

            let broken = break_long_word(token, width, &mut tracker);
            for line in &broken[..broken.len() - 1] {
                wrapped.push(line.clone());
            }
            current_line = broken[broken.len() - 1].clone();
            current_visible_length = visible_width(&current_line);
            continue;
        }

        let total_needed = current_visible_length + token_visible_length;

        if total_needed > width && current_visible_length > 0 {
            let mut line_to_wrap = js_trim_end(&current_line).to_string();
            let line_end_reset = tracker.get_line_end_reset();
            if !line_end_reset.is_empty() {
                line_to_wrap.push_str(&line_end_reset);
            }
            wrapped.push(line_to_wrap);
            if is_whitespace {
                current_line = tracker.get_active_codes();
                current_visible_length = 0;
            } else {
                current_line = tracker.get_active_codes() + token;
                current_visible_length = token_visible_length;
            }
        } else {
            current_line.push_str(token);
            current_visible_length += token_visible_length;
        }

        update_tracker_from_text(token, &mut tracker);
    }

    if !current_line.is_empty() {
        wrapped.push(current_line);
    }

    if wrapped.is_empty() {
        vec![String::new()]
    } else {
        wrapped
            .into_iter()
            .map(|line| js_trim_end(&line).to_string())
            .collect()
    }
}

/// Satzzeichen wie `PUNCTUATION_REGEX` in `utils.ts:936`.
pub fn is_punctuation_char(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c,
            '(' | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '.'
                | ','
                | ';'
                | ':'
                | '\''
                | '"'
                | '!'
                | '?'
                | '+'
                | '-'
                | '='
                | '*'
                | '/'
                | '\\'
                | '|'
                | '&'
                | '%'
                | '^'
                | '$'
                | '#'
                | '@'
                | '~'
                | '`'
        )
    })
}

/// Entspricht `/\s/.test(char)` in JavaScript.
pub fn is_whitespace_char(text: &str) -> bool {
    text.chars().any(|c| c.is_whitespace() || c == '\u{feff}')
}

fn break_long_word(word: &str, width: usize, tracker: &mut AnsiCodeTracker) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = tracker.get_active_codes();
    let mut current_width = 0;

    enum Segment<'a> {
        Ansi(&'a str),
        Grapheme(&'a str),
    }

    let mut segments: Vec<Segment<'_>> = Vec::new();
    let mut i = 0;
    while i < word.len() {
        if let Some(ansi) = extract_ansi_code(word, i) {
            segments.push(Segment::Ansi(ansi.code));
            i += ansi.length;
        } else {
            let mut end = i;
            while end < word.len() {
                if extract_ansi_code(word, end).is_some() {
                    break;
                }
                end += next_char_len(word, end);
            }
            for grapheme in graphemes(&word[i..end]) {
                segments.push(Segment::Grapheme(grapheme));
            }
            i = end;
        }
    }

    for segment in segments {
        match segment {
            Segment::Ansi(code) => {
                current_line.push_str(code);
                tracker.process(code);
            }
            Segment::Grapheme(grapheme) => {
                if grapheme.is_empty() {
                    continue;
                }
                let grapheme_width = visible_width(grapheme);

                if current_width + grapheme_width > width {
                    let line_end_reset = tracker.get_line_end_reset();
                    if !line_end_reset.is_empty() {
                        current_line.push_str(&line_end_reset);
                    }
                    lines.push(std::mem::take(&mut current_line));
                    current_line = tracker.get_active_codes();
                    current_width = 0;
                }

                current_line.push_str(grapheme);
                current_width += grapheme_width;
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

/// Wendet eine Hintergrundfunktion auf eine auf `width` aufgefüllte Zeile an.
pub fn apply_background_to_line(
    line: &str,
    width: usize,
    bg_fn: impl Fn(&str) -> String,
) -> String {
    let visible_len = visible_width(line);
    let padding = " ".repeat(width.saturating_sub(visible_len));
    bg_fn(&(line.to_string() + &padding))
}

/// Kürzt Text auf `max_width` sichtbare Spalten mit Ellipse `"..."`.
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    truncate_to_width_opts(text, max_width, "...", false)
}

/// Wie [`truncate_to_width`], mit wählbarer Ellipse und optionalem Padding.
pub fn truncate_to_width_opts(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let text_width = visible_width(text);
        if text_width <= max_width {
            return if pad {
                text.to_string() + &" ".repeat(max_width - text_width)
            } else {
                text.to_string()
            };
        }

        let clipped = truncate_fragment_to_width(ellipsis, max_width);
        if clipped.width == 0 {
            return if pad {
                " ".repeat(max_width)
            } else {
                String::new()
            };
        }
        return finalize_truncated_result("", 0, &clipped.text, clipped.width, max_width, pad);
    }

    if is_printable_ascii(text) {
        if text.len() <= max_width {
            return if pad {
                text.to_string() + &" ".repeat(max_width - text.len())
            } else {
                text.to_string()
            };
        }
        let target_width = max_width - ellipsis_width;
        return finalize_truncated_result(
            &text[..target_width],
            target_width,
            ellipsis,
            ellipsis_width,
            max_width,
            pad,
        );
    }

    let target_width = max_width - ellipsis_width;
    let mut result = String::new();
    let mut pending_ansi = String::new();
    let mut visible_so_far = 0;
    let mut kept_width = 0;
    let mut keep_contiguous_prefix = true;
    let mut overflowed = false;
    let exhausted_input;
    let has_ansi = text.contains('\x1b');
    let has_tabs = text.contains('\t');

    if !has_ansi && !has_tabs {
        for segment in graphemes(text) {
            let width = grapheme_width(segment);
            if keep_contiguous_prefix && kept_width + width <= target_width {
                result.push_str(segment);
                kept_width += width;
            } else {
                keep_contiguous_prefix = false;
            }
            visible_so_far += width;
            if visible_so_far > max_width {
                overflowed = true;
                break;
            }
        }
        exhausted_input = !overflowed;
    } else {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < text.len() {
            if let Some(ansi) = extract_ansi_code(text, i) {
                pending_ansi.push_str(ansi.code);
                i += ansi.length;
                continue;
            }

            if bytes[i] == b'\t' {
                if keep_contiguous_prefix && kept_width + 3 <= target_width {
                    if !pending_ansi.is_empty() {
                        result.push_str(&pending_ansi);
                        pending_ansi.clear();
                    }
                    result.push('\t');
                    kept_width += 3;
                } else {
                    keep_contiguous_prefix = false;
                    pending_ansi.clear();
                }
                visible_so_far += 3;
                if visible_so_far > max_width {
                    overflowed = true;
                    break;
                }
                i += 1;
                continue;
            }

            let mut end = i;
            while end < text.len() && bytes[end] != b'\t' {
                if extract_ansi_code(text, end).is_some() {
                    break;
                }
                end += next_char_len(text, end);
            }

            for segment in graphemes(&text[i..end]) {
                let width = grapheme_width(segment);
                if keep_contiguous_prefix && kept_width + width <= target_width {
                    if !pending_ansi.is_empty() {
                        result.push_str(&pending_ansi);
                        pending_ansi.clear();
                    }
                    result.push_str(segment);
                    kept_width += width;
                } else {
                    keep_contiguous_prefix = false;
                    pending_ansi.clear();
                }

                visible_so_far += width;
                if visible_so_far > max_width {
                    overflowed = true;
                    break;
                }
            }
            if overflowed {
                break;
            }
            i = end;
        }
        exhausted_input = i >= text.len();
    }

    if !overflowed && exhausted_input {
        return if pad {
            text.to_string() + &" ".repeat(max_width.saturating_sub(visible_so_far))
        } else {
            text.to_string()
        };
    }

    finalize_truncated_result(
        &result,
        kept_width,
        ellipsis,
        ellipsis_width,
        max_width,
        pad,
    )
}

/// Schneidet einen Bereich sichtbarer Spalten aus einer Zeile.
///
/// `strict`: breite Zeichen an der Grenze, die über den Bereich hinausragen,
/// werden ausgeschlossen.
pub fn slice_by_column(line: &str, start_col: usize, length: usize, strict: bool) -> String {
    slice_with_width(line, start_col, length, strict).text
}

/// Ergebnis von [`slice_with_width`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceWithWidth {
    /// Der ausgeschnittene Text inklusive ANSI-Sequenzen.
    pub text: String,
    /// Sichtbare Breite von `text`.
    pub width: usize,
}

/// Wie [`slice_by_column`], liefert zusätzlich die sichtbare Breite.
pub fn slice_with_width(
    line: &str,
    start_col: usize,
    length: usize,
    strict: bool,
) -> SliceWithWidth {
    if length == 0 {
        return SliceWithWidth {
            text: String::new(),
            width: 0,
        };
    }
    let end_col = start_col + length;
    let mut result = String::new();
    let mut result_width = 0;
    let mut current_col = 0;
    let mut i = 0;
    let mut pending_ansi = String::new();

    while i < line.len() {
        if let Some(ansi) = extract_ansi_code(line, i) {
            if current_col >= start_col && current_col < end_col {
                result.push_str(ansi.code);
            } else if current_col < start_col {
                pending_ansi.push_str(ansi.code);
            }
            i += ansi.length;
            continue;
        }

        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            text_end += next_char_len(line, text_end);
        }

        for segment in graphemes(&line[i..text_end]) {
            let w = grapheme_width(segment);
            let in_range = current_col >= start_col && current_col < end_col;
            let fits = !strict || current_col + w <= end_col;
            if in_range && fits {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push_str(segment);
                result_width += w;
            }
            current_col += w;
            if current_col >= end_col {
                break;
            }
        }
        i = text_end;
        if current_col >= end_col {
            break;
        }
    }

    SliceWithWidth {
        text: result,
        width: result_width,
    }
}

/// Ergebnis von [`extract_segments`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSegments {
    /// Inhalt vor dem Overlay-Bereich.
    pub before: String,
    /// Sichtbare Breite von `before`.
    pub before_width: usize,
    /// Inhalt nach dem Overlay-Bereich (erbt den Stil von davor).
    pub after: String,
    /// Sichtbare Breite von `after`.
    pub after_width: usize,
}

/// Extrahiert "before"- und "after"-Segmente einer Zeile in einem Durchlauf.
///
/// Wird für die Overlay-Komposition gebraucht. Der Stil vor dem Overlay wird an
/// den "after"-Teil vererbt. Die TS-Vorlage nutzt dafür einen global gepoolten
/// Tracker; hier ist er lokal (Abweichungsklasse 1 — `clear()` beim Eintritt
/// macht das Verhalten identisch, ohne globalen Zustand).
pub fn extract_segments(
    line: &str,
    before_end: usize,
    after_start: usize,
    after_len: usize,
    strict_after: bool,
) -> ExtractedSegments {
    let mut before = String::new();
    let mut before_width = 0;
    let mut after = String::new();
    let mut after_width = 0;
    let mut current_col = 0;
    let mut i = 0;
    let mut pending_ansi_before = String::new();
    let mut after_started = false;
    let after_end = after_start + after_len;

    let mut style_tracker = AnsiCodeTracker::new();

    while i < line.len() {
        if let Some(ansi) = extract_ansi_code(line, i) {
            style_tracker.process(ansi.code);
            if current_col < before_end {
                pending_ansi_before.push_str(ansi.code);
            } else if current_col >= after_start && current_col < after_end && after_started {
                after.push_str(ansi.code);
            }
            i += ansi.length;
            continue;
        }

        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            text_end += next_char_len(line, text_end);
        }

        for segment in graphemes(&line[i..text_end]) {
            let w = grapheme_width(segment);

            if current_col < before_end && current_col + w <= before_end {
                if !pending_ansi_before.is_empty() {
                    before.push_str(&pending_ansi_before);
                    pending_ansi_before.clear();
                }
                before.push_str(segment);
                before_width += w;
            } else if current_col >= after_start && current_col < after_end {
                let fits = !strict_after || current_col + w <= after_end;
                if fits {
                    if !after_started {
                        after.push_str(&style_tracker.get_active_codes());
                        after_started = true;
                    }
                    after.push_str(segment);
                    after_width += w;
                }
            }

            current_col += w;
            let done = if after_len == 0 {
                current_col >= before_end
            } else {
                current_col >= after_end
            };
            if done {
                break;
            }
        }
        i = text_end;
        let done = if after_len == 0 {
            current_col >= before_end
        } else {
            current_col >= after_end
        };
        if done {
            break;
        }
    }

    ExtractedSegments {
        before,
        before_width,
        after,
        after_width,
    }
}
