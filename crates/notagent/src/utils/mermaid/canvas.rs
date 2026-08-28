use super::types::{Cls, Span};
use super::width::measured;

/// Sentinel occupying the trailing column of a wide glyph. Never emitted: the
/// line builder skips it so a CJK character claims two cells of layout but
/// contributes one character of output.
pub const CONT: &str = "\0";

/// Connection direction bits, combined into a box-drawing glyph by `mask_char`.
pub const U: u8 = 1;
pub const D: u8 = 2;
pub const L: u8 = 4;
pub const R: u8 = 8;

/// Line styles, tracked per cell so crossing edges keep their own stroke.
pub const STY_DOT: u8 = 1;
pub const STY_THICK: u8 = 2;
pub const STY_SOLID: u8 = 4;

/// What `Canvas::to_lines` produced.
pub struct CanvasLines {
    pub plain: Vec<String>,
    pub styled: Vec<Vec<Span>>,
    pub width: usize,
}

/// A grid of cells. Edges accumulate as direction bits rather than glyphs so
/// that crossings and junctions resolve correctly whatever order they are drawn
/// in; `finalize_mask` turns the accumulated bits into characters at the end.
/// `occupied` marks cells claimed by a box, which edge bits must not overwrite.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub ch: Vec<String>,
    pub cls: Vec<Cls>,
    pub mask: Vec<u8>,
    pub style: Vec<u8>,
    pub occupied: Vec<u8>,
    pub cur_style: u8,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        let n = w * h;
        Self {
            w,
            h,
            ch: vec![" ".to_string(); n],
            cls: vec![Cls::None; n],
            mask: vec![0; n],
            style: vec![0; n],
            occupied: vec![0; n],
            cur_style: STY_SOLID,
        }
    }

    pub fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    pub fn set(&mut self, x: usize, y: usize, c: &str, cls: Cls) {
        if x >= self.w || y >= self.h {
            return;
        }
        let index = self.idx(x, y);
        self.ch[index] = c.to_string();
        self.cls[index] = cls;
    }

    /// Accumulate direction bits on a free cell.
    /// `cls` is the class to claim the cell for; `border` cells are never
    /// reclassified, so a connector meeting a box keeps the box's styling.
    pub fn add_bits(&mut self, x: usize, y: usize, bits: u8, cls: Cls) {
        if x >= self.w || y >= self.h {
            return;
        }
        let index = self.idx(x, y);
        if self.occupied[index] != 0 {
            return;
        }
        self.mask[index] |= bits;
        self.style[index] |= self.cur_style;
        if self.cls[index] != Cls::Border {
            self.cls[index] = cls;
        }
    }

    /// `addBits(x, y, bits)` with its default class.
    pub fn add_edge_bits(&mut self, x: usize, y: usize, bits: u8) {
        self.add_bits(x, y, bits, Cls::Edge);
    }

    /// Stamp a finished sub-canvas (a subgraph frame's contents) at an offset.
    pub fn blit(&mut self, sub: &Canvas, ox: usize, oy: usize) {
        for sy in 0..sub.h {
            for sx in 0..sub.w {
                let x = ox + sx;
                let y = oy + sy;
                if x >= self.w || y >= self.h {
                    continue;
                }
                let source = sub.idx(sx, sy);
                let target = self.idx(x, y);
                self.ch[target] = sub.ch[source].clone();
                self.cls[target] = sub.cls[source];
                self.style[target] = sub.style[source];
                self.occupied[target] = 1;
            }
        }
    }

    /// Add direction bits even to an occupied cell, so an edge can meet a border.
    pub fn junction(&mut self, x: usize, y: usize, bits: u8) {
        if x >= self.w || y >= self.h {
            return;
        }
        let index = self.idx(x, y);
        self.mask[index] |= bits;
        if self.cls[index] != Cls::Border {
            self.cls[index] = Cls::Edge;
        }
    }

    pub fn seg_v(&mut self, x: usize, y0: usize, y1: usize) {
        let a = y0.min(y1);
        let b = y0.max(y1);
        for y in a..=b {
            let mut bits = 0;
            if y > a {
                bits |= U;
            }
            if y < b {
                bits |= D;
            }
            self.add_edge_bits(x, y, bits);
        }
    }

    pub fn seg_h(&mut self, y: usize, x0: usize, x1: usize) {
        let a = x0.min(x1);
        let b = x0.max(x1);
        for x in a..=b {
            let mut bits = 0;
            if x > a {
                bits |= L;
            }
            if x < b {
                bits |= R;
            }
            self.add_edge_bits(x, y, bits);
        }
    }

    /// Resolve accumulated direction bits into glyphs, honouring line style.
    pub fn finalize_mask(&mut self) {
        for index in 0..self.ch.len() {
            if self.mask[index] != 0 && self.ch[index] == " " {
                let character = mask_char(self.mask[index]);
                self.ch[index] = if self.style[index] == STY_DOT {
                    dotted_char(character)
                } else if self.style[index] == STY_THICK {
                    thick_char(character)
                } else {
                    character.to_string()
                };
            }
        }
    }

    /// Mirror top-to-bottom for `BT`. Rows reorder but within-row text does
    /// not, so labels stay readable; box-drawing glyphs flip to match.
    pub fn flip_vertical(&mut self) {
        for y in 0..(self.h / 2) {
            let y2 = self.h - 1 - y;
            for x in 0..self.w {
                let i = self.idx(x, y);
                let j = self.idx(x, y2);
                self.ch.swap(i, j);
                self.cls.swap(i, j);
            }
        }
        for character in self.ch.iter_mut() {
            *character = flip_glyph_v(character);
        }
    }

    /// Mirror left-to-right for `RL`. Mirroring reverses each row, so after
    /// flipping glyphs each text/label run is reversed back to reading order.
    pub fn flip_horizontal(&mut self) {
        for y in 0..self.h {
            for x in 0..(self.w / 2) {
                let x2 = self.w - 1 - x;
                let i = self.idx(x, y);
                let j = self.idx(x2, y);
                self.ch.swap(i, j);
                self.cls.swap(i, j);
            }
        }
        for character in self.ch.iter_mut() {
            *character = flip_glyph_h(character);
        }
        for y in 0..self.h {
            let mut x = 0;
            while x < self.w {
                let cls = self.cls[self.idx(x, y)];
                if cls == Cls::Text || cls == Cls::EdgeLabel {
                    let start = self.idx(x, y);
                    while x < self.w && self.cls[self.idx(x, y)] == cls {
                        x += 1;
                    }
                    let end = self.idx(x, y);
                    self.ch[start..end].reverse();
                } else {
                    x += 1;
                }
            }
        }
    }

    /// Group each row into runs of one class, dropping wide-glyph continuations.
    pub fn to_lines(&self) -> CanvasLines {
        let mut plain: Vec<String> = Vec::new();
        let mut styled: Vec<Vec<Span>> = Vec::new();
        let mut width = 0;
        for y in 0..self.h {
            // A trailing CONT counts as painted: it is the second cell of a
            // wide glyph, so the row really does reach that column.
            let mut last = 0;
            for x in (0..self.w).rev() {
                if self.ch[self.idx(x, y)] != " " {
                    last = x + 1;
                    break;
                }
            }
            width = width.max(last);
            let mut spans: Vec<Span> = Vec::new();
            let mut plain_row = String::new();
            let mut run = String::new();
            let mut run_cls = Cls::None;
            for x in 0..last {
                let index = self.idx(x, y);
                let character = self.ch[index].as_str();
                if character == CONT {
                    continue;
                }
                let cls = self.cls[index];
                plain_row.push_str(character);
                if cls != run_cls && !run.is_empty() {
                    spans.push(Span {
                        text: std::mem::take(&mut run),
                        cls: run_cls,
                    });
                }
                run_cls = cls;
                run.push_str(character);
            }
            if !run.is_empty() {
                spans.push(Span {
                    text: run,
                    cls: run_cls,
                });
            }
            styled.push(spans);
            // Only ASCII spaces, which is all a blank cell ever holds.
            plain.push(plain_row.trim_end_matches(' ').to_string());
        }
        let mut first = 0;
        while first < plain.len() && plain[first].is_empty() {
            first += 1;
        }
        let mut end = plain.len();
        while end > first && plain[end - 1].is_empty() {
            end -= 1;
        }
        CanvasLines {
            plain: plain[first..end].to_vec(),
            styled: styled[first..end].to_vec(),
            width,
        }
    }
}

/// Paint `text` at `x, y`, one grapheme cluster per cell.
/// A wide cluster claims a second cell, marked with `CONT` so the line builder
/// emits one character for it rather than a stray space.
pub fn draw_text(canvas: &mut Canvas, text: &str, x: usize, y: usize, cls: Cls) {
    let mut cur = x;
    for (cluster, cluster_width) in measured(text) {
        if cluster_width == 0 {
            continue;
        }
        canvas.set(cur, y, cluster, cls);
        for k in 1..cluster_width {
            canvas.set(cur + k, y, CONT, cls);
        }
        cur += cluster_width;
    }
}

/// Paint `text` at `x, y`, clearing any edge bits underneath first.
pub fn draw_text_over_edges(canvas: &mut Canvas, text: &str, x: usize, y: usize, cls: Cls) {
    let mut cur = x;
    for (cluster, cluster_width) in measured(text) {
        if cluster_width == 0 {
            continue;
        }
        for k in 0..cluster_width {
            if cur + k < canvas.w && y < canvas.h {
                let index = canvas.idx(cur + k, y);
                canvas.mask[index] = 0;
            }
            canvas.set(cur + k, y, if k == 0 { cluster } else { CONT }, cls);
        }
        cur += cluster_width;
    }
}

pub fn mask_char(mask: u8) -> char {
    match mask {
        0 => ' ',
        m if m == U || m == D || m == U | D => '│',
        m if m == L || m == R || m == L | R => '─',
        m if m == D | R => '┌',
        m if m == D | L => '┐',
        m if m == U | R => '└',
        m if m == U | L => '┘',
        m if m == U | D | R => '├',
        m if m == U | D | L => '┤',
        m if m == D | L | R => '┬',
        m if m == U | L | R => '┴',
        _ => '┼',
    }
}

pub fn dotted_char(character: char) -> String {
    match character {
        '─' => '╌',
        '│' => '╎',
        other => other,
    }
    .to_string()
}

pub fn thick_char(character: char) -> String {
    match character {
        '─' => '━',
        '│' => '┃',
        '┌' => '┏',
        '┐' => '┓',
        '└' => '┗',
        '┘' => '┛',
        '├' => '┣',
        '┤' => '┫',
        '┬' => '┳',
        '┴' => '┻',
        '┼' => '╋',
        other => other,
    }
    .to_string()
}

pub fn flip_glyph_v(character: &str) -> String {
    match character {
        "┌" => "└",
        "└" => "┌",
        "┐" => "┘",
        "┘" => "┐",
        "┏" => "┗",
        "┗" => "┏",
        "┓" => "┛",
        "┛" => "┓",
        "╭" => "╰",
        "╰" => "╭",
        "╮" => "╯",
        "╯" => "╮",
        "┬" => "┴",
        "┴" => "┬",
        "┳" => "┻",
        "┻" => "┳",
        "▼" => "▲",
        "▲" => "▼",
        "▽" => "△",
        "△" => "▽",
        other => other,
    }
    .to_string()
}

pub fn flip_glyph_h(character: &str) -> String {
    match character {
        "┌" => "┐",
        "┐" => "┌",
        "└" => "┘",
        "┘" => "└",
        "┏" => "┓",
        "┓" => "┏",
        "┗" => "┛",
        "┛" => "┗",
        "╭" => "╮",
        "╮" => "╭",
        "╰" => "╯",
        "╯" => "╰",
        "├" => "┤",
        "┤" => "├",
        "┣" => "┫",
        "┫" => "┣",
        "▶" => "◄",
        "◄" => "▶",
        "▷" => "◁",
        "◁" => "▷",
        other => other,
    }
    .to_string()
}
