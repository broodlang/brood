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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
        let known_256 = ["kitty", "alacritty", "foot", "wezterm", "ghostty", "tmux"];
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Underline {
    #[default]
    None,
    Straight,
    Curly,
}

/// How one cell is drawn: its colours and attributes. `None` colours are the terminal's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
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
#[derive(Clone, PartialEq, Eq, Debug)]
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
}

/// The width a grapheme takes on screen and the text drawn for it: a control character (a
/// tab, an escape) is drawn as a space — written raw it would move the terminal's cursor
/// and every cell after it would be painted in the wrong place — and a grapheme wider than
/// two cells is counted as two, the most a terminal gives one.
fn glyph(grapheme: &str) -> (usize, &str) {
    if grapheme.chars().any(char::is_control) {
        return (1, " ");
    }
    (grapheme.width().min(2), grapheme)
}

impl Screen {
    /// A blank screen of `cols` × `rows`, no cursor shown.
    pub fn new(cols: u16, rows: u16) -> Screen {
        Screen {
            cols,
            rows,
            cells: vec![Cell::blank(Style::default()); cols as usize * rows as usize],
            cursor: None,
        }
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
            let (width, drawn) = glyph(grapheme);
            if width == 0 {
                continue;
            }
            let style = if style.bg.is_some() || at < 0 {
                style
            } else {
                Style {
                    bg: self.cell(row, (at.min(self.cols as i64 - 1)) as u16).style.bg,
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
                if width == 2 {
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

/// The SGR sequence that draws in `style` at `depth`, from a reset: `ESC [ 0 ; … m`.
pub fn sgr(style: Style, depth: ColorDepth) -> String {
    let mut parameters = vec!["0".to_string()];
    if style.bold {
        parameters.push("1".into());
    }
    if style.italic {
        parameters.push("3".into());
    }
    match style.underline {
        Underline::None => {}
        Underline::Straight => parameters.push("4".into()),
        // the curly underline (`4:3`) only where colon sub-parameters are safe to send: a
        // terminal with true colour reads them; an older one may take `4:3` for something else
        Underline::Curly if depth == ColorDepth::TrueColor => parameters.push("4:3".into()),
        Underline::Curly => parameters.push("4".into()),
    }
    if style.reverse {
        parameters.push("7".into());
    }
    let colour = |colour: Colour, base: u8, bright: u8, extended: u8| match depth.degrade(colour) {
        Colour::Indexed(index) if index < 8 => format!("{}", base + index),
        Colour::Indexed(index) if index < 16 && depth == ColorDepth::Ansi16 => {
            format!("{}", bright + index - 8)
        }
        Colour::Indexed(index) => format!("{extended};5;{index}"),
        Colour::Rgb(red, green, blue) => format!("{extended};2;{red};{green};{blue}"),
    };
    if let Some(fg) = style.fg {
        parameters.push(colour(fg, 30, 90, 38));
    }
    if let Some(bg) = style.bg {
        parameters.push(colour(bg, 40, 100, 48));
    }
    if let Some(underline_colour) = style.underline_colour
        && style.underline != Underline::None
        && depth != ColorDepth::Ansi16
    {
        parameters.push(match depth.degrade(underline_colour) {
            Colour::Indexed(index) => format!("58;5;{index}"),
            Colour::Rgb(red, green, blue) => format!("58;2;{red};{green};{blue}"),
        });
    }
    format!("\x1b[{}m", parameters.join(";"))
}

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

/// The bytes that turn the terminal showing `previous` into showing `next`, at `depth`,
/// appended to `out`. With no `previous`, or one of another size, the screen is cleared and
/// every non-blank cell written. Only changed cells are written otherwise; a frame that
/// changes nothing writes nothing. The whole update is bracketed as a synchronized update
/// (`CSI ?2026`), which a terminal that knows it paints at once and any other ignores, with
/// the cursor hidden while it moves. `cursor` is what the terminal's cursor was doing and
/// becomes what it does now.
pub fn render(
    previous: Option<&Screen>,
    next: &Screen,
    depth: ColorDepth,
    cursor: &mut CursorState,
    out: &mut Vec<u8>,
) {
    let blank;
    let (previous, cleared) = match previous {
        Some(screen) if screen.cols == next.cols && screen.rows == next.rows => (screen, false),
        _ => {
            blank = Screen::new(next.cols, next.rows);
            (&blank, true)
        }
    };
    let mut body = String::new();
    if cleared {
        body.push_str("\x1b[0m\x1b[2J");
    }
    let mut current_style: Option<Style> = if cleared {
        Some(Style::default())
    } else {
        None
    };
    for row in 0..next.rows {
        write_row_changes(previous, next, row, depth, &mut current_style, &mut body);
    }
    let wanted_shape = next.cursor.map(|at| at.shape);
    let cursor_changes = next.cursor.is_some() != cursor.visible
        || (next.cursor.is_some() && wanted_shape != cursor.shape)
        || (next.cursor.is_some() && previous.cursor != next.cursor);
    if body.is_empty() && !cursor_changes {
        return;
    }
    let mut frame = String::from("\x1b[?2026h");
    if cursor.visible && !body.is_empty() {
        frame.push_str("\x1b[?25l");
    }
    frame.push_str(&body);
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
    frame.push_str("\x1b[?2026l");
    out.extend_from_slice(frame.as_bytes());
}

/// Append to `body` the writes that bring row `row` from `previous` to `next`: each run of
/// changed cells (runs closer than [`RUN_GAP`] joined) after one cursor move, a style
/// change written only where the style changes. A run never starts on a wide grapheme's
/// continuation — it backs up to the grapheme — and a wide grapheme's continuation is never
/// written: the grapheme moved the terminal's cursor past it.
fn write_row_changes(
    previous: &Screen,
    next: &Screen,
    row: u16,
    depth: ColorDepth,
    current_style: &mut Option<Style>,
    body: &mut String,
) {
    let changed = |col: u16| previous.cell(row, col) != next.cell(row, col);
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
        for write_col in start..=end {
            let cell = next.cell(row, write_col);
            if cell.continuation() {
                continue;
            }
            if *current_style != Some(cell.style) {
                body.push_str(&sgr(cell.style, depth));
                *current_style = Some(cell.style);
            }
            body.push_str(&cell.text);
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

    #[test]
    fn the_first_frame_clears_and_writes_what_is_not_blank() {
        let mut screen = Screen::new(10, 2);
        screen.put_text(1, 3, "hi", red());
        let bytes = rendered(None, &screen);
        assert!(bytes.contains("\x1b[2J"));
        assert!(bytes.contains("\x1b[2;4H\x1b[0;31mhi"));
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
            assert_eq!(screen.cell(0, col).style.bg, band.bg, "the band under column {col}");
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
}
