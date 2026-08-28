//! A band of light travelling through a line of text.
//! Stands in for a spinner. A spinner says "something is happening" with a
//! glyph that has nothing to do with the message beside it; the band says the
//! same thing using the message itself, so the eye is drawn to the words rather
//! than away from them.
//! The band pulls the text toward the background rather than brightening it.
//! Brightening needs a colour beyond the text, which on a light terminal does
//! not exist; the background is always there and lies in opposite directions in
//! the two themes, so the same rule reads correctly in both.

use std::fmt::Write as _;
use std::time::Duration;

/// How often the band moves.
/// A sweep that advances five times a second reads as a series of jumps rather
/// than as motion — the band moves several characters between frames, which is
/// what makes it look like it is lagging behind rather than travelling. Thirty
/// frames a second is the rate at which the step falls below one character.
/// Only the status row repaints, and only while something is running.
pub const SHIMMER_FRAME_MS: u64 = 32;

/// How long one pass across the text takes.
const SWEEP_SECONDS: f32 = 2.0;

/// Blank positions before and after the text, so the band leaves and enters
/// rather than appearing at the first character.
const PADDING: usize = 10;

/// Half the width of the band, in characters.
const BAND_HALF_WIDTH: f32 = 5.0;

/// How far into the fade colour the centre of the band reaches. Short of 1.0
/// so the text never quite disappears.
const MAX_FADE: f32 = 0.9;

/// The two ends of the fade: the colour the text has, and the colour the band
/// pulls it toward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShimmerPalette {
    pub base: (u8, u8, u8),
    pub fade: (u8, u8, u8),
}

/// The colour the band should pull `base` toward: the background behind it.
/// Derived from the text rather than asked for, because the two themes differ
/// in exactly this and nothing else: light text sits on a dark ground and dark
/// text on a light one. Taking the direction from the text's own brightness
/// therefore reverses with the theme without anything having to be configured,
/// and a theme with an unusual palette still gets the direction that lowers
/// contrast rather than one that raises it past the text.
pub fn fade_toward_background(base: (u8, u8, u8)) -> (u8, u8, u8) {
    // Rec. 601 luma: green carries most of the perceived brightness, blue least.
    let luma = 0.299 * base.0 as f32 + 0.587 * base.1 as f32 + 0.114 * base.2 as f32;
    if luma > 127.5 {
        (0, 0, 0)
    } else {
        (255, 255, 255)
    }
}

fn blend(from: (u8, u8, u8), to: (u8, u8, u8), amount: f32) -> (u8, u8, u8) {
    let mix = |from: u8, to: u8| {
        (from as f32 * (1.0 - amount) + to as f32 * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

/// How far into the fade the band pushes the character at `index`.
fn fade_at(index: usize, position: f32) -> f32 {
    let distance = ((index + PADDING) as f32 - position).abs();
    if distance > BAND_HALF_WIDTH {
        return 0.0;
    }
    // A raised cosine, so the band has no edge to catch the eye on.
    let angle = std::f32::consts::PI * (distance / BAND_HALF_WIDTH);
    0.5 * (1.0 + angle.cos()) * MAX_FADE
}

/// Renders `text` with the band at the position `elapsed` puts it.
/// The sweep is derived from elapsed time rather than counted in frames, so a
/// dropped frame shifts nothing: the band is where the clock says it is.
pub fn shimmer(text: &str, palette: ShimmerPalette, elapsed: Duration) -> String {
    let characters: Vec<char> = text.chars().collect();
    if characters.is_empty() {
        return String::new();
    }

    let period = (characters.len() + PADDING * 2) as f32;
    let position = (elapsed.as_secs_f32() % SWEEP_SECONDS) / SWEEP_SECONDS * period;

    let mut rendered = String::with_capacity(text.len() * 2);
    let mut current: Option<(u8, u8, u8)> = None;
    for (index, character) in characters.iter().enumerate() {
        let color = blend(palette.base, palette.fade, fade_at(index, position));
        // One escape per run of equal colour, not per character: outside the
        // band that is a single sequence for the whole rest of the line.
        if current != Some(color) {
            let _ = write!(rendered, "\x1b[38;2;{};{};{}m", color.0, color.1, color.2);
            current = Some(color);
        }
        rendered.push(*character);
    }
    rendered.push_str("\x1b[39m");
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    const PALETTE: ShimmerPalette = ShimmerPalette {
        base: (200, 200, 200),
        fade: (100, 100, 100),
    };

    fn visible(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(character) = chars.next() {
            if character == '\x1b' {
                for escaped in chars.by_ref() {
                    if escaped == 'm' {
                        break;
                    }
                }
                continue;
            }
            out.push(character);
        }
        out
    }

    #[test]
    fn keeps_the_text_it_was_given() {
        let rendered = shimmer("Working on it", PALETTE, Duration::from_millis(400));
        assert_eq!(visible(&rendered), "Working on it");
    }

    #[test]
    fn renders_nothing_for_nothing() {
        assert!(shimmer("", PALETTE, Duration::ZERO).is_empty());
    }

    #[test]
    fn the_band_moves_with_the_clock() {
        let first = shimmer("Working on it", PALETTE, Duration::from_millis(0));
        let later = shimmer("Working on it", PALETTE, Duration::from_millis(700));
        assert_ne!(first, later);
    }

    #[test]
    fn comes_back_to_where_it_started() {
        let first = shimmer("Working", PALETTE, Duration::from_millis(0));
        let looped = shimmer("Working", PALETTE, Duration::from_millis(2000));
        assert_eq!(first, looped);
    }

    /// Away from the band the text keeps its own colour, and says so once
    /// rather than per character.
    #[test]
    fn colours_a_run_with_one_escape() {
        let rendered = shimmer("aaaaaaaaaaaaaaaaaaaa", PALETTE, Duration::ZERO);
        let escapes = rendered.matches("\x1b[38;2;").count();
        assert!(escapes < 20, "one escape per character: {escapes}");
        assert!(rendered.contains("\x1b[38;2;200;200;200m"));
    }

    #[test]
    fn the_band_travels_less_than_a_character_per_frame() {
        // A step of several characters between frames reads as jumping rather
        // than as motion, which is what "laggy" looks like.
        let shortest_message = 8.0;
        let period = shortest_message + PADDING as f32 * 2.0;
        let per_frame = period * (SHIMMER_FRAME_MS as f32 / 1000.0) / SWEEP_SECONDS;
        assert!(per_frame < 1.0, "{per_frame} characters per frame");
    }

    #[test]
    fn the_fade_reverses_with_the_theme() {
        // Light text sits on a dark ground and dark text on a light one, so the
        // direction that lowers contrast is the opposite one in each.
        assert_eq!(fade_toward_background((212, 212, 212)), (0, 0, 0));
        assert_eq!(fade_toward_background((31, 35, 40)), (255, 255, 255));
        // Judged by brightness, not by the largest channel: this blue is dark
        // despite a full blue channel.
        assert_eq!(fade_toward_background((0, 0, 255)), (255, 255, 255));
    }

    #[test]
    fn never_fades_the_text_all_the_way_out() {
        // The centre of the band, wherever it falls, keeps some of the base.
        for index in 0..40 {
            let fade = fade_at(index, index as f32 + PADDING as f32);
            assert!(fade <= MAX_FADE, "{fade}");
        }
        let centre = blend(PALETTE.base, PALETTE.fade, MAX_FADE);
        assert_ne!(centre, PALETTE.fade);
    }
}
