//! The windowed (GUI) frontend for the M3 display/input seam — ADR-046's second
//! frontend, alongside the terminal (crossterm) one in `builtins.rs`.
//!
//! The display *protocol* is unchanged: a frame is a vector of render ops
//! (`[:clear]`, `[:text row col s face]`, `[:cursor row col]`) — plain Brood data.
//! This module paints that frame to a native window instead of a terminal, and
//! reads keystrokes back in the same encoding (`"a"`, `:up`, `:ctrl-c`, …). So
//! `std/tool/observer.blsp`, the REPL editor, and the future editor drive it through
//! the identical `gui-*` ⇆ `term-*` surface and never know which backend is live.
//!
//! ## Threading & multiple windows
//!
//! A GUI toolkit insists on owning a thread + event loop, and winit allows only
//! **one** event loop per process — so a single dedicated **GUI thread** owns it
//! and multiplexes *every* window from a registry. The Brood side bridges with
//! channels, the same synchronous shape the `term-*` seam has, with the toolkit's
//! loop-ownership contained entirely behind these primitives:
//!
//! * `gui-open subscriber` asks the thread (via an `EventLoopProxy` user-event) to
//!   create a window whose input is delivered to process `subscriber`; replies with
//!   the window's integer id. The thread starts lazily on the first call.
//! * `gui-draw id` ships the frame as plain `Op`s to that window; the thread stores
//!   it and repaints. `gui-size id` reads a shared `(cols, rows)` updated on resize.
//! * `gui-close id` destroys one window. The thread itself never exits (winit can't
//!   restart a loop); it idles when no windows are open.
//!
//! **Input never blocks a worker (ADR-058).** Rather than handing keys back through
//! a channel the Brood side polls, the GUI thread turns each key/mouse event into a
//! `Message` and `deliver`s it straight to the subscriber's mailbox — so the
//! observer parks in `(receive)` (holding no scheduler worker) instead of pinning
//! one in a blocking poll. Each window is independent, so `(observe)` spawns one
//! observer process per window. Only Send data crosses (`Op` to the thread,
//! `Message` to the mailbox); the windows/surfaces/glyph caches never leave the GUI
//! thread. The whole backend is behind the `gui` cargo feature; without it the
//! primitives return an error naming the way to get it back (`./configure --with-gui
//! && make install`) so the symbols still exist uniformly.

/// A resolved text face: colours as RGB (already mapped from `:fg`/`:bg`
/// keywords by the caller, which has heap access), the attribute flags, the
/// optional font family (an interned `:family` keyword id, resolved to a loaded
/// font set by the renderer; `None` = the default family), and an integer font
/// `scale` (≥1): the op's text is drawn `scale`× larger, occupying a
/// `scale`×`scale` block of base cells anchored at its `(row, col)`. The terminal
/// frontend has no notion of scale and renders 1×. See ADR-079.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Face {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub family: Option<u32>,
    pub scale: u16,
}

impl Default for Face {
    /// The default face: unstyled, default family, scale 1 — a derived `Default`
    /// would give `scale: 0`, but scale is always at least one cell.
    fn default() -> Self {
        Face {
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            reverse: false,
            family: None,
            scale: 1,
        }
    }
}

/// What `gui-open` can decide about a window only while it is being *built* — as
/// opposed to the `gui-*!` primitives, which mutate a window that already exists.
/// One struct rather than a growing positional tail, since every one of these rides
/// the same path: the opts map -> `open` -> the GUI thread's `Open` event (or its
/// pre-`resumed` queue) -> `build_window`.
#[derive(Clone)]
pub struct WindowSpec {
    /// OS title-bar text; `None` gives the `"Brood"` default. Changeable later with
    /// `gui-title!`.
    pub title: Option<String>,
    /// Inner size in logical pixels; `None` is the 840×560 default.
    pub size: Option<(f64, f64)>,
    /// False for a borderless window — no OS title bar or frame, so an app that draws
    /// its own chrome (a browser's tab strip + toolbar) owns the whole surface instead
    /// of sitting under a redundant second title.
    pub decorations: bool,
    /// The desktop application id — Wayland's `app_id`, X11's `WM_CLASS`. This is how
    /// a desktop matches the window to the installed `.desktop` entry that names it,
    /// and therefore the only way it gets a real icon and name in a GNOME dash or
    /// alt-tab: unset (the default), a Brood window is unmatchable and draws the
    /// desktop's generic fallback icon. Distinct from `gui-icon!` — a Wayland client
    /// cannot hand the compositor pixels at all, so on Wayland *this* is the icon
    /// mechanism and `gui-icon!` does nothing.
    pub app_id: Option<String>,
}

impl Default for WindowSpec {
    /// The plain window: default title and size, decorated (a derived `Default` would
    /// give a borderless one), no app id.
    fn default() -> Self {
        WindowSpec {
            title: None,
            size: None,
            decorations: true,
            app_id: None,
        }
    }
}

/// The pointer shape a cursor zone requests — frontend-neutral (mapped to a winit
/// `CursorIcon` only inside the GUI backend). `ColResize` is the ↔ used for a
/// side-by-side (`:col`) split's divider; `RowResize` the ↕ for a stacked (`:row`)
/// one. (ADR-080.)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    ColResize,
    RowResize,
    /// A hand / link pointer — the `:pointer` shape a clickable text region (a
    /// results-buffer row, a mode-line segment) requests to read as a link.
    Pointer,
}

/// How the text cursor is drawn at its cell. `Block` (the default) overlays the
/// whole cell — the terminal-style caret; `Bar` is a thin vertical line on the
/// cell's left edge (a modern GUI insertion caret); `Underline` a thin rule along
/// the cell bottom. A `[:cursor row col]` op with no style is `Block`, so existing
/// callers are unchanged. The terminal frontend maps these to crossterm's steady
/// cursor styles.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

/// One render op, parsed out of a frame vector into plain (Send) data so it can
/// cross to the GUI thread. Mirrors the protocol `term-draw` interprets.
///
/// `PartialEq` is what the backend's frame diff runs on: a frame equal to the last one
/// is not repainted at all, and within a changed frame only the cell rows whose op
/// sequence differs are re-rasterised (`paint::strip_diff`).
#[derive(Clone, PartialEq)]
pub enum Op {
    Clear,
    Text {
        row: u16,
        col: u16,
        s: String,
        face: Face,
    },
    Cursor {
        row: u16,
        col: u16,
        style: CursorStyle,
    },
    /// Fill a `w`×`h` block of cells from `(row, col)` with `face`'s background — a
    /// solid panel (a popup card, a selection band, a gutter wash). Like a multi-row
    /// `bar`, but a real rectangle the renderer fills directly. The terminal frontend
    /// space-fills each row so the `:bg` shows.
    Rect {
        row: u16,
        col: u16,
        w: u16,
        h: u16,
        face: Face,
        /// Corner radius in **cell units**, 0.0 for a square panel. The GUI rounds
        /// the fill; the terminal frontend ignores it and space-fills as always, so
        /// one op serves both frontends (the same asymmetry `CursorZone` and
        /// `FRect` already rely on). Rounded chrome — a pill address field, a
        /// button highlight — no longer needs an `FRect` for the window plus a
        /// `Rect` for the terminal.
        radius: f32,
    },
    /// A **sub-cell** rounded rectangle: like `Rect`, but its position and size are in
    /// **cell units as floats** (`x`/`y` top-left, `w`/`h` size — `0.4` cells wide,
    /// `12.7` tall at `y = 3.25` are all legal), so it isn't snapped to the character
    /// grid. The GUI multiplies by the cell metrics + grid origin to land on pixels and
    /// fills an anti-aliased, alpha-blended (`opacity` 0..1) rounded (`radius`, in cell
    /// units) quad — the primitive a smooth scrollbar thumb, slider, or progress bar
    /// needs. GUI-only: the terminal frontend has no arm, so it's skipped (like
    /// `VSpans`). `face`'s `:bg` (or `:fg` under `:reverse`) is the fill colour.
    FRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        face: Face,
        opacity: f32,
        radius: f32,
    },
    /// A rectangular hot-zone (cells) that asks the frontend to show `shape` while
    /// the pointer is over it — e.g. a resize cursor on a window divider. The GUI
    /// hit-tests it on pointer-move; the terminal ignores it. (ADR-080.)
    CursorZone {
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        shape: CursorShape,
    },
    /// A batch of vertical column-spans — the fast path for column renderers
    /// (raycasters, spectrum bars, heat columns). `cols[i]` describes the cell
    /// column `col0 + i` as a top-to-bottom run of `(height-in-cells, color)`
    /// segments painted from `row0` down; a `None` color leaves the background
    /// showing through. Each segment is a flat filled rectangle — no glyph
    /// shaping — and the O(cells) per-cell expansion happens here in Rust, not in
    /// the Brood frame builder, so a wide scene that an op-per-cell frame can't
    /// build fast enough becomes O(columns) of Brood work. The terminal frontend
    /// ignores it (a GUI-only op, like a `:scale` face). (ADR-046 display seam.)
    VSpans {
        row0: u16,
        col0: u16,
        cols: Vec<Vec<(u16, Option<[u8; 3]>)>>,
    },
    /// A whole BITBOARD blitted as one op — the sparse-cell fast path for grid
    /// sims (Game of Life, cellular automata). `bits` is an arbitrary-precision
    /// integer in which set bit `y*w + x` means cell `(x, y)` is live; each live
    /// cell is filled as an `aspect`×1 screen-cell rectangle in `color`, anchored
    /// at screen cell `(row0, col0)`. Enumerating the set bits and expanding them
    /// to rects happens here in Rust (O(live), like `bit/positions`), so a frame
    /// of thousands of live cells costs the Brood side ONE op instead of one
    /// op-per-cell. GUI-only — the terminal ignores it, like `:vspans`.
    Cells {
        row0: u16,
        col0: u16,
        w: u32,
        aspect: u16,
        /// The board's set bits as little-endian bytes (bit `y*w+x` = byte `i/8` bit `i%8`).
        /// Decoded once at parse time from EITHER a bignum or a byte string, so the paint path
        /// is representation-agnostic — a plain set-bit byte scan.
        bytes: Vec<u8>,
        color: Option<[u8; 3]>,
    },
    /// Like `Cells`, but each live cell is filled with its OWN colour from `colors`
    /// (bit-index → rgb), falling back to `default`. One op for a whole COLOURED board,
    /// so the SIM emits a single op instead of an interpreted `[:text]` per live cell
    /// (the per-cell op-build was ~110 ms for 10K cells). The colour map is decoded once
    /// at parse time. GUI-only.
    CellsRgb {
        row0: u16,
        col0: u16,
        w: u32,
        aspect: u16,
        bytes: Vec<u8>,
        colors: std::collections::HashMap<u64, [u8; 3]>,
        default: [u8; 3],
    },
    /// Draw `ops` with every `Text`, `Rect`, and `Cursor` op shifted upward by
    /// `dy_frac × cell_h` pixels — the scoped, self-resetting primitive for pixel-accurate
    /// smooth scrolling. Unlike the old `ScrollOffset` sentinel pair, the offset is
    /// automatically contained to this block: ops outside the region are never affected.
    /// Regions may be nested (inner overrides outer). GUI-only: the terminal flattens inner
    /// ops and ignores the offset. ADR-114.
    ScrollRegion {
        dy_frac: f32,
        ops: Vec<Op>,
    },
}

/// A keystroke, in a backend-neutral shape the Brood side turns into the same
/// values `term-poll` yields: `Char` → a 1-char string, the rest → keywords
/// (`:ctrl-c`, `:alt-f`, `:up`, …). `PartialEq` so the event loop can tell an
/// auto-repeat (a press for the key *already* held) from a fresh press without
/// trusting winit's `ke.repeat` flag — unreliable on Wayland (ADR-086).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
    CtrlAlt(char),
    Named(&'static str),
}

/// The name of a named key under modifiers — ONE rule for both frontends, so a chord
/// reads the same whether it arrived from winit or from crossterm: `[ctrl-meta-|ctrl-|
/// alt-][shift-]<name>` — `:ctrl-left`, `:alt-shift-up`, `:ctrl-meta-delete`. Character
/// chords already spell their modifiers this way (`:ctrl-x`, `:ctrl-meta-f`); named keys
/// used to keep only Shift, and only on the motion keys, so `C-<left>` (Emacs
/// `right-word`) and `C-S-<arrow>` (swap a window with its neighbour) could not be bound
/// at all. Tab keeps its own spelling (Shift+Tab is `:back-tab`) and Escape carries none.
///
/// Returns a `&'static str` — the vocabulary is closed (a dozen keys × eight modifier
/// sets), so each spelling is built once and kept for the process, which lets [`Key`]
/// stay `Copy`.
pub fn named_key(base: &'static str, ctrl: bool, alt: bool, shift: bool) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    if !(ctrl || alt || shift) {
        return base;
    }
    let name = format!(
        "{}{}{}",
        match (ctrl, alt) {
            (true, true) => "ctrl-meta-",
            (true, false) => "ctrl-",
            (false, true) => "alt-",
            (false, false) => "",
        },
        if shift { "shift-" } else { "" },
        base
    );
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap();
    if let Some(s) = names.get(name.as_str()) {
        return s;
    }
    let leaked: &'static str = Box::leak(name.into_boxed_str());
    names.insert(leaked);
    leaked
}

#[cfg(test)]
mod named_key_tests {
    use super::named_key;
    #[test]
    fn spells_modifiers_the_way_character_chords_do() {
        assert_eq!(named_key("left", false, false, false), "left");
        assert_eq!(named_key("left", false, false, true), "shift-left");
        assert_eq!(named_key("left", true, false, false), "ctrl-left");
        assert_eq!(named_key("up", false, true, false), "alt-up");
        assert_eq!(named_key("up", true, true, false), "ctrl-meta-up");
        assert_eq!(named_key("right", true, false, true), "ctrl-shift-right");
        assert_eq!(named_key("delete", false, true, true), "alt-shift-delete");
    }
    #[test]
    fn the_same_chord_is_the_same_static_str() {
        let a = named_key("home", true, false, true);
        let b = named_key("home", true, false, true);
        assert!(std::ptr::eq(a, b));
    }
}

/// A mouse button, mirrored from winit's; the Brood side keywords it (`:left`).
#[derive(Clone, Copy, PartialEq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// What the mouse did. `Scroll*` carry no button. `Drag` is motion with a button
/// held; `Release` is the button coming back up — together they let an app track a
/// press→drag→release gesture (dragging a window divider to resize, ADR-077). Bare
/// motion (no button) is still not emitted — no consumer, and a per-pixel event
/// would flood. `Drag` is throttled to **cell granularity** (emitted only when the
/// pointer crosses into a new character cell, not per pixel), which is what made
/// adding it safe where ADR-056 had deferred it. The crossterm frontend maps to
/// this same set, so one `[:mouse …]` shape covers both.
#[derive(Clone, Copy)]
pub enum MouseAction {
    Press,
    Release,
    Drag,
    Move,
    ScrollUp,
    ScrollDown,
}

/// A mouse event at a character-cell position; the Brood side turns it into a
/// `[:mouse action button row col mods]` vector (`button` is nil for scroll;
/// `mods` is a vector of the held modifier keywords, e.g. `[:ctrl]` / `[]`, so an
/// app can bind Ctrl+wheel, Ctrl+drag, etc.). A **press** also carries a `count`
/// (1 = single, 2 = double, 3 = triple, …) — consecutive presses of the same button
/// in the same cell within the double-click window — appended as a trailing 7th
/// element `[… mods count]`; for every other action `count` is 0 and omitted.
/// A **scroll** also carries a `scroll_dy` — the delta in *line units* (positive =
/// scroll-up, i.e. away from the user): 1.0 per wheel notch for `LineDelta`,
/// `pixel_delta / cell_h` for `PixelDelta` (trackpad). Appended as trailing 7th
/// element for scroll events. When 0.0 (non-scroll) the field is omitted.
#[derive(Clone, Copy)]
pub struct Mouse {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub row: u16,
    pub col: u16,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Click chain length for a press (1/2/3/…); 0 for non-press actions (omitted).
    pub count: u8,
    /// Scroll delta in line units (positive = up); 0.0 for non-scroll actions.
    pub scroll_dy: f64,
}

#[cfg(not(feature = "gui"))]
const NOT_COMPILED: &str = "gui backend not compiled in — this `brood` was built without it. \
     From the brood repo: `./configure --with-gui && make install` (note that ./configure starts \
     from the defaults, so pass every option you want on one line). Building with cargo directly: \
     `--features gui`.";

#[cfg(not(feature = "gui"))]
mod disabled;

#[cfg(feature = "gui")]
pub(crate) mod backend;

#[cfg(feature = "gui-gpu")]
pub(crate) mod gpu; // the experimental OpenGL render path behind `BROOD_GUI_GPU=1`

#[cfg(not(feature = "gui"))]
pub use disabled::{
    bg, close, drag_move, drag_resize, draw, focus, font, fullscreen, grab, held_key,
    host_main_thread, icon, inset, line_height, maximize, minimize, open, register_family, size,
    text_aa, text_contrast, title, TextAa,
};

#[cfg(feature = "gui")]
pub use backend::{
    bg, close, drag_move, drag_resize, draw, focus, font, fullscreen, grab, held_key,
    host_main_thread, icon, inset, line_height, maximize, minimize, open, register_family, size,
    text_aa, text_contrast, title, TextAa,
};
