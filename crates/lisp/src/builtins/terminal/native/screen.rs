//! What the terminal shows, cell by cell, and the bytes that turn one screen into the next
//! (ADR-586).
//!
//! `term-draw` used to write every op of every frame straight to the tty: a frame that starts
//! with `[:clear]` (every editor frame does) erased the whole screen and printed it again, so a
//! keystroke cost the full screen in bytes and a terminal over ssh flickered as it redrew. Now a
//! frame is painted into a [`Screen`] — a grid of cells, each a grapheme and its [`Style`] — and
//! [`render`] writes only the cells that differ from the screen the terminal already shows,
//! with the colours a terminal can show ([`ColorDepth`]) and the cursor where the frame put it.
//!
//! Pure: nothing here touches the tty, so the grid and the diff are tested as data.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A colour as the terminal is asked for it: one of its 256 indexed colours (0–15 the named
/// palette the user's theme sets) or a true colour.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Colour {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Colour {
    /// The `Colour` of a crossterm colour (what a face's colour parses to).
    pub fn of(colour: crossterm::style::Color) -> Option<Colour> {
        use crossterm::style::Color;
        Some(match colour {
            Color::Reset => return None,
            Color::Black => Colour::Indexed(0),
            Color::DarkRed => Colour::Indexed(1),
            Color::DarkGreen => Colour::Indexed(2),
            Color::DarkYellow => Colour::Indexed(3),
            Color::DarkBlue => Colour::Indexed(4),
            Color::DarkMagenta => Colour::Indexed(5),
            Color::DarkCyan => Colour::Indexed(6),
            Color::Grey => Colour::Indexed(7),
            Color::DarkGrey => Colour::Indexed(8),
            Color::Red => Colour::Indexed(9),
            Color::Green => Colour::Indexed(10),
            Color::Yellow => Colour::Indexed(11),
            Color::Blue => Colour::Indexed(12),
            Color::Magenta => Colour::Indexed(13),
            Color::Cyan => Colour::Indexed(14),
            Color::White => Colour::Indexed(15),
            Color::AnsiValue(index) => Colour::Indexed(index),
            Color::Rgb { r, g, b } => Colour::Rgb(r, g, b),
        })
    }
}

/// How many colours the terminal can show: what a true colour is degraded to before it is
/// written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    TrueColor,
    Ansi256,
    Ansi16,
}

/// The terminals that draw 24-bit colour, by the name they go by in lower case.
const TRUE_COLOUR_TERMINALS: [&str; 9] = [
    "alacritty",
    "contour",
    "foot",
    "ghostty",
    "iterm2",
    "kitty",
    "konsole",
    "mintty",
    "wezterm",
];

/// Whether `name` names a terminal that draws 24-bit colour ([`TRUE_COLOUR_TERMINALS`]),
/// whichever way it was learned: its `$TERM` (`xterm-kitty`, `foot-extra`) or its XTVERSION
/// answer (`WezTerm 20240203`, `kitty(0.36.4)`) — the one list both read. Case aside, and an
/// `xterm-` prefix aside.
pub fn names_true_colour_terminal(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let name = name.strip_prefix("xterm-").unwrap_or(&name);
    TRUE_COLOUR_TERMINALS
        .iter()
        .any(|terminal| name.starts_with(terminal))
}

impl ColorDepth {
    /// The depth `$COLORTERM` and `$TERM` say the terminal has, as Emacs and most TUIs read
    /// them: `COLORTERM=truecolor` (or `24bit`) is true colour, a `TERM` naming `256color` (or
    /// `direct`, or a terminal known to have it) is 256, anything else — the Linux console,
    /// `xterm`, `screen` — the 16 named colours. Over ssh `COLORTERM` is rarely passed on, so a
    /// remote session is usually 256 colours, which is what it can rely on.
    pub fn detect(colorterm: Option<&str>, term: Option<&str>) -> ColorDepth {
        let colorterm = colorterm.unwrap_or("").to_ascii_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" {
            return ColorDepth::TrueColor;
        }
        let term = term.unwrap_or("").to_ascii_lowercase();
        if term.ends_with("-direct") {
            return ColorDepth::TrueColor;
        }
        // a terminal that draws 24-bit colour whether or not COLORTERM reached us (over ssh it
        // rarely does), named by its TERM
        if names_true_colour_terminal(&term) {
            return ColorDepth::TrueColor;
        }
        let known_256 = ["tmux"];
        if term.contains("256color") || known_256.iter().any(|name| term.starts_with(name)) {
            return ColorDepth::Ansi256;
        }
        ColorDepth::Ansi16
    }

    /// The keyword the Brood side names this depth by: `:truecolor`, `:256`, `:16`.
    pub fn name(self) -> &'static str {
        match self {
            ColorDepth::TrueColor => "truecolor",
            ColorDepth::Ansi256 => "256",
            ColorDepth::Ansi16 => "16",
        }
    }

    /// The depth named `name` (see [`ColorDepth::name`]).
    pub fn named(name: &str) -> Option<ColorDepth> {
        match name {
            "truecolor" => Some(ColorDepth::TrueColor),
            "256" => Some(ColorDepth::Ansi256),
            "16" => Some(ColorDepth::Ansi16),
            _ => None,
        }
    }

    /// `colour` as this depth can show it: a true colour becomes the nearest of the 256 (the
    /// 6×6×6 cube or the grey ramp) or of the 16 named colours.
    pub fn degrade(self, colour: Colour) -> Colour {
        match (self, colour) {
            (ColorDepth::TrueColor, _) => colour,
            (ColorDepth::Ansi256, Colour::Rgb(red, green, blue)) => {
                Colour::Indexed(nearest_256(red, green, blue))
            }
            (ColorDepth::Ansi256, indexed) => indexed,
            (ColorDepth::Ansi16, Colour::Indexed(index)) if index < 16 => colour,
            (ColorDepth::Ansi16, Colour::Indexed(index)) => {
                let (red, green, blue) = rgb_of_index(index);
                Colour::Indexed(nearest_16(red, green, blue))
            }
            (ColorDepth::Ansi16, Colour::Rgb(red, green, blue)) => {
                Colour::Indexed(nearest_16(red, green, blue))
            }
        }
    }
}

/// The xterm palette's 16 named colours, as xterm draws them by default — what a true colour
/// is measured against for a 16-colour terminal.
const NAMED_RGB: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

/// The levels of the 256-colour cube's axes.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let delta = |x: u8, y: u8| (x as i32 - y as i32).unsigned_abs();
    // weighted as the eye weighs them (the "redmean"-free cheap form: green counts most)
    let (red, green, blue) = (delta(a.0, b.0), delta(a.1, b.1), delta(a.2, b.2));
    3 * red * red + 4 * green * green + 2 * blue * blue
}

/// The RGB the 256-colour palette index `index` stands for.
fn rgb_of_index(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => NAMED_RGB[index as usize],
        16..=231 => {
            let cube = index - 16;
            (
                CUBE_LEVELS[(cube / 36) as usize],
                CUBE_LEVELS[((cube / 6) % 6) as usize],
                CUBE_LEVELS[(cube % 6) as usize],
            )
        }
        _ => {
            let level = 8 + 10 * (index - 232);
            (level, level, level)
        }
    }
}

/// The nearest 256-palette colour to an RGB: the nearest cube point or the nearest grey,
/// whichever is closer (never 0–15, whose RGB the user's theme decides).
fn nearest_256(red: u8, green: u8, blue: u8) -> u8 {
    let axis = |value: u8| {
        (0..6)
            .min_by_key(|&step| (CUBE_LEVELS[step] as i32 - value as i32).unsigned_abs())
            .unwrap_or(0) as u8
    };
    let cube = 16 + 36 * axis(red) + 6 * axis(green) + axis(blue);
    let average = (red as u32 + green as u32 + blue as u32) / 3;
    let grey = 232 + ((average.saturating_sub(3)) / 10).min(23) as u8;
    let target = (red, green, blue);
    if distance(rgb_of_index(grey), target) < distance(rgb_of_index(cube), target) {
        grey
    } else {
        cube
    }
}

/// The nearest of the 16 named colours to an RGB.
fn nearest_16(red: u8, green: u8, blue: u8) -> u8 {
    (0..16u8)
        .min_by_key(|&index| distance(NAMED_RGB[index as usize], (red, green, blue)))
        .unwrap_or(0)
}

/// How a cell is underlined.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Underline {
    #[default]
    None,
    Straight,
    Curly,
}

/// How one cell is drawn: its colours and attributes. `None` colours are the terminal's own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Style {
    pub fg: Option<Colour>,
    pub bg: Option<Colour>,
    pub bold: bool,
    pub italic: bool,
    pub underline: Underline,
    pub underline_colour: Option<Colour>,
    pub reverse: bool,
}

impl Style {
    /// This style with the colours it does not name taken from `default` — the default face
    /// (`term-bg`), as a window draws a face with no `:bg` on its background.
    pub fn over(self, default: Style) -> Style {
        Style {
            fg: self.fg.or(default.fg),
            bg: self.bg.or(default.bg),
            ..self
        }
    }
}

/// One cell: the grapheme drawn there and its style. A wide grapheme (a CJK character, most
/// emoji) takes two cells — its own and a CONTINUATION after it, whose text is empty.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Cell {
    pub text: String,
    pub style: Style,
}

impl Cell {
    fn blank(style: Style) -> Cell {
        Cell {
            text: " ".to_string(),
            style,
        }
    }

    fn continuation(&self) -> bool {
        self.text.is_empty()
    }
}

/// The caret's shape (DECSCUSR's steady shapes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

/// Where the terminal's cursor is shown, and how.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    pub shape: CursorShape,
}

/// A terminal's screen as cells, `cols` × `rows`, and where its cursor shows (none: hidden).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    cells: Vec<Cell>,
    pub cursor: Option<Cursor>,
    /// Whether the terminal measures a grapheme cluster as one glyph of its Unicode width
    /// (mode 2027, [`glyph`]); without it a cluster whose code points add up to another width
    /// is drawn as its first code point.
    pub clusters: bool,
}

/// How a grapheme is drawn: the cells it takes — its Unicode width, at most two, the width an
/// app lays text out by — and the text written for them, which the terminal advances its
/// cursor by `advance` cells for (no more than `cells`; the cells past it are blanks).
///
/// A control character (a tab, an escape) is drawn as a space: written raw it would move the
/// terminal's cursor and every cell after it would be painted in the wrong place. A grapheme
/// wider than two cells is counted as two, the most a terminal gives one.
///
/// A terminal measures a grapheme as Unicode does only with grapheme clustering (mode 2027,
/// `clusters`); any other advances by each code point's width, as `wcwidth` does — a family
/// emoji joined by ZWJs is three wide glyphs there, a heart with an emoji presentation
/// selector one narrow one. Such a cluster is drawn as its first code point, in the cells the
/// app laid it out in, so every cell after it lands where the screen has it.
fn glyph(grapheme: &str, clusters: bool) -> (usize, &str, usize) {
    use unicode_width::UnicodeWidthChar;
    if grapheme.chars().any(char::is_control) {
        return (1, " ", 1);
    }
    let cells = grapheme.width().min(2);
    let by_code_points: usize = grapheme.chars().map(|c| c.width().unwrap_or(0)).sum();
    if clusters || by_code_points == cells {
        return (cells, grapheme, cells);
    }
    let first = grapheme.chars().next().map_or(0, char::len_utf8);
    let base = &grapheme[..first];
    match base.width() {
        advance @ 1.. if advance <= cells => (cells, base, advance),
        _ => (cells, " ", 1),
    }
}

/// A rectangle of the screen a scroll moves: rows `top..=bottom`, columns `left..=right`. The
/// one shape both a scroll of whole rows (the margins top and bottom, `DECSTBM`) and a scroll
/// of a band of columns (left and right too, `DECSLRM`) are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Region {
    top: u16,
    bottom: u16,
    left: u16,
    right: u16,
}

impl Region {
    /// Whether the region is whole rows of a screen `cols` wide: no left or right margin.
    fn whole_rows(&self, cols: u16) -> bool {
        self.left == 0 && self.right + 1 == cols
    }

    /// The columns, as a range of cell indices within a row.
    fn columns(&self) -> std::ops::Range<usize> {
        self.left as usize..self.right as usize + 1
    }
}

impl Screen {
    /// A blank screen of `cols` × `rows`, no cursor shown.
    pub fn new(cols: u16, rows: u16) -> Screen {
        Screen::filled(cols, rows, Style::default())
    }

    /// A screen of `cols` × `rows` blanks in `style`, no cursor shown, drawn for a terminal
    /// that measures code points ([`glyph`]).
    pub fn filled(cols: u16, rows: u16, style: Style) -> Screen {
        Screen {
            cols,
            rows,
            cells: vec![Cell::blank(style); cols as usize * rows as usize],
            cursor: None,
            clusters: false,
        }
    }

    /// The cells of `row`, left to right.
    fn row_cells(&self, row: u16) -> &[Cell] {
        let start = self.index(row, 0);
        &self.cells[start..start + self.cols as usize]
    }

    /// The cells of `row` in `columns`, left to right.
    fn segment(&self, row: u16, columns: &std::ops::Range<usize>) -> &[Cell] {
        &self.row_cells(row)[columns.clone()]
    }

    /// This screen as the terminal shows it after `scroll`: the region's cells moved, the
    /// cells it leaves blank filled in its `fill`, the rest as they were.
    fn scrolled(&self, scroll: Scroll) -> Screen {
        let mut moved = self.clone();
        let region = scroll.region;
        let columns = region.columns();
        for row in region.top..=region.bottom {
            let source = row as i32 + scroll.shift;
            let target = self.index(row, 0);
            let target = target + columns.start..target + columns.end;
            if (region.top as i32..=region.bottom as i32).contains(&source) {
                let from = self.index(source as u16, 0);
                moved.cells[target]
                    .clone_from_slice(&self.cells[from + columns.start..from + columns.end]);
            } else {
                moved.cells[target].fill(Cell::blank(scroll.fill));
            }
        }
        moved
    }

    fn index(&self, row: u16, col: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    /// The cell at `row`, `col` (both in range).
    pub fn cell(&self, row: u16, col: u16) -> &Cell {
        &self.cells[self.index(row, col)]
    }

    /// The text of row `row` as drawn — continuations skipped — for tests and debugging.
    #[cfg(test)]
    pub fn row_text(&self, row: u16) -> String {
        (0..self.cols)
            .map(|col| self.cell(row, col).text.as_str())
            .collect()
    }

    /// Every cell blank in `style` (`[:clear]`): the terminal's own colours for the default
    /// style, or the default face `term-bg` set.
    pub fn clear(&mut self, style: Style) {
        for cell in &mut self.cells {
            *cell = Cell::blank(style);
        }
    }

    /// Make the cell at `row`, `col` safe to overwrite alone: when it is half of a wide
    /// grapheme, the other half becomes a blank in the grapheme's style.
    fn detach(&mut self, row: u16, col: u16) {
        let here = self.index(row, col);
        if self.cells[here].continuation() {
            if col > 0 {
                let head = here - 1;
                self.cells[head] = Cell::blank(self.cells[head].style);
            }
        } else if col + 1 < self.cols && self.cells[here + 1].continuation() {
            self.cells[here + 1] = Cell::blank(self.cells[here + 1].style);
        }
    }

    fn set(&mut self, row: u16, col: u16, cell: Cell) {
        self.detach(row, col);
        let here = self.index(row, col);
        self.cells[here] = cell;
    }

    /// Draw `text` from `row`, `col` in `style` (`[:text …]`): grapheme by grapheme, a wide
    /// one taking two cells, clipped to the screen. A wide grapheme cut by an edge draws as
    /// a blank in its place.
    ///
    /// A `style` that names no background draws OVER the cell, as a window draws text over
    /// what is already painted: the cell keeps its background — a `[:rect …]` under it (a
    /// current-line band, a selection bar), else the default face the frame cleared to.
    pub fn put_text(&mut self, row: i64, col: i64, text: &str, style: Style) {
        if row < 0 || row >= self.rows as i64 {
            return;
        }
        let row = row as u16;
        let mut at = col;
        for grapheme in text.graphemes(true) {
            if at >= self.cols as i64 {
                return;
            }
            let (width, drawn, advance) = glyph(grapheme, self.clusters);
            if width == 0 {
                continue;
            }
            let style = if style.bg.is_some() || at < 0 {
                style
            } else {
                Style {
                    bg: self
                        .cell(row, (at.min(self.cols as i64 - 1)) as u16)
                        .style
                        .bg,
                    ..style
                }
            };
            let end = at + width as i64;
            if at < 0 || end > self.cols as i64 {
                // a wide grapheme cut by the left or right edge: its visible cell is blank
                for cell_col in at.max(0)..end.min(self.cols as i64) {
                    self.set(row, cell_col as u16, Cell::blank(style));
                }
            } else {
                self.set(
                    row,
                    at as u16,
                    Cell {
                        text: drawn.to_string(),
                        style,
                    },
                );
                if advance < width {
                    // drawn narrower than laid out ([`glyph`]): the cell after it a blank
                    self.set(row, at as u16 + 1, Cell::blank(style));
                } else if width == 2 {
                    // the continuation's own right neighbour may be a wide head's tail
                    self.detach(row, at as u16 + 1);
                    let tail = self.index(row, at as u16 + 1);
                    self.cells[tail] = Cell {
                        text: String::new(),
                        style,
                    };
                }
            }
            at = end;
        }
    }

    /// Fill `width` × `height` cells from `row`, `col` with blanks in `style` (`[:rect …]`),
    /// clipped to the screen.
    pub fn fill(&mut self, row: i64, col: i64, width: i64, height: i64, style: Style) {
        let rows = row.max(0)..row.saturating_add(height).min(self.rows as i64);
        let cols = col.max(0)..col.saturating_add(width).min(self.cols as i64);
        for fill_row in rows {
            for fill_col in cols.clone() {
                self.set(fill_row as u16, fill_col as u16, Cell::blank(style));
            }
        }
    }
}

/// The SGR parameters a style sets at a depth, one slot each — what [`sgr`] writes after its
/// reset, and what [`sgr_change`] compares to write only the slots that differ.
#[derive(PartialEq, Eq)]
struct Rendition {
    bold: bool,
    italic: bool,
    underline: Option<&'static str>,
    reverse: bool,
    fg: Option<String>,
    bg: Option<String>,
    underline_colour: Option<String>,
}

fn rendition(style: Style, depth: ColorDepth) -> Rendition {
    let colour = |colour: Colour, base: u8, bright: u8, extended: u8| match depth.degrade(colour) {
        Colour::Indexed(index) if index < 8 => format!("{}", base + index),
        Colour::Indexed(index) if index < 16 && depth == ColorDepth::Ansi16 => {
            format!("{}", bright + index - 8)
        }
        Colour::Indexed(index) => format!("{extended};5;{index}"),
        Colour::Rgb(red, green, blue) => format!("{extended};2;{red};{green};{blue}"),
    };
    Rendition {
        bold: style.bold,
        italic: style.italic,
        underline: match style.underline {
            Underline::None => None,
            Underline::Straight => Some("4"),
            // the curly underline (`4:3`) only where colon sub-parameters are safe to send: a
            // terminal with true colour reads them; an older one may take `4:3` for something else
            Underline::Curly if depth == ColorDepth::TrueColor => Some("4:3"),
            Underline::Curly => Some("4"),
        },
        reverse: style.reverse,
        fg: style.fg.map(|fg| colour(fg, 30, 90, 38)),
        bg: style.bg.map(|bg| colour(bg, 40, 100, 48)),
        underline_colour: style
            .underline_colour
            .filter(|_| style.underline != Underline::None && depth != ColorDepth::Ansi16)
            .map(|underline_colour| match depth.degrade(underline_colour) {
                Colour::Indexed(index) => format!("58;5;{index}"),
                Colour::Rgb(red, green, blue) => format!("58;2;{red};{green};{blue}"),
            }),
    }
}

/// The SGR sequence that draws in `style` at `depth`, from a reset: `ESC [ 0 ; … m`.
pub fn sgr(style: Style, depth: ColorDepth) -> String {
    let rendition = rendition(style, depth);
    let mut parameters = vec!["0"];
    if rendition.bold {
        parameters.push("1");
    }
    if rendition.italic {
        parameters.push("3");
    }
    if let Some(underline) = rendition.underline {
        parameters.push(underline);
    }
    if rendition.reverse {
        parameters.push("7");
    }
    for slot in [&rendition.fg, &rendition.bg, &rendition.underline_colour]
        .into_iter()
        .flatten()
    {
        parameters.push(slot);
    }
    format!("\x1b[{}m", parameters.join(";"))
}

/// The SGR sequence that turns drawing in `from` into drawing in `to` — only the slots that
/// differ, each switched off by its own reset (`22` `23` `24` `27` `39` `49` `59`) — or the
/// whole of [`sgr`] from a reset when that is shorter, or when nothing is known of `from`.
/// Empty when the two draw the same. A page of code changes colour every few cells; from a
/// reset each change restated both colours, ~35 bytes, more than half of what a scroll wrote.
pub fn sgr_change(from: Option<Style>, to: Style, depth: ColorDepth) -> String {
    let Some(from) = from else {
        return sgr(to, depth);
    };
    if from == to {
        return String::new();
    }
    let full = sgr(to, depth);
    let (before, after) = (rendition(from, depth), rendition(to, depth));
    if before == after {
        return String::new();
    }
    let mut parameters: Vec<&str> = Vec::new();
    let mut switch = |was: bool, now: bool, on: &'static str, off: &'static str| {
        if was != now {
            parameters.push(if now { on } else { off });
        }
    };
    switch(before.bold, after.bold, "1", "22");
    switch(before.italic, after.italic, "3", "23");
    switch(before.reverse, after.reverse, "7", "27");
    if before.underline != after.underline {
        parameters.push(after.underline.unwrap_or("24"));
    }
    for (was, now, off) in [
        (&before.fg, &after.fg, "39"),
        (&before.bg, &after.bg, "49"),
        (&before.underline_colour, &after.underline_colour, "59"),
    ] {
        if was != now {
            parameters.push(now.as_deref().unwrap_or(off));
        }
    }
    let delta = format!("\x1b[{}m", parameters.join(";"));
    if delta.len() < full.len() {
        delta
    } else {
        full
    }
}

/// The OSC sequences that make the terminal's OWN colours `face`'s: its default background
/// (OSC 11) and foreground (OSC 10), and the cursor's colour (OSC 12, the foreground's) — so
/// the cells beyond the grid (a terminal's padding, the strip a window's odd size leaves) and
/// the cursor are the theme's, where only the cells were before. Only at true colour, where
/// the cells are drawn in exactly those colours; empty when `face` names neither colour.
pub fn default_colours(face: Style, depth: ColorDepth) -> String {
    if depth != ColorDepth::TrueColor {
        return String::new();
    }
    let spec = |colour: Colour| {
        let (red, green, blue) = match colour {
            Colour::Rgb(red, green, blue) => (red, green, blue),
            Colour::Indexed(index) => rgb_of_index(index),
        };
        format!("rgb:{red:02x}/{green:02x}/{blue:02x}")
    };
    let mut out = String::new();
    if let Some(bg) = face.bg {
        out.push_str(&format!("\x1b]11;{}\x1b\\", spec(bg)));
    }
    if let Some(fg) = face.fg {
        out.push_str(&format!("\x1b]10;{}\x1b\\", spec(fg)));
        out.push_str(&format!("\x1b]12;{}\x1b\\", spec(fg)));
    }
    out
}

/// What gives the terminal its own colours back after [`default_colours`]: OSC 110, 111, 112.
pub const DEFAULT_COLOURS_RESET: &str = "\x1b]110\x1b\\\x1b]111\x1b\\\x1b]112\x1b\\";

/// The bytes that make the terminal's own colours `face`'s at `depth` when it `holds` an
/// earlier face's ([`default_colours`]): those given back first, so a colour the new face
/// leaves the terminal's is the terminal's again — and whether it holds `face`'s after.
/// Nothing is written, and nothing held, where `face` sets none and nothing was.
pub fn default_colours_change(face: Style, depth: ColorDepth, holds: bool) -> (String, bool) {
    let colours = default_colours(face, depth);
    let mut out = String::new();
    if holds {
        out.push_str(DEFAULT_COLOURS_RESET);
    }
    let held = !colours.is_empty();
    out.push_str(&colours);
    (out, held)
}

/// What frames may rely on in the terminal they are written to: its colour depth; whether it
/// fills an erased cell with the current background ("background colour erase", which every
/// terminal emulator in use does and GNU screen does not by default) — where it does, a blank
/// run is erased (`CSI K`, `CSI n X`) rather than written cell by cell, and the first frame
/// clears to its own background; whether rows that moved are moved by the terminal, in a
/// scroll region (`DECSTBM` + `SU`/`SD`, ADR-740), and whether that region may be a band of
/// columns too (`DECSLRM`, [`ColumnMargins`]); and whether a frame is bracketed as a
/// synchronized update (`CSI ? 2026 h` … `l`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Output {
    pub depth: ColorDepth,
    pub erase_fills_background: bool,
    pub scroll_regions: bool,
    pub column_margins: ColumnMargins,
    pub synchronized: bool,
}

/// Whether a scroll region may have left and right margins (`DECSLRM`), which only a terminal
/// in left-right margin mode (`DECLRMM`, mode 69) reads — `CSI s` is otherwise "save the
/// cursor". Known only from the terminal's own answer (`probe`): `$TERM` cannot say.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColumnMargins {
    /// No left and right margins: a scroll moves whole rows.
    Unavailable,
    /// The mode is there, reset: set around each column scroll (`CSI ? 69 h` … `CSI ? 69 l`,
    /// whose reset also gives the margins back), so the terminal is never left in it.
    Toggled,
    /// The mode is on already: the margins set, and given back (`CSI s`) after the scroll.
    Held,
}

impl Output {
    /// What `$TERM` says: erasing fills in every terminal but GNU screen's (`screen*`) and a
    /// dumb one; scroll regions where it does, but on the Linux console (no `SU`/`SD`); no
    /// column margins, which only the terminal's answer tells.
    pub fn detect(depth: ColorDepth, term: Option<&str>) -> Output {
        let term = term.unwrap_or("").to_ascii_lowercase();
        let erase_fills_background =
            !(term.is_empty() || term == "dumb" || term.starts_with("screen"));
        Output {
            depth,
            erase_fills_background,
            scroll_regions: erase_fills_background && !term.starts_with("linux"),
            column_margins: ColumnMargins::Unavailable,
            synchronized: true,
        }
    }
}

impl From<ColorDepth> for Output {
    fn from(depth: ColorDepth) -> Output {
        Output {
            depth,
            erase_fills_background: false,
            scroll_regions: false,
            column_margins: ColumnMargins::Unavailable,
            synchronized: true,
        }
    }
}

/// Whether erasing in `style` draws the cell as `Cell::blank(style)` would: a space with no
/// underline or reverse, whose look is only its background.
fn erasable(cell: &Cell) -> bool {
    cell.text == " " && cell.style.underline == Underline::None && !cell.style.reverse
}

/// The fewest blank cells worth an erase: below it, writing the spaces costs no more.
const ERASE_AT_LEAST: u16 = 6;

/// What the terminal's cursor is doing, as the last frame left it — so a frame writes a
/// show, a hide or a shape only when it changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CursorState {
    pub visible: bool,
    pub shape: Option<CursorShape>,
}

/// How far apart two changed cells on a row may be and still be written as one run, the
/// unchanged cells between them reprinted: a cursor move costs about as many bytes.
const RUN_GAP: u16 = 4;

/// The bytes that turn the terminal showing `previous` into showing `next`, written as
/// `output` allows, appended to `out` — in stages, each over the screen the last one left:
///
/// 1. With no `previous`, or one of another size, the screen is cleared ([`clear`]) — to the
///    frame's commonest blank, where erasing fills with the background — and every other cell
///    written.
/// 2. A band of rows that moved is moved by the terminal in a scroll region, where `output`
///    allows ([`find_scroll`], ADR-740).
/// 3. Each row's changed cells are written ([`write_row_changes`]): a style change as only
///    what changed ([`sgr_change`]), a blank run as an erase where `output` allows.
/// 4. The writes are framed ([`frame`]): bracketed as a synchronized update (`CSI ?2026`,
///    unless `output` says the terminal does not know it — one that knows it paints at once,
///    any other ignores it), the cursor hidden while it moves and then placed.
///
/// A frame that changes nothing writes nothing. `cursor` is what the terminal's cursor was
/// doing and becomes what it does now.
pub fn render(
    previous: Option<&Screen>,
    next: &Screen,
    output: impl Into<Output>,
    cursor: &mut CursorState,
    out: &mut Vec<u8>,
) {
    let output = output.into();
    let mut body = String::new();
    let mut current_style: Option<Style> = None;
    let cleared;
    let previous = match previous {
        Some(screen) if screen.cols == next.cols && screen.rows == next.rows => screen,
        _ => {
            let ground = clear(next, output, &mut body);
            current_style = Some(ground);
            cleared = Screen::filled(next.cols, next.rows, ground);
            &cleared
        }
    };
    // rows that moved, moved by the terminal: what it then shows is what the rows are diffed
    // against
    let scrolled;
    let previous = match find_scroll(previous, next, output) {
        Some(scroll) => {
            body.push_str(&sgr_change(current_style, scroll.fill, output.depth));
            current_style = Some(scroll.fill);
            body.push_str(&scroll.sequence(next.cols, output.column_margins));
            scrolled = previous.scrolled(scroll);
            &scrolled
        }
        None => previous,
    };
    for row in 0..next.rows {
        write_row_changes(previous, next, row, output, &mut current_style, &mut body);
    }
    frame(&body, previous, next, output, current_style, cursor, out);
}

/// Clear the screen onto `body`, and the style its blanks are then in: the commonest blank of
/// `next` where an erase fills with it, so those cells need no writing; the terminal's
/// default otherwise.
fn clear(next: &Screen, output: Output, body: &mut String) -> Style {
    let ground = if output.erase_fills_background {
        commonest_blank(next)
    } else {
        Style::default()
    };
    body.push_str(&sgr(ground, output.depth));
    body.push_str("\x1b[2J");
    ground
}

/// The frame around `body`, the writes that bring `shown` to `next`, appended to `out`: a
/// synchronized update where `output` allows, the cursor hidden while the writes move it, the
/// style reset after them (`current_style` is the one they leave), and the cursor then placed,
/// shaped and shown as `next` wants it, or hidden. Nothing when the body is empty and the
/// cursor stays as it is.
fn frame(
    body: &str,
    shown: &Screen,
    next: &Screen,
    output: Output,
    current_style: Option<Style>,
    cursor: &mut CursorState,
    out: &mut Vec<u8>,
) {
    let wanted_shape = next.cursor.map(|at| at.shape);
    let cursor_changes = next.cursor.is_some() != cursor.visible
        || (next.cursor.is_some() && wanted_shape != cursor.shape)
        || (next.cursor.is_some() && shown.cursor != next.cursor);
    if body.is_empty() && !cursor_changes {
        return;
    }
    let mut frame = String::new();
    if output.synchronized {
        frame.push_str("\x1b[?2026h");
    }
    if cursor.visible && !body.is_empty() {
        frame.push_str("\x1b[?25l");
    }
    frame.push_str(body);
    if current_style.is_some_and(|style| style != Style::default()) {
        frame.push_str("\x1b[0m");
    }
    match next.cursor {
        Some(at) => {
            frame.push_str(&format!(
                "\x1b[{};{}H",
                at.row as u32 + 1,
                at.col as u32 + 1
            ));
            if cursor.shape != Some(at.shape) {
                frame.push_str(match at.shape {
                    CursorShape::Block => "\x1b[2 q",
                    CursorShape::Underline => "\x1b[4 q",
                    CursorShape::Bar => "\x1b[6 q",
                });
            }
            frame.push_str("\x1b[?25h");
            *cursor = CursorState {
                visible: true,
                shape: Some(at.shape),
            };
        }
        None => {
            frame.push_str("\x1b[?25l");
            cursor.visible = false;
        }
    }
    if output.synchronized {
        frame.push_str("\x1b[?2026l");
    }
    out.extend_from_slice(frame.as_bytes());
}

/// The style most of `screen`'s erasable blanks are in — what a cleared screen is filled with
/// so they need no writing — or the default style when it has none.
fn commonest_blank(screen: &Screen) -> Style {
    commonest_blank_in(screen.cells.iter())
}

/// The style most of `cells`' erasable blanks are in, or the default style when none is.
fn commonest_blank_in<'a>(cells: impl Iterator<Item = &'a Cell>) -> Style {
    let mut counts: Vec<(Style, usize)> = Vec::new();
    for cell in cells.filter(|cell| erasable(cell)) {
        if let Some(slot) = counts.iter().position(|(style, _)| *style == cell.style) {
            counts[slot].1 += 1;
        } else if counts.len() < 32 {
            counts.push((cell.style, 1));
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map_or(Style::default(), |(style, _)| style)
}

/// Cells the terminal moves instead of the frame rewriting them (ADR-740, ADR-744): the
/// `region` scrolled by `shift` rows — up (`SU`) when positive, the content of row `r + shift`
/// coming to row `r`; down (`SD`) when negative — and the cells it leaves blank filled in
/// `fill`, the current background when the scroll is written. A region of whole rows is
/// bounded by the top and bottom margins alone; a band of columns (one pane of a side-by-side
/// split) by the left and right margins too.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Scroll {
    region: Region,
    shift: i32,
    fill: Style,
}

impl Scroll {
    /// The margins set, the region scrolled, the margins back to the whole screen (which also
    /// homes the cursor; every write after it moves the cursor first). A band of columns sets
    /// the left and right margins inside left-right margin mode, as `margins` says the
    /// terminal has it — set for the scroll and reset after (`CSI ? 69 l` gives the margins
    /// back too), or on already, the margins then given back by `CSI s`.
    fn sequence(&self, cols: u16, margins: ColumnMargins) -> String {
        let region = self.region;
        let (count, direction) = if self.shift > 0 {
            (self.shift, 'S')
        } else {
            (-self.shift, 'T')
        };
        let rows = format!(
            "\x1b[{};{}r",
            region.top as u32 + 1,
            region.bottom as u32 + 1
        );
        if region.whole_rows(cols) {
            return format!("{rows}\x1b[{count}{direction}\x1b[r");
        }
        let columns = format!(
            "\x1b[{};{}s",
            region.left as u32 + 1,
            region.right as u32 + 1
        );
        let (enter, leave) = match margins {
            ColumnMargins::Held => ("", "\x1b[s"),
            ColumnMargins::Toggled | ColumnMargins::Unavailable => ("\x1b[?69h", "\x1b[?69l"),
        };
        format!("{enter}{rows}{columns}\x1b[{count}{direction}{leave}\x1b[r")
    }

    /// The rows the scroll leaves blank (in the region's columns).
    fn vacated(&self) -> std::ops::RangeInclusive<u16> {
        let count = self.shift.unsigned_abs() as u16;
        let region = self.region;
        if self.shift > 0 {
            region.bottom + 1 - count..=region.bottom
        } else {
            region.top..=region.top + count - 1
        }
    }
}

/// The fewest rows that must have moved to the same new place before a scroll is looked for.
const SCROLL_AT_LEAST: usize = 3;

/// About what a scroll's own sequences cost, in cells written: a scroll that saves no more
/// than this is not worth it.
const SCROLL_COST: i64 = 16;

/// What a band of columns adds to that: the left and right margins and the mode around them.
const COLUMN_SCROLL_COST: i64 = 16;

/// A row's content in `columns`, hashed: rows are matched by it before their cells are
/// compared.
fn row_hash(screen: &Screen, row: u16, columns: &std::ops::Range<usize>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    screen.segment(row, columns).hash(&mut hasher);
    hasher.finish()
}

/// How many cells of `row` of `next` differ from row `from` of `previous`, in `columns`: what
/// a rewrite of them costs, roughly.
fn row_cost(
    previous: &Screen,
    from: u16,
    next: &Screen,
    row: u16,
    columns: &std::ops::Range<usize>,
) -> i64 {
    previous
        .segment(from, columns)
        .iter()
        .zip(next.segment(row, columns))
        .filter(|(was, now)| was != now)
        .count() as i64
}

/// The scroll that turns most of `previous` into `next`, when one saves more than it costs.
///
/// Where the terminal has left and right margins (`output.column_margins`), first within the
/// columns anything changed in ([`changed_columns`]) — one pane of a side-by-side split, whose
/// rows match nowhere as whole screen rows since the pane beside it did not move — and
/// otherwise across whole rows ([`scroll_within`]).
fn find_scroll(previous: &Screen, next: &Screen, output: Output) -> Option<Scroll> {
    if !output.scroll_regions || previous.cols != next.cols || previous.rows != next.rows {
        return None;
    }
    let whole = 0..next.cols as usize;
    if output.column_margins != ColumnMargins::Unavailable
        && let Some(columns) = changed_columns(previous, next)
        && columns != whole
        && let Some(scroll) = scroll_within(previous, next, columns)
        && !splits_a_glyph(previous, scroll.region)
    {
        return Some(scroll);
    }
    scroll_within(previous, next, whole)
}

/// The scroll of a region in `columns` that turns most of `previous` into `next`, when one
/// saves more than it costs — in three stages: the shift rows moved by
/// ([`likeliest_shift`]), the band of rows that moved by it ([`best_band`]), and whether that
/// band, less what the rows the scroll leaves blank then cost ([`vacated_cost`]), is worth a
/// scroll.
fn scroll_within(
    previous: &Screen,
    next: &Screen,
    columns: std::ops::Range<usize>,
) -> Option<Scroll> {
    let shift = likeliest_shift(previous, next, &columns)?;
    let (first, last, gain) = best_band(previous, next, shift, &columns)?;
    let count = shift.unsigned_abs() as u16;
    let (top, bottom) = if shift > 0 {
        (first, last + count)
    } else {
        (first - count, last)
    };
    let region = Region {
        top,
        bottom,
        left: columns.start as u16,
        right: columns.end as u16 - 1,
    };
    let mut scroll = Scroll {
        region,
        shift,
        fill: Style::default(),
    };
    scroll.fill = commonest_blank_in(
        scroll
            .vacated()
            .flat_map(|row| next.segment(row, &columns).iter()),
    );
    let cost = if region.whole_rows(next.cols) {
        SCROLL_COST
    } else {
        SCROLL_COST + COLUMN_SCROLL_COST
    };
    (gain - vacated_cost(previous, next, scroll) > cost).then_some(scroll)
}

/// The columns every cell that changed from `previous` to `next` lies in, widened to take
/// whole the wide graphemes at their edges — `None` when nothing changed.
fn changed_columns(previous: &Screen, next: &Screen) -> Option<std::ops::Range<usize>> {
    let cols = next.cols as usize;
    let (mut left, mut right) = (cols, 0);
    for row in 0..next.rows {
        let (was, now) = (previous.row_cells(row), next.row_cells(row));
        if let Some(first) = (0..left).find(|&col| was[col] != now[col]) {
            left = first;
        }
        if let Some(last) = (right..cols).rev().find(|&col| was[col] != now[col]) {
            right = right.max(last);
        }
    }
    if left == cols {
        return None;
    }
    let continuation_at = |col: usize| {
        (0..next.rows).any(|row| {
            previous.row_cells(row)[col].continuation() || next.row_cells(row)[col].continuation()
        })
    };
    if left > 0 && continuation_at(left) {
        left -= 1;
    }
    if right + 1 < cols && continuation_at(right + 1) {
        right += 1;
    }
    Some(left..right + 1)
}

/// Whether moving `region` of `previous` would cut a wide grapheme at its left or right edge:
/// where a terminal puts the halves of one is its own affair, so no such region is scrolled.
fn splits_a_glyph(previous: &Screen, region: Region) -> bool {
    (region.top..=region.bottom).any(|row| {
        let cells = previous.row_cells(row);
        cells[region.left as usize].continuation()
            || cells
                .get(region.right as usize + 1)
                .is_some_and(Cell::continuation)
    })
}

/// The shift most rows that changed find their old content at — `source - row`, nonzero —
/// when at least [`SCROLL_AT_LEAST`] agree. Rows are matched whole in `columns`, by hash; a
/// content on more than two rows of `previous` (a blank line) says nothing of where it came
/// from.
fn likeliest_shift(
    previous: &Screen,
    next: &Screen,
    columns: &std::ops::Range<usize>,
) -> Option<i32> {
    let rows = next.rows;
    let previous_hashes: Vec<u64> = (0..rows)
        .map(|row| row_hash(previous, row, columns))
        .collect();
    let next_hashes: Vec<u64> = (0..rows).map(|row| row_hash(next, row, columns)).collect();
    let changed: Vec<u16> = (0..rows)
        .filter(|&row| previous_hashes[row as usize] != next_hashes[row as usize])
        .collect();
    if changed.len() < SCROLL_AT_LEAST {
        return None;
    }
    let mut found_at: std::collections::HashMap<u64, Vec<u16>> = Default::default();
    for (row, hash) in previous_hashes.iter().enumerate() {
        found_at.entry(*hash).or_default().push(row as u16);
    }
    let mut votes: std::collections::HashMap<i32, usize> = Default::default();
    for &row in &changed {
        if let Some(sources) = found_at.get(&next_hashes[row as usize])
            && sources.len() <= 2
        {
            for &source in sources {
                *votes.entry(source as i32 - row as i32).or_default() += 1;
            }
        }
    }
    // the most votes; between equals the smaller shift, then the larger (a total order, so
    // the map's order never decides)
    let (shift, count) = votes
        .into_iter()
        .filter(|&(shift, _)| shift != 0)
        .max_by_key(|&(shift, count)| (count, -shift.abs(), shift))?;
    (count >= SCROLL_AT_LEAST).then_some(shift)
}

/// The run of rows `first..=last` of `next` that gains most by reading each row's `columns`
/// from `row + shift` of `previous` instead of from the row in place — the gain the cells that
/// would differ in place, less those that differ shifted — and that gain; `None` when no run
/// gains. The maximum-sum run, found in one pass.
fn best_band(
    previous: &Screen,
    next: &Screen,
    shift: i32,
    columns: &std::ops::Range<usize>,
) -> Option<(u16, u16, i64)> {
    let rows = next.rows;
    let (mut best, mut best_gain) = (None, 0i64);
    let (mut start, mut gain) = (None::<u16>, 0i64);
    for row in 0..rows {
        let source = row as i32 + shift;
        if source < 0 || source >= rows as i32 {
            (start, gain) = (None, 0);
            continue;
        }
        let row_gain = row_cost(previous, row, next, row, columns)
            - row_cost(previous, source as u16, next, row, columns);
        if start.is_none() || gain <= 0 {
            (start, gain) = (Some(row), 0);
        }
        gain += row_gain;
        if gain > best_gain {
            (best, best_gain) = (start.map(|first| (first, row)), gain);
        }
    }
    best.map(|(first, last)| (first, last, best_gain))
}

/// What the cells `scroll` leaves blank cost once it has: those that are not its fill, less
/// what they would have cost in place.
fn vacated_cost(previous: &Screen, next: &Screen, scroll: Scroll) -> i64 {
    let blank = Cell::blank(scroll.fill);
    let columns = scroll.region.columns();
    scroll
        .vacated()
        .map(|row| {
            next.segment(row, &columns)
                .iter()
                .filter(|cell| **cell != blank)
                .count() as i64
                - row_cost(previous, row, next, row, &columns)
        })
        .sum()
}

/// How many of `cells`, from the first, are erasable blanks in the first one's style, and
/// that style — `None` when the first is no erasable blank. The one scan both a row's blank
/// tail (read from its end) and a blank stretch inside a run (read forward) are measured by.
fn leading_blanks<'a>(mut cells: impl Iterator<Item = &'a Cell>) -> Option<(u16, Style)> {
    let first = cells.next().filter(|cell| erasable(cell))?;
    let more = cells
        .take_while(|cell| erasable(cell) && cell.style == first.style)
        .count();
    Some((1 + more as u16, first.style))
}

/// The column from which every cell of `row` to its end is an erasable blank in one style,
/// and that style — when the run is long enough to be worth a `CSI K`.
fn blank_tail(screen: &Screen, row: u16) -> Option<(u16, Style)> {
    let (length, style) = leading_blanks(screen.row_cells(row).iter().rev())?;
    (length >= ERASE_AT_LEAST).then_some((screen.cols - length, style))
}

/// How many erasable blanks in one style start at `col` of `row`, up to `end` (inclusive).
fn blank_run(screen: &Screen, row: u16, col: u16, end: u16) -> u16 {
    let cells = &screen.row_cells(row)[col as usize..=end as usize];
    leading_blanks(cells.iter()).map_or(0, |(length, _)| length)
}

/// Append to `body` the writes that bring row `row` from `previous` to `next`: each run of
/// changed cells (runs closer than [`RUN_GAP`] joined) after one cursor move, a style
/// change written only where the style changes. Where `output` allows, a run reaching the
/// row's blank tail ends in one `CSI K`, and a long blank stretch inside a run is erased
/// (`CSI n X`) and stepped over (`CSI n C`). A run never starts on a wide grapheme's
/// continuation — it backs up to the grapheme — and a wide grapheme's continuation is never
/// written: the grapheme moved the terminal's cursor past it.
fn write_row_changes(
    previous: &Screen,
    next: &Screen,
    row: u16,
    output: Output,
    current_style: &mut Option<Style>,
    body: &mut String,
) {
    let depth = output.depth;
    let changed = |col: u16| previous.cell(row, col) != next.cell(row, col);
    let tail = if output.erase_fills_background {
        blank_tail(next, row)
    } else {
        None
    };
    let mut set_style = |style: Style, body: &mut String| {
        body.push_str(&sgr_change(*current_style, style, depth));
        *current_style = Some(style);
    };
    let mut col = 0u16;
    while col < next.cols {
        if !changed(col) {
            col += 1;
            continue;
        }
        let mut start = col;
        while start > 0 && next.cell(row, start).continuation() {
            start -= 1;
        }
        // the end of this run: the last changed cell with no unchanged gap of RUN_GAP after it
        let mut end = col;
        let mut probe = col + 1;
        while probe < next.cols && probe <= end + RUN_GAP {
            if changed(probe) {
                end = probe;
            }
            probe += 1;
        }
        body.push_str(&format!("\x1b[{};{}H", row as u32 + 1, start as u32 + 1));
        let mut write_col = start;
        while write_col <= end {
            if let Some((tail_from, tail_style)) = tail
                && write_col >= tail_from
            {
                // the rest of the row is one blank: erase it, and the row is done
                set_style(tail_style, body);
                body.push_str("\x1b[K");
                return;
            }
            let cell = next.cell(row, write_col);
            if cell.continuation() {
                write_col += 1;
                continue;
            }
            if output.erase_fills_background {
                let length = blank_run(next, row, write_col, end);
                if length >= ERASE_AT_LEAST {
                    set_style(cell.style, body);
                    body.push_str(&format!("\x1b[{length}X\x1b[{length}C"));
                    write_col += length;
                    continue;
                }
            }
            set_style(cell.style, body);
            body.push_str(&cell.text);
            write_col += 1;
        }
        // a wide grapheme at the run's end covered its continuation too
        col = end + 1;
        if col < next.cols && next.cell(row, col).continuation() {
            col += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red() -> Style {
        Style {
            fg: Some(Colour::Indexed(1)),
            ..Style::default()
        }
    }

    fn rendered(previous: Option<&Screen>, next: &Screen) -> String {
        let mut cursor = CursorState::default();
        let mut out = Vec::new();
        render(previous, next, ColorDepth::TrueColor, &mut cursor, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn text_lands_in_cells_and_is_clipped() {
        let mut screen = Screen::new(6, 2);
        screen.put_text(0, 2, "hello", Style::default());
        assert_eq!(screen.row_text(0), "  hell");
        screen.put_text(1, -2, "abcd", Style::default());
        assert_eq!(screen.row_text(1), "cd    ");
        screen.put_text(5, 0, "off screen", Style::default());
    }

    #[test]
    fn a_wide_grapheme_takes_two_cells_and_overwriting_half_blanks_the_other() {
        let mut screen = Screen::new(6, 1);
        screen.put_text(0, 0, "日本", Style::default());
        assert_eq!(screen.row_text(0), "日本  ");
        assert!(screen.cell(0, 1).continuation());
        // writing into the continuation of 日 blanks its head
        screen.put_text(0, 1, "x", Style::default());
        assert_eq!(screen.row_text(0), " x本  ");
        // writing over the head of 本 blanks its continuation
        screen.put_text(0, 2, "y", Style::default());
        assert_eq!(screen.row_text(0), " xy   ");
        // a wide grapheme cut by the right edge is a blank
        screen.put_text(0, 5, "日", Style::default());
        assert_eq!(screen.row_text(0), " xy   ");
    }

    #[test]
    fn a_control_character_draws_as_a_space_and_a_combining_mark_joins_its_base() {
        let mut screen = Screen::new(4, 1);
        screen.put_text(0, 0, "a\tb", Style::default());
        assert_eq!(screen.row_text(0), "a b ");
        screen.put_text(0, 0, "e\u{301}x", Style::default());
        assert_eq!(screen.cell(0, 0).text, "e\u{301}");
        assert_eq!(screen.cell(0, 1).text, "x");
    }

    /// The SGR slots a terminal draws with: each attribute or colour slot and its value.
    type DrawingState = std::collections::BTreeMap<&'static str, String>;

    /// The terminal's drawing state after the SGR sequences in `bytes`, for the attributes and
    /// colours `sgr` writes — a small interpreter of exactly what a terminal does with them.
    fn drawing_state(bytes: &str) -> Vec<String> {
        let mut state = DrawingState::new();
        for sequence in bytes.split("\x1b[").skip(1) {
            if let Some(parameters) = sequence.strip_suffix('m') {
                apply_sgr(&mut state, parameters);
            }
        }
        listed(&state)
    }

    fn listed(state: &DrawingState) -> Vec<String> {
        state
            .iter()
            .map(|(slot, value)| format!("{slot}={value}"))
            .collect()
    }

    /// One SGR sequence's `parameters` applied to `state`.
    fn apply_sgr(state: &mut DrawingState, parameters: &str) {
        let parts: Vec<&str> = parameters.split(';').collect();
        let mut at = 0;
        while at < parts.len() {
            let part = parts[at];
            let take_colour = |slot: &'static str, state: &mut DrawingState| {
                let width = if parts[at + 1] == "5" { 3 } else { 5 };
                state.insert(slot, parts[at..at + width].join(";")[3..].to_string());
                width
            };
            let step = match part {
                "0" => {
                    state.clear();
                    1
                }
                "1" => {
                    state.insert("bold", "on".into());
                    1
                }
                "22" => {
                    state.remove("bold");
                    1
                }
                "3" => {
                    state.insert("italic", "on".into());
                    1
                }
                "23" => {
                    state.remove("italic");
                    1
                }
                "4" | "4:3" => {
                    state.insert("underline", part.into());
                    1
                }
                "24" => {
                    state.remove("underline");
                    1
                }
                "7" => {
                    state.insert("reverse", "on".into());
                    1
                }
                "27" => {
                    state.remove("reverse");
                    1
                }
                "39" => {
                    state.remove("fg");
                    1
                }
                "49" => {
                    state.remove("bg");
                    1
                }
                "59" => {
                    state.remove("underline-colour");
                    1
                }
                "38" => take_colour("fg", state),
                "48" => take_colour("bg", state),
                "58" => take_colour("underline-colour", state),
                named => {
                    let number: u8 = named.parse().unwrap();
                    match number {
                        30..=37 | 90..=97 => {
                            state.insert("fg", named.into());
                        }
                        40..=47 | 100..=107 => {
                            state.insert("bg", named.into());
                        }
                        _ => panic!("unexpected SGR parameter {named}"),
                    }
                    1
                }
            };
            at += step;
        }
    }

    /// A style change leaves the terminal drawing exactly what a reset to the new style would,
    /// over every pair of a spread of styles, at every depth — and is never longer than it.
    #[test]
    fn a_style_change_draws_what_a_reset_would() {
        let colours = [
            None,
            Some(Colour::Indexed(1)),
            Some(Colour::Indexed(12)),
            Some(Colour::Rgb(30, 40, 50)),
        ];
        let mut styles = Vec::new();
        for (index, fg) in colours.iter().enumerate() {
            for bg in colours {
                styles.push(Style {
                    fg: *fg,
                    bg,
                    bold: index % 2 == 0,
                    italic: index == 1,
                    underline: [Underline::None, Underline::Curly, Underline::Straight][index % 3],
                    underline_colour: colours[(index + 1) % 4],
                    reverse: index == 3,
                });
            }
        }
        for depth in [
            ColorDepth::TrueColor,
            ColorDepth::Ansi256,
            ColorDepth::Ansi16,
        ] {
            for from in &styles {
                for to in &styles {
                    let change = sgr_change(Some(*from), *to, depth);
                    let before = sgr(*from, depth);
                    assert_eq!(
                        drawing_state(&format!("{before}{change}")),
                        drawing_state(&sgr(*to, depth)),
                        "{from:?} -> {to:?} at {depth:?}: {change:?}"
                    );
                    assert!(change.len() <= sgr(*to, depth).len());
                }
            }
        }
        assert_eq!(sgr_change(None, red(), ColorDepth::TrueColor), "\x1b[0;31m");
        assert_eq!(sgr_change(Some(red()), red(), ColorDepth::TrueColor), "");
    }

    fn rendered_erasing(previous: Option<&Screen>, next: &Screen) -> String {
        let mut cursor = CursorState::default();
        let mut out = Vec::new();
        let output = Output {
            depth: ColorDepth::TrueColor,
            erase_fills_background: true,
            scroll_regions: false,
            column_margins: ColumnMargins::Unavailable,
            synchronized: true,
        };
        render(previous, next, output, &mut cursor, &mut out);
        String::from_utf8(out).unwrap()
    }

    /// Where erasing fills with the background, the first frame clears to the commonest blank
    /// (those cells are never written), a row's blank tail is one `CSI K` and a blank stretch
    /// inside a run is erased and stepped over.
    #[test]
    fn blanks_are_erased_where_the_terminal_fills_with_the_background() {
        let ground = Style {
            bg: Some(Colour::Rgb(30, 30, 46)),
            ..Style::default()
        };
        let mut screen = Screen::filled(40, 3, ground);
        screen.put_text(0, 0, "abc", red());
        let first = rendered_erasing(None, &screen);
        assert!(
            first.starts_with("\x1b[?2026h\x1b[0;48;2;30;30;46m\x1b[2J"),
            "{first:?}"
        );
        assert!(
            !first.contains("\x1b[2;1H"),
            "blank rows are not written: {first:?}"
        );
        let mut next = screen.clone();
        next.put_text(0, 0, "x", red());
        next.fill(0, 1, 39, 1, ground);
        let edited = rendered_erasing(Some(&screen), &next);
        assert!(
            edited.contains("x\x1b[39m\x1b[K") || edited.contains("x\x1b[0;48;2;30;30;46m\x1b[K"),
            "{edited:?}"
        );
        let mut gapped = Screen::filled(40, 1, ground);
        gapped.put_text(0, 0, "left", red());
        gapped.put_text(0, 20, "right", red());
        let mut was = Screen::filled(40, 1, ground);
        was.put_text(0, 0, "#########################", red());
        let gap = rendered_erasing(Some(&was), &gapped);
        assert!(gap.contains("X\x1b[") && gap.contains("C"), "{gap:?}");
        // the same frames where erasing does not fill: every blank written
        let mut cursor = CursorState::default();
        let mut out = Vec::new();
        render(
            Some(&screen),
            &next,
            ColorDepth::TrueColor,
            &mut cursor,
            &mut out,
        );
        assert!(!String::from_utf8(out).unwrap().contains("\x1b[K"));
    }

    /// The terminal takes the default face's colours at true colour, cursor and all, and
    /// gives nothing at a depth whose cells are drawn in nearer colours.
    #[test]
    fn the_terminal_takes_the_default_colours_at_true_colour() {
        let face = Style {
            fg: Some(Colour::Rgb(0xcd, 0xd6, 0xf4)),
            bg: Some(Colour::Rgb(0x1e, 0x1e, 0x2e)),
            ..Style::default()
        };
        assert_eq!(
            default_colours(face, ColorDepth::TrueColor),
            "\x1b]11;rgb:1e/1e/2e\x1b\\\x1b]10;rgb:cd/d6/f4\x1b\\\x1b]12;rgb:cd/d6/f4\x1b\\"
        );
        assert_eq!(default_colours(face, ColorDepth::Ansi256), "");
        assert_eq!(default_colours(Style::default(), ColorDepth::TrueColor), "");
    }

    /// A theme's colours are given back before another's are set, and a face (or a depth)
    /// that sets none leaves the terminal its own — never a reset for colours never set,
    /// which would undo what the user's shell gave the terminal.
    #[test]
    fn the_terminals_colours_change_with_the_face() {
        let dark = Style {
            bg: Some(Colour::Rgb(0x1e, 0x1e, 0x2e)),
            ..Style::default()
        };
        let (first, held) = default_colours_change(dark, ColorDepth::TrueColor, false);
        assert_eq!((first.as_str(), held), ("\x1b]11;rgb:1e/1e/2e\x1b\\", true));
        let (again, held) = default_colours_change(dark, ColorDepth::TrueColor, true);
        assert_eq!(
            (again, held),
            (format!("{DEFAULT_COLOURS_RESET}{first}"), true)
        );
        assert_eq!(
            default_colours_change(Style::default(), ColorDepth::TrueColor, true),
            (DEFAULT_COLOURS_RESET.to_string(), false)
        );
        assert_eq!(
            default_colours_change(dark, ColorDepth::Ansi256, false),
            (String::new(), false)
        );
    }

    /// One list of true-colour terminals, read by `$TERM` and by an XTVERSION answer alike.
    #[test]
    fn a_true_colour_terminal_is_known_by_either_name() {
        for name in [
            "xterm-kitty",
            "foot",
            "xterm-ghostty",
            "kitty(0.36.4)",
            "WezTerm 2024",
        ] {
            assert!(names_true_colour_terminal(name), "{name}");
        }
        for name in ["xterm-256color", "XTerm(390)", "tmux 3.4", "screen", ""] {
            assert!(!names_true_colour_terminal(name), "{name}");
        }
    }

    /// A row's blank tail and a blank stretch inside it are the same measure, read from either
    /// end: blanks in one style, a cell of another style or a glyph ending them.
    #[test]
    fn blanks_are_measured_in_one_style() {
        let ground = Style {
            bg: Some(Colour::Indexed(4)),
            ..Style::default()
        };
        let mut screen = Screen::filled(20, 1, ground);
        screen.put_text(0, 2, "ab", red());
        screen.fill(0, 10, 2, 1, Style::default());
        assert_eq!(blank_tail(&screen, 0), Some((12, ground)));
        assert_eq!(blank_run(&screen, 0, 4, 19), 6);
        assert_eq!(blank_run(&screen, 0, 2, 19), 0);
        assert_eq!(blank_run(&screen, 0, 4, 6), 3);
        screen.fill(0, 16, 4, 1, Style::default());
        assert_eq!(
            blank_tail(&screen, 0),
            None,
            "four blanks are not worth an erase"
        );
    }

    #[test]
    fn erasing_follows_term() {
        assert!(
            Output::detect(ColorDepth::TrueColor, Some("xterm-256color")).erase_fills_background
        );
        assert!(
            !Output::detect(ColorDepth::Ansi256, Some("screen-256color")).erase_fills_background
        );
        assert!(!Output::detect(ColorDepth::Ansi16, None).erase_fills_background);
    }

    #[test]
    fn the_first_frame_clears_and_writes_what_is_not_blank() {
        let mut screen = Screen::new(10, 2);
        screen.put_text(1, 3, "hi", red());
        let bytes = rendered(None, &screen);
        assert!(bytes.contains("\x1b[2J"));
        assert!(bytes.contains("\x1b[2;4H\x1b[31mhi"), "{bytes:?}");
        assert!(
            !bytes.contains("\x1b[1;1H"),
            "blank row 0 is not written: {bytes:?}"
        );
    }

    #[test]
    fn an_unchanged_frame_writes_nothing() {
        let mut screen = Screen::new(10, 2);
        screen.put_text(0, 0, "same", Style::default());
        assert_eq!(rendered(Some(&screen.clone()), &screen), "");
    }

    #[test]
    fn only_the_changed_cells_are_written() {
        let mut before = Screen::new(20, 3);
        before.put_text(0, 0, "the quick brown fox", Style::default());
        before.put_text(2, 0, "untouched", Style::default());
        let mut after = before.clone();
        after.put_text(0, 4, "QUICK", Style::default());
        let bytes = rendered(Some(&before), &after);
        assert!(bytes.contains("\x1b[1;5H"), "{bytes:?}");
        assert!(bytes.contains("QUICK"));
        assert!(!bytes.contains("brown"));
        assert!(!bytes.contains("untouched"));
        assert!(!bytes.contains("\x1b[2J"));
    }

    #[test]
    fn close_changes_are_one_run_and_far_ones_two() {
        let mut before = Screen::new(30, 1);
        before.put_text(0, 0, "abcdefghijklmnopqrstuvwxyz", Style::default());
        let mut close = before.clone();
        close.put_text(0, 0, "A", Style::default());
        close.put_text(0, 3, "D", Style::default());
        let bytes = rendered(Some(&before), &close);
        assert_eq!(bytes.matches('H').count(), 1, "{bytes:?}");
        assert!(bytes.contains("AbcD"));
        let mut far = before.clone();
        far.put_text(0, 0, "A", Style::default());
        far.put_text(0, 20, "U", Style::default());
        assert_eq!(rendered(Some(&before), &far).matches('H').count(), 2);
    }

    #[test]
    fn a_change_inside_a_wide_grapheme_rewrites_the_grapheme() {
        let mut before = Screen::new(6, 1);
        before.put_text(0, 0, "ab", Style::default());
        let mut after = before.clone();
        after.put_text(0, 0, "日", Style::default());
        let bytes = rendered(Some(&before), &after);
        assert!(bytes.contains("\x1b[1;1H\x1b[0m日"), "{bytes:?}");
    }

    #[test]
    fn the_cursor_is_placed_shown_and_shaped_once() {
        let mut screen = Screen::new(10, 2);
        screen.cursor = Some(Cursor {
            row: 1,
            col: 4,
            shape: CursorShape::Bar,
        });
        let mut cursor = CursorState::default();
        let mut out = Vec::new();
        render(None, &screen, ColorDepth::TrueColor, &mut cursor, &mut out);
        let first = String::from_utf8(out).unwrap();
        assert!(first.contains("\x1b[2;5H\x1b[6 q\x1b[?25h"), "{first:?}");
        // the same cursor again: nothing to write
        let mut out = Vec::new();
        render(
            Some(&screen.clone()),
            &screen,
            ColorDepth::TrueColor,
            &mut cursor,
            &mut out,
        );
        assert!(out.is_empty());
        // no cursor in the next frame: hidden
        let mut hidden = screen.clone();
        hidden.cursor = None;
        let mut out = Vec::new();
        render(
            Some(&screen),
            &hidden,
            ColorDepth::TrueColor,
            &mut cursor,
            &mut out,
        );
        assert!(String::from_utf8(out).unwrap().contains("\x1b[?25l"));
        assert!(!cursor.visible);
    }

    #[test]
    fn a_face_takes_the_default_faces_colours_it_does_not_name() {
        let default = Style {
            fg: Some(Colour::Rgb(205, 214, 244)),
            bg: Some(Colour::Rgb(30, 30, 46)),
            ..Style::default()
        };
        let face = Style {
            fg: Some(Colour::Indexed(1)),
            bold: true,
            ..Style::default()
        };
        let drawn = face.over(default);
        assert_eq!(drawn.fg, Some(Colour::Indexed(1)));
        assert_eq!(drawn.bg, Some(Colour::Rgb(30, 30, 46)));
        assert!(drawn.bold);
        let mut screen = Screen::new(3, 1);
        screen.clear(default);
        assert_eq!(screen.cell(0, 2).style, default);
    }

    #[test]
    fn text_with_no_background_keeps_the_rect_under_it() {
        // a current-line band or a selection bar, then the line's text in a face naming only
        // a foreground (or none): the band stays under the text, as a window draws it
        let default = Style {
            bg: Some(Colour::Rgb(30, 30, 46)),
            ..Style::default()
        };
        let band = Style {
            bg: Some(Colour::Rgb(49, 50, 68)),
            ..Style::default()
        };
        let mut screen = Screen::new(8, 2);
        screen.clear(default);
        screen.fill(0, 0, 8, 1, band);
        screen.put_text(0, 1, "hello", red());
        screen.put_text(1, 1, "world", red());
        assert_eq!(screen.row_text(0), " hello  ");
        for col in 0..8 {
            assert_eq!(
                screen.cell(0, col).style.bg,
                band.bg,
                "the band under column {col}"
            );
        }
        assert_eq!(screen.cell(0, 2).style.fg, red().fg);
        // off the band, the text is on the background the frame cleared to
        assert_eq!(screen.cell(1, 2).style.bg, default.bg);
        // a face that names its own background still paints it
        let own = Style {
            bg: Some(Colour::Indexed(4)),
            ..red()
        };
        screen.put_text(0, 1, "x", own);
        assert_eq!(screen.cell(0, 1).style.bg, Some(Colour::Indexed(4)));
        // a wide grapheme over the band keeps it in both of its cells
        screen.put_text(0, 4, "日", red());
        assert_eq!(screen.cell(0, 4).style.bg, band.bg);
        assert_eq!(screen.cell(0, 5).style.bg, band.bg);
    }

    #[test]
    fn a_resize_repaints_everything() {
        let mut small = Screen::new(5, 1);
        small.put_text(0, 0, "abc", Style::default());
        let mut large = Screen::new(8, 2);
        large.put_text(0, 0, "abc", Style::default());
        let bytes = rendered(Some(&small), &large);
        assert!(bytes.contains("\x1b[2J") && bytes.contains("abc"));
    }

    #[test]
    fn colours_degrade_to_what_the_terminal_shows() {
        let style = Style {
            fg: Some(Colour::Rgb(255, 0, 0)),
            bg: Some(Colour::Rgb(30, 30, 46)),
            ..Style::default()
        };
        assert_eq!(
            sgr(style, ColorDepth::TrueColor),
            "\x1b[0;38;2;255;0;0;48;2;30;30;46m"
        );
        assert_eq!(sgr(style, ColorDepth::Ansi256), "\x1b[0;38;5;196;48;5;235m");
        assert_eq!(sgr(style, ColorDepth::Ansi16), "\x1b[0;91;40m");
        // a named colour stays the terminal's own at every depth
        assert_eq!(sgr(red(), ColorDepth::Ansi16), "\x1b[0;31m");
        assert_eq!(
            sgr(
                Style {
                    fg: Some(Colour::Indexed(12)),
                    ..Style::default()
                },
                ColorDepth::Ansi256
            ),
            "\x1b[0;38;5;12m"
        );
    }

    #[test]
    fn attributes_and_the_curly_underline() {
        let style = Style {
            bold: true,
            italic: true,
            underline: Underline::Curly,
            underline_colour: Some(Colour::Rgb(1, 2, 3)),
            reverse: true,
            ..Style::default()
        };
        assert_eq!(
            sgr(style, ColorDepth::TrueColor),
            "\x1b[0;1;3;4:3;7;58;2;1;2;3m"
        );
        assert_eq!(sgr(style, ColorDepth::Ansi16), "\x1b[0;1;3;4;7m");
    }

    #[test]
    fn the_depth_is_read_from_colorterm_and_term() {
        assert_eq!(
            ColorDepth::detect(Some("truecolor"), Some("xterm")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::detect(Some("24bit"), None),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::detect(None, Some("xterm-256color")),
            ColorDepth::Ansi256
        );
        assert_eq!(
            ColorDepth::detect(None, Some("xterm-direct")),
            ColorDepth::TrueColor
        );
        assert_eq!(
            ColorDepth::detect(None, Some("tmux-256color")),
            ColorDepth::Ansi256
        );
        assert_eq!(
            ColorDepth::detect(None, Some("xterm-kitty")),
            ColorDepth::TrueColor
        );
        assert_eq!(ColorDepth::detect(None, Some("linux")), ColorDepth::Ansi16);
        assert_eq!(ColorDepth::detect(None, None), ColorDepth::Ansi16);
    }

    #[test]
    fn the_nearest_palette_colours() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(128, 128, 128), 244);
        assert_eq!(nearest_16(250, 10, 10), 9);
        assert_eq!(nearest_16(0, 0, 0), 0);
        assert_eq!(rgb_of_index(196), (255, 0, 0));
        assert_eq!(rgb_of_index(232), (8, 8, 8));
    }

    /// A model terminal for the bytes [`render`] writes: the cursor moves, text, SGR, the
    /// erases (`CSI 2J`, `CSI K`, `CSI n X`, `CSI n C`), the scroll margins — top and bottom
    /// (`DECSTBM`), and left and right (`DECSLRM`) in left-right margin mode (`CSI ? 69 h`,
    /// whose reset gives them back) — and `SU`/`SD` within them: an erased or scrolled-in cell
    /// keeps only the current background, as a terminal that fills with it does. Text advances
    /// by its graphemes' Unicode widths with grapheme clustering (mode 2027, as `clusters` or
    /// `CSI ? 2027 h` set it), else by its code points' widths, as `wcwidth` does. Each cell is
    /// its text and the drawing state it was written in.
    struct ModelTerminal {
        cols: usize,
        rows: usize,
        cells: Vec<(String, Vec<String>)>,
        row: usize,
        col: usize,
        state: DrawingState,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        margin_mode: bool,
        clusters: bool,
    }

    impl ModelTerminal {
        fn new(cols: u16, rows: u16) -> ModelTerminal {
            ModelTerminal {
                cols: cols as usize,
                rows: rows as usize,
                cells: vec![(" ".to_string(), Vec::new()); cols as usize * rows as usize],
                row: 0,
                col: 0,
                state: DrawingState::new(),
                top: 0,
                bottom: rows as usize - 1,
                left: 0,
                right: cols as usize - 1,
                margin_mode: false,
                clusters: true,
            }
        }

        /// A model terminal that advances by code points until told otherwise (`CSI ? 2027 h`).
        fn measuring_code_points(cols: u16, rows: u16) -> ModelTerminal {
            ModelTerminal {
                clusters: false,
                ..ModelTerminal::new(cols, rows)
            }
        }

        /// A blank in the current background: what an erase or a scroll leaves.
        fn erased(&self) -> (String, Vec<String>) {
            let background = self
                .state
                .get("bg")
                .map(|bg| vec![format!("bg={bg}")])
                .unwrap_or_default();
            (" ".to_string(), background)
        }

        fn erase(&mut self, row: usize, from: usize, to: usize) {
            let blank = self.erased();
            for col in from..to.min(self.cols) {
                self.cells[row * self.cols + col] = blank.clone();
            }
        }

        /// Scroll the margins' cells by `shift` rows: up when positive, down when negative.
        fn scroll(&mut self, shift: i64) {
            let mut moved = self.cells.clone();
            let blank = self.erased();
            for row in self.top..=self.bottom {
                let source = row as i64 + shift;
                for col in self.left..=self.right {
                    moved[row * self.cols + col] =
                        if (self.top as i64..=self.bottom as i64).contains(&source) {
                            self.cells[source as usize * self.cols + col].clone()
                        } else {
                            blank.clone()
                        };
                }
            }
            self.cells = moved;
        }

        /// The margins `parameters` name (`first;last`, 1-based), or `whole` for none.
        fn margins(parameters: &str, whole: usize) -> (usize, usize) {
            let margins = match parameters.split_once(';') {
                Some((first, last)) => (
                    first.parse::<usize>().unwrap() - 1,
                    last.parse::<usize>().unwrap() - 1,
                ),
                None => (0, whole - 1),
            };
            assert!(margins.0 < margins.1, "{parameters}");
            margins
        }

        /// How far the cursor moves for `grapheme`, as this terminal measures.
        fn advance(&self, grapheme: &str) -> usize {
            use unicode_width::UnicodeWidthChar;
            if self.clusters {
                grapheme.width()
            } else {
                grapheme.chars().map(|c| c.width().unwrap_or(0)).sum()
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            let text = std::str::from_utf8(bytes).unwrap();
            let mut rest = text;
            while !rest.is_empty() {
                if let Some(sequence) = rest.strip_prefix("\x1b[") {
                    let end = sequence.find(|c: char| ('@'..='~').contains(&c)).unwrap();
                    let (parameters, final_byte) = (&sequence[..end], &sequence[end..end + 1]);
                    rest = &sequence[end + 1..];
                    let number = |default: usize| parameters.parse().unwrap_or(default);
                    match final_byte {
                        "m" => apply_sgr(&mut self.state, parameters),
                        "H" => {
                            let (row, col) = parameters.split_once(';').unwrap_or(("1", "1"));
                            self.row = row.parse::<usize>().unwrap() - 1;
                            self.col = col.parse::<usize>().unwrap() - 1;
                        }
                        "J" => {
                            assert_eq!(parameters, "2");
                            for row in 0..self.rows {
                                self.erase(row, 0, self.cols);
                            }
                        }
                        "K" => self.erase(self.row, self.col, self.cols),
                        "X" => self.erase(self.row, self.col, self.col + number(1)),
                        "C" => self.col = (self.col + number(1)).min(self.cols - 1),
                        "r" => {
                            (self.top, self.bottom) = ModelTerminal::margins(parameters, self.rows);
                            (self.row, self.col) = (0, 0);
                        }
                        "s" => {
                            // DECSLRM — only in left-right margin mode, where `CSI s` is no
                            // longer "save the cursor"
                            assert!(self.margin_mode, "CSI {parameters}s outside mode 69");
                            (self.left, self.right) = ModelTerminal::margins(parameters, self.cols);
                            (self.row, self.col) = (0, 0);
                        }
                        "S" => self.scroll(number(1) as i64),
                        "T" => self.scroll(-(number(1) as i64)),
                        "h" | "l" => {
                            let set = final_byte == "h";
                            match parameters {
                                "?69" => {
                                    self.margin_mode = set;
                                    (self.left, self.right) = (0, self.cols - 1);
                                }
                                "?2027" => self.clusters = set,
                                // the cursor's visibility, synchronized updates
                                _ => {}
                            }
                        }
                        // the cursor's shape
                        "q" => {}
                        other => panic!("unexpected CSI {parameters:?}{other}"),
                    }
                } else if let Some(sequence) = rest.strip_prefix("\x1b]") {
                    // an OSC: the terminal's own colours, nothing drawn
                    rest = &sequence[sequence.find("\x1b\\").unwrap() + 2..];
                } else {
                    let end = rest.find('\x1b').unwrap_or(rest.len());
                    for grapheme in rest[..end].graphemes(true) {
                        if self.col >= self.cols {
                            // past the edge: this model clips, as no frame here wraps
                            break;
                        }
                        let advance = self.advance(grapheme);
                        let state = listed(&self.state);
                        let at = self.row * self.cols + self.col;
                        self.cells[at] = (grapheme.to_string(), state.clone());
                        for more in 1..advance.min(self.cols - self.col) {
                            self.cells[at + more] = (String::new(), state.clone());
                        }
                        self.col += advance;
                    }
                    rest = &rest[end..];
                }
            }
        }

        /// Whether the model shows `screen`: every cell's text in its style at `depth`, an
        /// erasable blank only by its background (which is all an erased cell shows).
        fn assert_shows(&self, screen: &Screen, depth: ColorDepth, context: &str) {
            for row in 0..screen.rows {
                for col in 0..screen.cols {
                    let want = screen.cell(row, col);
                    let (text, state) = &self.cells[row as usize * self.cols + col as usize];
                    let wanted_state = drawing_state(&sgr(want.style, depth));
                    let background = |state: &Vec<String>| {
                        state.iter().find(|slot| slot.starts_with("bg=")).cloned()
                    };
                    if erasable(want) {
                        assert!(
                            text == " " && background(state) == background(&wanted_state),
                            "{context}: cell {row},{col} is {text:?} {state:?}, wanted a blank on {:?}",
                            background(&wanted_state)
                        );
                    } else {
                        assert!(
                            *text == want.text && *state == wanted_state,
                            "{context}: cell {row},{col} is {text:?} {state:?}, wanted {:?} {wanted_state:?}",
                            want.text
                        );
                    }
                }
            }
        }
    }

    /// A small deterministic generator for the test documents.
    struct Generator(u64);

    impl Generator {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) % bound
        }
    }

    const GROUND: Style = Style {
        fg: Some(Colour::Rgb(205, 214, 244)),
        bg: Some(Colour::Rgb(30, 30, 46)),
        bold: false,
        italic: false,
        underline: Underline::None,
        underline_colour: None,
        reverse: false,
    };

    /// Lines of words in a few colours, some blank, some with a wide grapheme.
    fn document(lines: usize, seed: u64) -> Vec<Vec<(String, Style)>> {
        let mut generator = Generator(seed);
        let colours = [
            Colour::Rgb(243, 139, 168),
            Colour::Rgb(166, 227, 161),
            Colour::Indexed(4),
            Colour::Rgb(250, 179, 135),
        ];
        (0..lines)
            .map(|line| {
                if generator.next(5) == 0 {
                    return Vec::new();
                }
                let mut words = vec![(format!("{line:>3} "), GROUND)];
                for _ in 0..generator.next(7) + 1 {
                    let word = match generator.next(9) {
                        0 => "日本".to_string(),
                        length => "abcdefghij"[..length as usize].to_string(),
                    };
                    let style = Style {
                        fg: Some(colours[generator.next(4) as usize]),
                        bold: generator.next(6) == 0,
                        underline: if generator.next(10) == 0 {
                            Underline::Curly
                        } else {
                            Underline::None
                        },
                        ..GROUND
                    };
                    words.push((format!("{word} "), style));
                }
                words
            })
            .collect()
    }

    /// A pane of `document` from line `top`, at column `left`, `width` wide and `height` rows
    /// tall, with a scrollbar thumb in its last column.
    fn draw_pane(
        screen: &mut Screen,
        document: &[Vec<(String, Style)>],
        top: usize,
        left: i64,
        width: i64,
        height: i64,
    ) {
        screen.fill(0, left, width, height, GROUND);
        for row in 0..height {
            let mut col = left;
            for (text, style) in document.get(top + row as usize).into_iter().flatten() {
                let room = (left + width - 1 - col).max(0) as usize;
                let shown: String = text.chars().take(room).collect();
                screen.put_text(row, col, &shown, *style);
                col += shown.width() as i64;
            }
        }
        let thumb = (top as i64 * height / document.len() as i64).min(height - 3);
        let bar = Style {
            bg: Some(Colour::Rgb(88, 91, 112)),
            ..GROUND
        };
        screen.fill(thumb, left + width - 1, 1, 3, bar);
    }

    fn frame_at(document: &[Vec<(String, Style)>], top: usize) -> Screen {
        let mut screen = Screen::filled(60, 16, GROUND);
        draw_pane(&mut screen, document, top, 0, 60, 14);
        let mode_line = Style {
            bg: Some(Colour::Rgb(49, 50, 68)),
            reverse: false,
            ..GROUND
        };
        screen.fill(14, 0, 60, 1, mode_line);
        screen.put_text(14, 1, &format!("view.blsp  top {top}"), mode_line);
        screen.put_text(15, 0, "echo", GROUND);
        screen
    }

    fn erasing_output() -> Output {
        Output::detect(ColorDepth::TrueColor, Some("xterm-256color"))
    }

    /// Replay `frames` through [`render`] onto the model terminal, checking after each that it
    /// shows exactly the frame; the bytes written in all.
    fn replay(frames: &[Screen], output: Output) -> String {
        replay_onto(
            ModelTerminal::new(frames[0].cols, frames[0].rows),
            frames,
            output,
        )
    }

    /// [`replay`] onto `model`.
    fn replay_onto(mut model: ModelTerminal, frames: &[Screen], output: Output) -> String {
        let mut cursor = CursorState::default();
        let mut previous: Option<&Screen> = None;
        let mut written = String::new();
        for (index, frame) in frames.iter().enumerate() {
            let mut out = Vec::new();
            render(previous, frame, output, &mut cursor, &mut out);
            model.feed(&out);
            model.assert_shows(frame, output.depth, &format!("frame {index}"));
            written.push_str(&String::from_utf8(out).unwrap());
            previous = Some(frame);
        }
        written
    }

    /// Rows that moved are moved by the terminal (`DECSTBM` + `SU`/`SD`), and what it then
    /// shows is cell for cell the frame — up and down, by one row and by a page, past blank
    /// lines, wide graphemes and a scrollbar that moved too — in fewer bytes than a rewrite.
    #[test]
    fn a_scroll_region_draws_what_a_rewrite_would() {
        let document = document(120, 7);
        let tops = [0, 1, 4, 3, 16, 30, 27, 27, 90, 100, 99, 0];
        let frames: Vec<Screen> = tops.iter().map(|&top| frame_at(&document, top)).collect();
        let scrolled = replay(&frames, erasing_output());
        assert!(
            scrolled.contains("S\x1b[r") && scrolled.contains("T\x1b[r"),
            "{scrolled:?}"
        );
        let rewritten = replay(
            &frames,
            Output {
                scroll_regions: false,
                ..erasing_output()
            },
        );
        assert!(!rewritten.contains("\x1b[r"));
        assert!(
            scrolled.len() * 10 < rewritten.len() * 7,
            "{} scrolled, {} rewritten",
            scrolled.len(),
            rewritten.len()
        );
    }

    /// A scroll by one row moves the band and writes only the row it brought in (and the
    /// chrome that changed): the mode line and the echo area below it stay out of the region.
    #[test]
    fn a_one_row_scroll_writes_one_row() {
        let document = document(60, 11);
        let (before, after) = (frame_at(&document, 10), frame_at(&document, 11));
        let mut cursor = CursorState::default();
        let mut out = Vec::new();
        render(
            Some(&before),
            &after,
            erasing_output(),
            &mut cursor,
            &mut out,
        );
        let written = String::from_utf8(out).unwrap();
        assert!(written.contains("\x1b[1;14r\x1b[1S\x1b[r"), "{written:?}");
        let mut model = ModelTerminal::new(60, 16);
        let mut first = Vec::new();
        render(
            None,
            &before,
            erasing_output(),
            &mut CursorState::default(),
            &mut first,
        );
        model.feed(&first);
        model.feed(written.as_bytes());
        model.assert_shows(&after, ColorDepth::TrueColor, "one row");
    }

    /// Where erasing does not fill with the background (GNU screen), a scrolled-in row would
    /// not be the theme's: no scroll regions there, and none on the Linux console.
    #[test]
    fn scroll_regions_follow_term() {
        assert!(Output::detect(ColorDepth::TrueColor, Some("xterm-kitty")).scroll_regions);
        assert!(!Output::detect(ColorDepth::Ansi256, Some("screen-256color")).scroll_regions);
        assert!(!Output::detect(ColorDepth::Ansi16, Some("linux")).scroll_regions);
        let document = document(60, 13);
        let frames: Vec<Screen> = [0, 2, 4]
            .iter()
            .map(|&top| frame_at(&document, top))
            .collect();
        let written = replay(
            &frames,
            Output::detect(ColorDepth::TrueColor, Some("screen-256color")),
        );
        assert!(!written.contains("\x1b[r"));
    }
    /// Frames of two panes side by side, 60×12 — the left one scrolled to each of `tops` and
    /// the right one still, or the other way round — with a divider and a mode line under each
    /// that names its top line.
    fn side_by_side(tops: &[usize], left_scrolls: bool) -> Vec<Screen> {
        let left = document(80, 3);
        let right = document(80, 5);
        let divider = Style {
            bg: Some(Colour::Rgb(49, 50, 68)),
            ..GROUND
        };
        tops.iter()
            .map(|&top| {
                let (left_top, right_top) = if left_scrolls { (top, 20) } else { (20, top) };
                let mut screen = Screen::filled(60, 13, GROUND);
                draw_pane(&mut screen, &left, left_top, 0, 29, 12);
                screen.fill(0, 29, 1, 13, divider);
                draw_pane(&mut screen, &right, right_top, 30, 30, 12);
                screen.put_text(12, 1, &format!("left {left_top}"), divider);
                screen.put_text(12, 31, &format!("right {right_top}"), divider);
                screen
            })
            .collect()
    }

    /// The left and right margins (1-based) every `DECSLRM` in `written` sets.
    fn column_bands(written: &str) -> Vec<(u16, u16)> {
        written
            .split("\x1b[")
            .filter_map(|sequence| {
                let (left, right) = sequence.strip_suffix('s')?.split_once(';')?;
                Some((left.parse().ok()?, right.parse().ok()?))
            })
            .collect()
    }

    /// Side by side, one pane scrolling and the other still, on a terminal without left and
    /// right margins: no screen row moved whole, so no scroll region (it would move the still
    /// pane too) — the rows are diffed.
    #[test]
    fn a_pane_beside_a_still_one_is_diffed_not_scrolled() {
        let frames = side_by_side(&[0, 3, 6, 2], true);
        let written = replay(&frames, erasing_output());
        assert!(!written.contains("\x1b[r"), "{written:?}");
    }

    /// With left and right margins (`DECLRMM`), the scrolling pane is moved by the terminal in
    /// a region of its own columns (`DECSTBM` + `DECSLRM` + `SU`/`SD`) and the pane beside it
    /// stays — what the terminal then shows is cell for cell the frame, either pane scrolling,
    /// up and down, whether the mode is set around each scroll or on already — in fewer bytes
    /// than the diff.
    #[test]
    fn a_pane_beside_a_still_one_scrolls_in_its_columns() {
        for left_scrolls in [true, false] {
            let frames = side_by_side(&[0, 1, 2, 5, 4, 3, 6, 9, 8, 30, 27], left_scrolls);
            let diffed = replay(&frames, erasing_output());
            for margins in [ColumnMargins::Toggled, ColumnMargins::Held] {
                let output = Output {
                    column_margins: margins,
                    ..erasing_output()
                };
                let mut model = ModelTerminal::new(60, 13);
                if margins == ColumnMargins::Held {
                    model.feed(b"\x1b[?69h");
                }
                let scrolled = replay_onto(model, &frames, output);
                // every band inside the scrolling pane's columns (1-based)
                let pane = if left_scrolls { 1..=29 } else { 31..=60 };
                let bands = column_bands(&scrolled);
                assert!(
                    !bands.is_empty()
                        && bands
                            .iter()
                            .all(|(left, right)| pane.contains(left) && pane.contains(right)),
                    "{margins:?}: {bands:?}"
                );
                assert_eq!(
                    scrolled.contains("\x1b[?69h"),
                    margins == ColumnMargins::Toggled,
                    "{scrolled:?}"
                );
                assert!(
                    scrolled.len() * 10 < diffed.len() * 7,
                    "{margins:?}: {} scrolled in columns, {} diffed",
                    scrolled.len(),
                    diffed.len()
                );
            }
        }
    }

    /// `text` as a Brood string literal.
    fn brood_string(text: &str) -> String {
        let mut literal = String::from("\"");
        for character in text.chars() {
            match character {
                '\\' => literal.push_str("\\\\"),
                '"' => literal.push_str("\\\""),
                '\x1b' => literal.push_str("\\e"),
                control if control < ' ' || control == '\x7f' => {
                    literal.push_str(&format!("\\x{:02x}", control as u32))
                }
                other => literal.push(other),
            }
        }
        literal.push('"');
        literal
    }

    /// std/vt — the terminal bedit's pty drivers read the editor through — plays the column
    /// scrolls as [`ModelTerminal`] does (ADR-747): the bytes of every frame of either pane
    /// scrolling, either margin mode, fed to `vt/feed` show each frame's rows, row for row.
    /// The two models are one spec checked from both sides on the same byte stream.
    #[test]
    fn std_vt_plays_the_column_scrolls_as_the_model_does() {
        for left_scrolls in [true, false] {
            let frames = side_by_side(&[0, 1, 2, 5, 4, 3, 6, 9, 8, 30, 27], left_scrolls);
            for margins in [ColumnMargins::Toggled, ColumnMargins::Held] {
                let output = Output {
                    column_margins: margins,
                    ..erasing_output()
                };
                let mut model = ModelTerminal::new(60, 13);
                let prelude = if margins == ColumnMargins::Held {
                    "\x1b[?69h"
                } else {
                    ""
                };
                model.feed(prelude.as_bytes());
                let mut cursor = CursorState::default();
                let mut previous: Option<&Screen> = None;
                let (mut chunks, mut wanted) = (Vec::new(), Vec::new());
                for frame in &frames {
                    let mut out = Vec::new();
                    render(previous, frame, output, &mut cursor, &mut out);
                    model.feed(&out);
                    model.assert_shows(frame, output.depth, "the model");
                    chunks.push(brood_string(&String::from_utf8(out).unwrap()));
                    let rows = (0..frame.rows)
                        .map(|row| brood_string(frame.row_text(row).trim_end()))
                        .collect::<Vec<_>>();
                    wanted.push(format!("[{}]", rows.join(" ")));
                    previous = Some(frame);
                }
                let program = format!(
                    "(letrec (go (fn (vt chunks wanted index)
                       (if (empty? chunks)
                         -1
                         (let (next (vt/feed vt (first chunks))
                               shown (into [] (map (vt/lines next) string/trim-right)))
                           (if (= shown (first wanted))
                             (go next (rest chunks) (rest wanted) (inc index))
                             index)))))
                       (go (vt/feed (vt/make 60 13) {}) (list {}) (list {}) 0))",
                    brood_string(prelude),
                    chunks.join(" "),
                    wanted.join(" ")
                );
                let mut interp = crate::Interp::new();
                let first_wrong = interp.eval_str(&program).expect("std/vt runs").as_int();
                assert_eq!(
                    first_wrong,
                    Some(-1),
                    "{margins:?}, left scrolls {left_scrolls}: std/vt shows frame {first_wrong:?} otherwise"
                );
            }
        }
    }

    /// A column band is never scrolled where it would cut a wide grapheme in two at its edge;
    /// the columns of the changes take a wide grapheme at their edge whole.
    #[test]
    fn a_column_band_keeps_wide_graphemes_whole() {
        let mut previous = Screen::filled(10, 4, GROUND);
        for row in 0..4 {
            previous.put_text(row, 3, "日", GROUND);
        }
        let region = Region {
            top: 0,
            bottom: 3,
            left: 4,
            right: 9,
        };
        assert!(splits_a_glyph(&previous, region));
        assert!(!splits_a_glyph(&previous, Region { left: 3, ..region }));
        assert!(!splits_a_glyph(&previous, Region { left: 5, ..region }));
        let mut next = previous.clone();
        next.put_text(2, 4, "x", GROUND);
        assert_eq!(changed_columns(&previous, &next), Some(3..5));
        assert_eq!(changed_columns(&previous, &previous), None);
    }

    /// Without grapheme clustering a terminal advances by each code point: a cluster whose code
    /// points add up to another width than its own is drawn as its first code point, in the
    /// cells it was laid out in, so the text after it lands where it should — on a terminal
    /// measuring code points as on one measuring clusters (mode 2027), which is drawn the
    /// cluster whole.
    #[test]
    fn a_cluster_is_drawn_as_the_terminal_measures_it() {
        let text =
            "a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b\u{2764}\u{FE0F}c\u{1F44D}\u{1F3FD}d";
        let draw = |clusters: bool| {
            let mut screen = Screen::filled(12, 1, GROUND);
            screen.clusters = clusters;
            screen.put_text(0, 0, text, GROUND);
            screen
        };
        let by_code_points = draw(false);
        let cells = |screen: &Screen| -> Vec<String> {
            (0..screen.cols)
                .map(|col| screen.cell(0, col).text.clone())
                .collect()
        };
        assert_eq!(
            cells(&by_code_points),
            [
                "a",
                "\u{1F468}",
                "",
                "b",
                "\u{2764}",
                " ",
                "c",
                "\u{1F44D}",
                "",
                "d",
                " ",
                " "
            ]
        );
        let by_clusters = draw(true);
        assert_eq!(
            by_clusters.cell(0, 1).text,
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"
        );
        assert_eq!(by_clusters.cell(0, 4).text, "\u{2764}\u{FE0F}");
        assert_eq!(by_clusters.cell(0, 9).text, "d");
        // each drawn as its terminal measures, every cell where the screen has it
        for (screen, model) in [
            (by_code_points, ModelTerminal::measuring_code_points(12, 1)),
            (by_clusters, ModelTerminal::new(12, 1)),
        ] {
            replay_onto(model, &[screen], erasing_output());
        }
        // the cluster whole on a terminal measuring code points puts what follows astray
        let mut astray = ModelTerminal::measuring_code_points(12, 1);
        let mut out = Vec::new();
        render(
            None,
            &draw(true),
            erasing_output(),
            &mut CursorState::default(),
            &mut out,
        );
        astray.feed(&out);
        assert_ne!(astray.cells[3].0, "b");
    }
}
