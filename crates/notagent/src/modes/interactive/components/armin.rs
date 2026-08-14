//! Armin says hi! A fun easter egg with animated XBM art.
//!
//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/armin.ts` (382 LOC).

use std::time::{Duration, Instant};

use rand::RngExt as _;

use notagent_tui::tui::Component;

use crate::modes::interactive::theme::theme::{ThemeColor, theme};

// XBM image: 31x36 pixels, LSB first, 1=background, 0=foreground
const WIDTH: usize = 31;
const HEIGHT: usize = 36;
const BITS: [u8; 144] = [
    0xff, 0xff, 0xff, 0x7f, 0xff, 0xf0, 0xff, 0x7f, 0xff, 0xed, 0xff, 0x7f, 0xff, 0xdb, 0xff, 0x7f,
    0xff, 0xb7, 0xff, 0x7f, 0xff, 0x77, 0xfe, 0x7f, 0x3f, 0xf8, 0xfe, 0x7f, 0xdf, 0xff, 0xfe, 0x7f,
    0xdf, 0x3f, 0xfc, 0x7f, 0x9f, 0xc3, 0xfb, 0x7f, 0x6f, 0xfc, 0xf4, 0x7f, 0xf7, 0x0f, 0xf7, 0x7f,
    0xf7, 0xff, 0xf7, 0x7f, 0xf7, 0xff, 0xe3, 0x7f, 0xf7, 0x07, 0xe8, 0x7f, 0xef, 0xf8, 0x67, 0x70,
    0x0f, 0xff, 0xbb, 0x6f, 0xf1, 0x00, 0xd0, 0x5b, 0xfd, 0x3f, 0xec, 0x53, 0xc1, 0xff, 0xef, 0x57,
    0x9f, 0xfd, 0xee, 0x5f, 0x9f, 0xfc, 0xae, 0x5f, 0x1f, 0x78, 0xac, 0x5f, 0x3f, 0x00, 0x50, 0x6c,
    0x7f, 0x00, 0xdc, 0x77, 0xff, 0xc0, 0x3f, 0x78, 0xff, 0x01, 0xf8, 0x7f, 0xff, 0x03, 0x9c, 0x78,
    0xff, 0x07, 0x8c, 0x7c, 0xff, 0x0f, 0xce, 0x78, 0xff, 0xff, 0xcf, 0x7f, 0xff, 0xff, 0xcf, 0x78,
    0xff, 0xff, 0xdf, 0x78, 0xff, 0xff, 0xdf, 0x7d, 0xff, 0xff, 0x3f, 0x7e, 0xff, 0xff, 0xff, 0x7f,
];

const BYTES_PER_ROW: usize = WIDTH.div_ceil(8);
/// Half-block rendering
const DISPLAY_HEIGHT: usize = HEIGHT.div_ceil(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Typewriter,
    Scanline,
    Rain,
    Fade,
    Crt,
    Glitch,
    Dissolve,
}

const EFFECTS: [Effect; 7] = [
    Effect::Typewriter,
    Effect::Scanline,
    Effect::Rain,
    Effect::Fade,
    Effect::Crt,
    Effect::Glitch,
    Effect::Dissolve,
];

/// Get pixel at (x, y): true = foreground, false = background
fn get_pixel(x: usize, y: usize) -> bool {
    if y >= HEIGHT {
        return false;
    }
    let byte_index = y * BYTES_PER_ROW + x / 8;
    let bit_index = x % 8;
    ((BITS[byte_index] >> bit_index) & 1) == 0
}

/// Get the character for a cell (2 vertical pixels packed)
fn get_char(x: usize, row: usize) -> char {
    let upper = get_pixel(x, row * 2);
    let lower = get_pixel(x, row * 2 + 1);
    match (upper, lower) {
        (true, true) => '█',
        (true, false) => '▀',
        (false, true) => '▄',
        (false, false) => ' ',
    }
}

/// Build the final image grid
fn build_final_grid() -> Vec<Vec<char>> {
    (0..DISPLAY_HEIGHT)
        .map(|row| (0..WIDTH).map(|x| get_char(x, row)).collect())
        .collect()
}

fn create_empty_grid() -> Vec<Vec<char>> {
    vec![vec![' '; WIDTH]; DISPLAY_HEIGHT]
}

#[derive(Clone, Copy)]
struct Drop {
    y: isize,
    settled: usize,
}

enum EffectState {
    Typewriter {
        pos: usize,
    },
    Scanline {
        row: usize,
    },
    Rain {
        drops: Vec<Drop>,
    },
    Positions {
        positions: Vec<(usize, usize)>,
        idx: usize,
    },
    Crt {
        expansion: usize,
    },
    Glitch {
        phase: usize,
        glitch_frames: usize,
    },
}

fn shuffled_positions() -> Vec<(usize, usize)> {
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for row in 0..DISPLAY_HEIGHT {
        for x in 0..WIDTH {
            positions.push((row, x));
        }
    }
    // Fisher-Yates shuffle
    let mut rng = rand::rng();
    for index in (1..positions.len()).rev() {
        let other = rng.random_range(0..=index);
        positions.swap(index, other);
    }
    positions
}

/// The easter egg component.
///
/// The `setInterval` animation is polled like every other timer of this port:
/// [`ArminComponent::deadline`] says when the next frame is due and
/// [`ArminComponent::tick`] advances it.
pub struct ArminComponent {
    effect: Effect,
    final_grid: Vec<Vec<char>>,
    current_grid: Vec<Vec<char>>,
    effect_state: EffectState,
    cached_lines: Vec<String>,
    cached_width: usize,
    grid_version: u64,
    cached_version: i64,
    frame_interval: Duration,
    next_frame: Option<Instant>,
}

impl Default for ArminComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl ArminComponent {
    /// New component; the animation starts right away with a random effect.
    pub fn new() -> Self {
        let effect = EFFECTS[rand::rng().random_range(0..EFFECTS.len())];
        Self::with_effect(effect)
    }

    fn with_effect(effect: Effect) -> Self {
        let final_grid = build_final_grid();
        let mut component = Self {
            effect,
            final_grid,
            current_grid: create_empty_grid(),
            effect_state: EffectState::Typewriter { pos: 0 },
            cached_lines: Vec::new(),
            cached_width: 0,
            grid_version: 0,
            cached_version: -1,
            // `1000 / fps`
            frame_interval: Duration::from_secs_f64(
                1.0 / if effect == Effect::Glitch { 60.0 } else { 30.0 },
            ),
            next_frame: None,
        };
        component.init_effect();
        component.next_frame = Some(Instant::now() + component.frame_interval);
        component
    }

    fn init_effect(&mut self) {
        self.effect_state = match self.effect {
            Effect::Typewriter => EffectState::Typewriter { pos: 0 },
            Effect::Scanline => EffectState::Scanline { row: 0 },
            Effect::Rain => {
                // Track falling position for each column
                let mut rng = rand::rng();
                EffectState::Rain {
                    drops: (0..WIDTH)
                        .map(|_| Drop {
                            y: -(rng.random_range(0..DISPLAY_HEIGHT * 2) as isize),
                            settled: 0,
                        })
                        .collect(),
                }
            }
            // Shuffle all pixel positions
            Effect::Fade => EffectState::Positions {
                positions: shuffled_positions(),
                idx: 0,
            },
            Effect::Crt => EffectState::Crt { expansion: 0 },
            Effect::Glitch => EffectState::Glitch {
                phase: 0,
                glitch_frames: 8,
            },
            Effect::Dissolve => {
                // Start with random noise
                let chars = [' ', '░', '▒', '▓', '█', '▀', '▄'];
                let mut rng = rand::rng();
                self.current_grid = (0..DISPLAY_HEIGHT)
                    .map(|_| {
                        (0..WIDTH)
                            .map(|_| chars[rng.random_range(0..chars.len())])
                            .collect()
                    })
                    .collect();
                // Shuffle positions for gradual resolve
                EffectState::Positions {
                    positions: shuffled_positions(),
                    idx: 0,
                }
            }
        };
    }

    /// When the next animation frame is due.
    pub fn deadline(&self) -> Option<Instant> {
        self.next_frame
    }

    /// Advance the animation. `true` when the caller has to render again.
    pub fn tick(&mut self) -> bool {
        let Some(next_frame) = self.next_frame else {
            return false;
        };
        if Instant::now() < next_frame {
            return false;
        }
        let done = self.tick_effect();
        self.update_display();
        if done {
            self.stop_animation();
        } else {
            self.next_frame = Some(next_frame + self.frame_interval);
        }
        true
    }

    fn stop_animation(&mut self) {
        self.next_frame = None;
    }

    /// `dispose()` — stops the animation.
    pub fn dispose(&mut self) {
        self.stop_animation();
    }

    fn tick_effect(&mut self) -> bool {
        match self.effect {
            Effect::Typewriter => self.tick_typewriter(),
            Effect::Scanline => self.tick_scanline(),
            Effect::Rain => self.tick_rain(),
            Effect::Fade => self.tick_positions(15),
            Effect::Crt => self.tick_crt(),
            Effect::Glitch => self.tick_glitch(),
            Effect::Dissolve => self.tick_positions(20),
        }
    }

    fn tick_typewriter(&mut self) -> bool {
        let EffectState::Typewriter { mut pos } = self.effect_state else {
            return true;
        };
        let pixels_per_frame = 3;

        for _ in 0..pixels_per_frame {
            let row = pos / WIDTH;
            let x = pos % WIDTH;
            if row >= DISPLAY_HEIGHT {
                self.effect_state = EffectState::Typewriter { pos };
                return true;
            }
            self.current_grid[row][x] = self.final_grid[row][x];
            pos += 1;
        }
        self.effect_state = EffectState::Typewriter { pos };
        false
    }

    fn tick_scanline(&mut self) -> bool {
        let EffectState::Scanline { mut row } = self.effect_state else {
            return true;
        };
        if row >= DISPLAY_HEIGHT {
            return true;
        }

        // Copy row
        for x in 0..WIDTH {
            self.current_grid[row][x] = self.final_grid[row][x];
        }
        row += 1;
        self.effect_state = EffectState::Scanline { row };
        false
    }

    fn tick_rain(&mut self) -> bool {
        let EffectState::Rain { mut drops } =
            std::mem::replace(&mut self.effect_state, EffectState::Typewriter { pos: 0 })
        else {
            return true;
        };

        let mut all_settled = true;
        self.current_grid = create_empty_grid();
        let mut rng = rand::rng();

        for (x, drop) in drops.iter_mut().enumerate().take(WIDTH) {
            // Draw settled pixels
            let mut row = DISPLAY_HEIGHT as isize - 1;
            while row >= DISPLAY_HEIGHT as isize - drop.settled as isize {
                if row >= 0 {
                    self.current_grid[row as usize][x] = self.final_grid[row as usize][x];
                }
                row -= 1;
            }

            // Check if this column is done
            if drop.settled >= DISPLAY_HEIGHT {
                continue;
            }

            all_settled = false;

            // Find the target row for this column (lowest non-space pixel)
            let mut target_row: isize = -1;
            let mut row = DISPLAY_HEIGHT as isize - 1 - drop.settled as isize;
            while row >= 0 {
                if self.final_grid[row as usize][x] != ' ' {
                    target_row = row;
                    break;
                }
                row -= 1;
            }

            // Move drop down
            drop.y += 1;

            // Draw falling drop
            if drop.y >= 0 && drop.y < DISPLAY_HEIGHT as isize {
                if target_row >= 0 && drop.y >= target_row {
                    // Settle
                    drop.settled = DISPLAY_HEIGHT - target_row as usize;
                    drop.y = -(rng.random_range(0..5) as isize) - 1;
                } else {
                    // Still falling
                    self.current_grid[drop.y as usize][x] = '▓';
                }
            }
        }

        self.effect_state = EffectState::Rain { drops };
        all_settled
    }

    /// `tickFade` and `tickDissolve`; they differ only in the pixel budget.
    fn tick_positions(&mut self, pixels_per_frame: usize) -> bool {
        let EffectState::Positions {
            ref positions,
            mut idx,
        } = self.effect_state
        else {
            return true;
        };
        let positions = positions.clone();

        for _ in 0..pixels_per_frame {
            let Some((row, x)) = positions.get(idx).copied() else {
                self.effect_state = EffectState::Positions { positions, idx };
                return true;
            };
            self.current_grid[row][x] = self.final_grid[row][x];
            idx += 1;
        }
        self.effect_state = EffectState::Positions { positions, idx };
        false
    }

    fn tick_crt(&mut self) -> bool {
        let EffectState::Crt { mut expansion } = self.effect_state else {
            return true;
        };
        let mid_row = DISPLAY_HEIGHT / 2;

        self.current_grid = create_empty_grid();

        // Draw from middle expanding outward
        let top = mid_row as isize - expansion as isize;
        let bottom = mid_row + expansion;

        let mut row = top.max(0) as usize;
        while row <= bottom.min(DISPLAY_HEIGHT - 1) {
            for x in 0..WIDTH {
                self.current_grid[row][x] = self.final_grid[row][x];
            }
            row += 1;
        }

        expansion += 1;
        self.effect_state = EffectState::Crt { expansion };
        expansion > DISPLAY_HEIGHT
    }

    fn tick_glitch(&mut self) -> bool {
        let EffectState::Glitch {
            mut phase,
            glitch_frames,
        } = self.effect_state
        else {
            return true;
        };

        if phase < glitch_frames {
            let mut rng = rand::rng();
            // Glitch phase: show corrupted version
            self.current_grid = self
                .final_grid
                .iter()
                .map(|row| {
                    let offset = rng.random_range(0..7) as isize - 3;
                    let glitch_row = row.clone();

                    // Random horizontal offset
                    if rng.random::<f64>() < 0.3 {
                        // `slice(offset)` counts from the end for a negative offset.
                        let start = if offset >= 0 {
                            (offset as usize).min(glitch_row.len())
                        } else {
                            glitch_row.len() - ((-offset) as usize).min(glitch_row.len())
                        };
                        let mut shifted: Vec<char> = glitch_row[start..].to_vec();
                        shifted.extend_from_slice(&glitch_row[..start]);
                        shifted.truncate(WIDTH);
                        return shifted;
                    }

                    // Random vertical swap
                    if rng.random::<f64>() < 0.2 {
                        let swap_row = rng.random_range(0..DISPLAY_HEIGHT);
                        return self.final_grid[swap_row].clone();
                    }

                    glitch_row
                })
                .collect();
            phase += 1;
            self.effect_state = EffectState::Glitch {
                phase,
                glitch_frames,
            };
            return false;
        }

        // Final frame: show clean image
        self.current_grid = self.final_grid.clone();
        true
    }

    fn update_display(&mut self) {
        self.grid_version += 1;
    }
}

impl Component for ArminComponent {
    fn invalidate(&mut self) {
        self.cached_width = 0;
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        if width == self.cached_width && self.cached_version == self.grid_version as i64 {
            return self.cached_lines.clone();
        }

        let theme_instance = theme();
        let padding = 1usize;
        let available_width = width.saturating_sub(padding);

        self.cached_lines = self
            .current_grid
            .iter()
            .map(|row| {
                // Clip row to available width before applying color
                let clipped: String = row.iter().take(available_width).collect();
                let pad_right = width
                    .saturating_sub(padding)
                    .saturating_sub(clipped.chars().count());
                format!(
                    " {}{}",
                    theme_instance.fg(ThemeColor::Accent, &clipped),
                    " ".repeat(pad_right)
                )
            })
            .collect();

        // Add "ARMIN SAYS HI" at the end
        let message = "ARMIN SAYS HI";
        let msg_pad_right = width
            .saturating_sub(padding)
            .saturating_sub(message.chars().count());
        self.cached_lines.push(format!(
            " {}{}",
            theme_instance.fg(ThemeColor::Accent, message),
            " ".repeat(msg_pad_right)
        ));

        self.cached_width = width;
        self.cached_version = self.grid_version as i64;

        self.cached_lines.clone()
    }
}
