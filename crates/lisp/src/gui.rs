//! The windowed (GUI) frontend for the M3 display/input seam — ADR-046's second
//! frontend, alongside the terminal (crossterm) one in `builtins.rs`.
//!
//! The display *protocol* is unchanged: a frame is a vector of render ops
//! (`[:clear]`, `[:text row col s face]`, `[:cursor row col]`) — plain Brood data.
//! This module paints that frame to a native window instead of a terminal, and
//! reads keystrokes back in the same encoding (`"a"`, `:up`, `:ctrl-c`, …). So
//! `std/observer.blsp`, the REPL editor, and the future editor drive it through
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
//! primitives return a clear "rebuild with --features gui" error so the symbols
//! still exist uniformly.

/// A resolved text face: colours as RGB (already mapped from `:fg`/`:bg`
/// keywords by the caller, which has heap access), the attribute flags, the
/// optional font family (an interned `:family` keyword id, resolved to a loaded
/// font set by the renderer; `None` = the default family), and an integer font
/// `scale` (≥1): the op's text is drawn `scale`× larger, occupying a
/// `scale`×`scale` block of base cells anchored at its `(row, col)`. The terminal
/// frontend has no notion of scale and renders 1×. See ADR-079.
#[derive(Clone, Copy)]
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

/// The pointer shape a cursor zone requests — frontend-neutral (mapped to a winit
/// `CursorIcon` only inside the GUI backend). `ColResize` is the ↔ used for a
/// side-by-side (`:col`) split's divider; `RowResize` the ↕ for a stacked (`:row`)
/// one. (ADR-080.)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    ColResize,
    RowResize,
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

/// A mouse button, mirrored from winit's; the Brood side keywords it (`:left`).
#[derive(Clone, Copy)]
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
/// app can bind Ctrl+wheel, Ctrl+drag, etc.).
#[derive(Clone, Copy)]
pub struct Mouse {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub row: u16,
    pub col: u16,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[cfg(not(feature = "gui"))]
const NOT_COMPILED: &str = "gui backend not compiled in; rebuild with `--features gui`";

#[cfg(not(feature = "gui"))]
mod disabled {
    use super::Op;
    use super::NOT_COMPILED;
    pub fn open(
        _subscriber: u64,
        _title: Option<String>,
        _size: Option<(f64, f64)>,
    ) -> Result<u64, String> {
        Err(NOT_COMPILED.into())
    }
    pub fn close(_id: u64) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn title(_id: u64, _title: String) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn icon(_id: u64, _rgba: Vec<u8>, _w: u32, _h: u32) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn focus(_id: u64) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn grab(_id: u64, _on: bool) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn size(_id: u64) -> Result<(u16, u16), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn held_key(_id: u64) -> Result<Option<super::Key>, String> {
        Err(NOT_COMPILED.into())
    }
    pub fn draw(_id: u64, _ops: Vec<Op>) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn font(_id: Option<u64>, _family: Option<u32>, _px: Option<f32>) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn register_family(
        _name: u32,
        _regular: Vec<u8>,
        _bold: Vec<u8>,
        _italic: Vec<u8>,
        _bold_italic: Vec<u8>,
    ) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
}

#[cfg(not(feature = "gui"))]
pub use disabled::{
    close, draw, focus, font, grab, held_key, icon, open, register_family, size, title,
};

#[cfg(feature = "gui")]
pub use backend::{
    close, draw, focus, font, grab, held_key, icon, open, register_family, size, title,
};

#[cfg(feature = "gui")]
mod backend {
    use super::{Key, Mouse, MouseAction, MouseButton, Op};
    use crate::core::value;
    use crate::process::{deliver, Message};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::num::NonZeroU32;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex, OnceLock};

    use winit::application::ApplicationHandler;
    use winit::dpi::{LogicalSize, PhysicalPosition};
    use winit::event::{
        ElementState, KeyEvent, MouseButton as WMouseButton, MouseScrollDelta, WindowEvent,
    };
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::keyboard::{Key as WKey, ModifiersState, NamedKey, PhysicalKey};
    use winit::platform::wayland::EventLoopBuilderExtWayland;
    use winit::window::{CursorGrabMode, CursorIcon, Icon, Window, WindowId};

    use cosmic_text::{
        fontdb, Attrs, Buffer as CtBuffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
        SwashContent, Weight,
    };
    use unicode_segmentation::UnicodeSegmentation;

    use crate::text_width::cluster_cells;

    /// The winit cursor for a frontend-neutral `CursorShape`.
    fn cursor_icon(shape: super::CursorShape) -> CursorIcon {
        match shape {
            super::CursorShape::ColResize => CursorIcon::EwResize, // ↔ side-by-side divider
            super::CursorShape::RowResize => CursorIcon::NsResize, // ↕ stacked divider
        }
    }

    /// The cursor shape for the pointer at cell `(col, row)`, given the window's
    /// zones — the first zone containing the point, or None (default cursor).
    fn shape_at(zones: &[(u16, u16, u16, u16, super::CursorShape)], col: u16, row: u16) -> Option<super::CursorShape> {
        zones.iter().find_map(|&(x, y, w, h, shape)| {
            if col >= x && col < x + w && row >= y && row < y + h {
                Some(shape)
            } else {
                None
            }
        })
    }

    // Bundled monospace font, four styles (see assets/README.md) — the default
    // `:mono` family; a face's :bold/:italic pick the style. No system font discovery.
    const FONT_REGULAR: &[u8] = include_bytes!("../assets/DejaVuSansMono.ttf");
    const FONT_BOLD: &[u8] = include_bytes!("../assets/DejaVuSansMono-Bold.ttf");
    const FONT_ITALIC: &[u8] = include_bytes!("../assets/DejaVuSansMono-Oblique.ttf");
    const FONT_BOLD_ITALIC: &[u8] = include_bytes!("../assets/DejaVuSansMono-BoldOblique.ttf");
    // Bundled color emoji font (CBDT), loaded only as a *fallback*: a cluster the mono
    // font can't cover (an emoji, a flag, a CJK char, …) is shaped + rasterised from
    // here by cosmic-text/swash, in color. Not a selectable `:family`. ~11 MB.
    const FONT_EMOJI: &[u8] = include_bytes!("../assets/NotoColorEmoji.ttf");
    // The family name fontdb assigns the bundled mono faces — what we pass as the
    // primary `Attrs` family; cosmic-text's fallback list then reaches the emoji font.
    const MONO_FAMILY: &str = "DejaVu Sans Mono";
    // The default font family keyword (`:mono`), and the default cell pixel size.
    const DEFAULT_FAMILY: &str = "mono";
    const DEFAULT_PX: f32 = 15.0;

    // A terminal-ish dark theme for unstyled cells.
    const DEFAULT_BG: [u8; 3] = [0x10, 0x14, 0x18];
    const DEFAULT_FG: [u8; 3] = [0xcd, 0xd6, 0xe0];
    // The solid colour of a thin (bar / underline) cursor caret — crisp near-white,
    // since the cursor op carries no face to colour it from.
    const CURSOR_FG: [u8; 3] = [0xf5, 0xf5, 0xf5];

    /// Messages the Brood side pushes to the single GUI thread via the event-loop
    /// proxy. Each carries the window id it targets: winit allows only one event
    /// loop per process (ADR-056), so one thread multiplexes every window.
    enum UserEvent {
        /// Open a new window whose input is delivered to process `subscriber`'s
        /// mailbox; reply with its id + shared size (or a build error).
        Open {
            subscriber: u64,
            title: Option<String>,
            size: Option<(f64, f64)>,
            reply: Sender<Result<OpenReply, String>>,
        },
        /// Replace window `id`'s frame and repaint it.
        Draw { id: u64, ops: Vec<Op> },
        /// Destroy window `id`.
        Close { id: u64 },
        /// Set window `id`'s OS title-bar text at runtime. Behind `gui-title!`.
        Title { id: u64, title: String },
        /// Set window `id`'s taskbar/title-bar icon from raw RGBA pixels (row-major,
        /// `w*h*4` bytes). Behind `gui-icon!`; ignored if the data is the wrong length.
        Icon { id: u64, rgba: Vec<u8>, w: u32, h: u32 },
        /// Raise window `id` to the front and give it OS keyboard focus (un-
        /// minimising it first). Behind `gui-focus` — surfaces an already-open
        /// singleton window instead of opening a duplicate.
        Focus { id: u64 },
        /// Confine the pointer to window `id` (`on`) or release it. Behind
        /// `gui-grab-cursor` — keeps the cursor inside the window for mouse-look so
        /// it can't slip out and click another app.
        Grab { id: u64, on: bool },
        /// Set a cell font — family and/or pixel size; `None` fields are left
        /// unchanged. `id: None` is the **global default**: applied to every open
        /// window and remembered for windows opened later. `id: Some(w)` targets
        /// **just window `w`** and does *not* touch the global default, so two
        /// windows can run different fonts side by side (the no-id call behind
        /// `(gui-font! spec)`, the per-window one behind `(gui-font! id spec)`).
        Font {
            id: Option<u64>,
            family: Option<u32>,
            px: Option<f32>,
        },
        /// Register a font family (interned `name`) from raw TTF bytes per style, so
        /// a face's `:family` can select it. Parsed on the GUI thread and shared by
        /// every renderer. Behind `gui-font-register`.
        RegisterFamily {
            name: u32,
            regular: Vec<u8>,
            bold: Vec<u8>,
            italic: Vec<u8>,
            bold_italic: Vec<u8>,
        },
    }

    /// A freshly opened window's wiring, handed back to the Brood side: its id and
    /// the shared cell size the GUI thread keeps current. Input is *not* polled — the
    /// GUI thread delivers it straight to the subscriber's mailbox (ADR-058).
    struct OpenReply {
        id: u64,
        size: Arc<Mutex<(u16, u16)>>,
        held_key: Arc<Mutex<Option<Key>>>,
    }

    /// What the Brood side keeps per open window (keyed by the id `open` returns) —
    /// just the shared cell size for `gui-size`. Input arrives as mailbox messages,
    /// so there is no receiver to keep here (ADR-058).
    struct WinHandle {
        size: Arc<Mutex<(u16, u16)>>,
        /// The key the window currently sees as physically held (set on press,
        /// cleared on release / focus loss), so `gui-held-key` can be polled as the
        /// source of truth for a held key — immune to a missed key-up (ADR-086).
        held_key: Arc<Mutex<Option<Key>>>,
    }

    /// The one GUI thread's event-loop proxy, started lazily on the first `open`.
    /// Cached as a `Result` so a failed start (e.g. no display) reports the same
    /// error to every caller without retrying. Behind a `Mutex` because several
    /// Brood processes (on different worker threads) may send events concurrently.
    fn gui() -> Result<&'static Mutex<EventLoopProxy<UserEvent>>, String> {
        static G: OnceLock<Result<Mutex<EventLoopProxy<UserEvent>>, String>> = OnceLock::new();
        G.get_or_init(|| start_thread().map(Mutex::new))
            .as_ref()
            .map_err(|e| e.clone())
    }

    /// The Brood-side registry of open windows, keyed by the id `open` returns.
    fn windows() -> &'static Mutex<HashMap<u64, WinHandle>> {
        static W: OnceLock<Mutex<HashMap<u64, WinHandle>>> = OnceLock::new();
        W.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn next_id() -> u64 {
        static N: AtomicU64 = AtomicU64::new(1);
        N.fetch_add(1, Ordering::Relaxed)
    }

    /// Spawn the GUI thread + build the (single) event loop; return a proxy to it.
    fn start_thread() -> Result<EventLoopProxy<UserEvent>, String> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<EventLoopProxy<UserEvent>, String>>();
        std::thread::Builder::new()
            .name("brood-gui".into())
            .spawn(move || run_gui(ready_tx))
            .map_err(|e| e.to_string())?;
        ready_rx
            .recv()
            .map_err(|_| "gui thread exited during init".to_string())?
    }

    /// `(gui-open subscriber)` — open a new window whose key/mouse input is
    /// delivered to process `subscriber`'s mailbox; return the window id. Starts the
    /// GUI thread on the first call. Each call is an independent window.
    pub fn open(
        subscriber: u64,
        title: Option<String>,
        size: Option<(f64, f64)>,
    ) -> Result<u64, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        // Send under the proxy lock, then drop it before awaiting the reply so a
        // slow window build can't block other windows' sends.
        gui()?
            .lock()
            .unwrap()
            .send_event(UserEvent::Open {
                subscriber,
                title,
                size,
                reply: reply_tx,
            })
            .map_err(|_| "gui thread is gone".to_string())?;
        let OpenReply { id, size, held_key } = reply_rx
            .recv()
            .map_err(|_| "gui thread did not reply".to_string())??;
        windows()
            .lock()
            .unwrap()
            .insert(id, WinHandle { size, held_key });
        Ok(id)
    }

    /// `(gui-close id)` — destroy window `id` (idempotent; unknown id is a no-op).
    pub fn close(id: u64) -> Result<(), String> {
        windows().lock().unwrap().remove(&id);
        if let Ok(g) = gui() {
            let _ = g.lock().unwrap().send_event(UserEvent::Close { id });
        }
        Ok(())
    }

    /// `(gui-focus id)` — raise window `id` and request OS keyboard focus (un-
    /// minimising it). The window lives on the GUI thread, so this routes the
    /// request through the event-loop proxy like `close`/`draw`; the actual
    /// `focus_window` runs there. Errors only if the id isn't a live window.
    pub fn focus(id: u64) -> Result<(), String> {
        {
            let w = windows().lock().unwrap();
            if !w.contains_key(&id) {
                return Err("gui window not open".into());
            }
        }
        gui()?
            .lock()
            .unwrap()
            .send_event(UserEvent::Focus { id })
            .map_err(|_| "gui thread is gone".to_string())
    }

    /// `(gui-grab-cursor id on)` — confine the pointer to window `id` (`on` true) or
    /// release it. Dispatched to the GUI thread like `focus`.
    pub fn grab(id: u64, on: bool) -> Result<(), String> {
        {
            let w = windows().lock().unwrap();
            if !w.contains_key(&id) {
                return Err("gui window not open".into());
            }
        }
        gui()?
            .lock()
            .unwrap()
            .send_event(UserEvent::Grab { id, on })
            .map_err(|_| "gui thread is gone".to_string())
    }

    /// `(gui-size id)` — window `id`'s size in character cells.
    pub fn size(id: u64) -> Result<(u16, u16), String> {
        let w = windows().lock().unwrap();
        let h = w.get(&id).ok_or("gui window not open")?;
        let size = *h.size.lock().unwrap();
        Ok(size)
    }

    /// `(gui-held-key id)` — the key window `id` currently sees as physically held,
    /// or `None` when none is. Read from the shared state the event loop keeps current
    /// from press/release transitions (not winit's unreliable `ke.repeat`), so an app
    /// can confirm a key is still down before repeating — the source of truth that
    /// makes a missed key-up unable to cause runaway repeat (ADR-086).
    pub fn held_key(id: u64) -> Result<Option<Key>, String> {
        let w = windows().lock().unwrap();
        let h = w.get(&id).ok_or("gui window not open")?;
        let k = *h.held_key.lock().unwrap();
        Ok(k)
    }

    /// `(gui-draw id ops)` — paint a frame to window `id`.
    pub fn draw(id: u64, ops: Vec<Op>) -> Result<(), String> {
        {
            let w = windows().lock().unwrap();
            if !w.contains_key(&id) {
                return Err("gui window not open".into());
            }
        }
        gui()?
            .lock()
            .unwrap()
            .send_event(UserEvent::Draw { id, ops })
            .map_err(|_| "gui thread is gone".to_string())
    }

    /// `(gui-font! …)` — set a cell font (family and/or pixel size). `id: None`
    /// sets the global default (every open window + ones opened later); `id:
    /// Some(w)` targets just window `w`, leaving the global default untouched.
    /// No-op (silently) if the GUI thread never started.
    pub fn font(id: Option<u64>, family: Option<u32>, px: Option<f32>) -> Result<(), String> {
        if let Ok(g) = gui() {
            let _ = g
                .lock()
                .unwrap()
                .send_event(UserEvent::Font { id, family, px });
        }
        Ok(())
    }

    /// `(gui-title! id text)` — set window `id`'s title-bar text at runtime. Routed
    /// through the event-loop proxy like `font`; a no-op (silently) if the GUI thread
    /// never started or `id` isn't a live window.
    pub fn title(id: u64, title: String) -> Result<(), String> {
        if let Ok(g) = gui() {
            let _ = g.lock().unwrap().send_event(UserEvent::Title { id, title });
        }
        Ok(())
    }

    /// `(gui-icon! id rgba w h)` — set window `id`'s taskbar/title-bar icon from raw
    /// RGBA pixels at runtime. Routed through the proxy like `title`; a silent no-op
    /// if the GUI thread never started or `id` isn't a live window.
    pub fn icon(id: u64, rgba: Vec<u8>, w: u32, h: u32) -> Result<(), String> {
        if let Ok(g) = gui() {
            let _ = g.lock().unwrap().send_event(UserEvent::Icon { id, rgba, w, h });
        }
        Ok(())
    }

    /// `(gui-font-register …)` — register a font family (interned `name`) from raw
    /// TTF bytes per style; the GUI thread parses + shares it so `:family` can pick
    /// it. Starts the GUI thread if needed (so a family can be registered up front).
    pub fn register_family(
        name: u32,
        regular: Vec<u8>,
        bold: Vec<u8>,
        italic: Vec<u8>,
        bold_italic: Vec<u8>,
    ) -> Result<(), String> {
        gui()?
            .lock()
            .unwrap()
            .send_event(UserEvent::RegisterFamily {
                name,
                regular,
                bold,
                italic,
                bold_italic,
            })
            .map_err(|_| "gui thread is gone".to_string())
    }

    /// A key as the Brood value `term-poll`/`gui` deliver: a printable → a 1-char
    /// string, the rest → keywords. Built as a `Message` (no heap) so the GUI thread
    /// can deliver it straight to a mailbox (ADR-058). Mirrors `key_to_value`.
    fn key_message(k: &Key) -> Message {
        match k {
            Key::Char(c) => Message::Str(c.to_string()),
            Key::Ctrl(c) => Message::Keyword(value::intern(&format!("ctrl-{c}"))),
            Key::Alt(c) => Message::Keyword(value::intern(&format!("alt-{c}"))),
            Key::CtrlAlt(c) => Message::Keyword(value::intern(&format!("ctrl-meta-{c}"))),
            Key::Named(s) => Message::Keyword(value::intern(s)),
        }
    }

    /// A key *release*, as the `[:key-up <key>]` vector — the press value (what
    /// `key_message` yields) tagged so the app can tell it from a press. The press
    /// itself stays the bare value, so existing dispatch is untouched; release is
    /// purely additive. Apps pair down→up to track a held key and drive their own
    /// repeat (consumer-paced), rather than relying on the OS auto-repeat we drop.
    fn key_up_message(k: &Key) -> Message {
        Message::Vector(vec![
            Message::Keyword(value::intern("key-up")),
            key_message(k),
        ])
    }

    /// A mouse event as the shared `[:mouse action button row col mods]` vector,
    /// built as a `Message` (no heap). Mirrors `builtins::mouse_to_value`'s shape so
    /// the two frontends stay identical.
    fn mouse_message(m: &Mouse) -> Message {
        let action = match m.action {
            MouseAction::Press => "press",
            MouseAction::Release => "release",
            MouseAction::Drag => "drag",
            MouseAction::Move => "move",
            MouseAction::ScrollUp => "scroll-up",
            MouseAction::ScrollDown => "scroll-down",
        };
        let button = match m.button {
            Some(MouseButton::Left) => Message::Keyword(value::intern("left")),
            Some(MouseButton::Right) => Message::Keyword(value::intern("right")),
            Some(MouseButton::Middle) => Message::Keyword(value::intern("middle")),
            None => Message::Nil,
        };
        // Held modifiers, in a stable order (ctrl, alt, shift); empty `[]` when none.
        let mut mods = Vec::new();
        if m.ctrl {
            mods.push(Message::Keyword(value::intern("ctrl")));
        }
        if m.alt {
            mods.push(Message::Keyword(value::intern("alt")));
        }
        if m.shift {
            mods.push(Message::Keyword(value::intern("shift")));
        }
        Message::Vector(vec![
            Message::Keyword(value::intern("mouse")),
            Message::Keyword(value::intern(action)),
            button,
            Message::Int(m.row as i64),
            Message::Int(m.col as i64),
            Message::Vector(mods),
        ])
    }

    /// A synthetic release of held button `b` at cell `(col, row)` — delivered when the
    /// pointer leaves the window or focus is lost while a button is down, so its real
    /// (off-window) release can't strand the app thinking the button is still pressed.
    fn release_of(b: MouseButton, col: u16, row: u16, mods: &ModifiersState) -> Mouse {
        Mouse {
            action: MouseAction::Release,
            button: Some(b),
            row,
            col,
            ctrl: mods.control_key(),
            alt: mods.alt_key(),
            shift: mods.shift_key(),
        }
    }

    /// A resize event as the `[:resize cols rows]` vector (the new cell grid),
    /// built as a `Message` (no heap) so the GUI thread can deliver it to a
    /// mailbox. Wakes the app loop so it re-renders at the new size instead of
    /// waiting out its poll timeout.
    fn resize_message(cols: u16, rows: u16) -> Message {
        Message::Vector(vec![
            Message::Keyword(value::intern("resize")),
            Message::Int(cols as i64),
            Message::Int(rows as i64),
        ])
    }

    /// One open window's GUI-thread-side state.
    struct Win {
        window: Rc<Window>,
        // Keeps the softbuffer display connection alive for `surface`'s lifetime.
        _context: softbuffer::Context<Rc<Window>>,
        surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
        renderer: Renderer,
        size: Arc<Mutex<(u16, u16)>>,
        /// The process this window's input is delivered to (its mailbox).
        subscriber: u64,
        frame: Vec<Op>,
        mods: ModifiersState,
        cursor: (u16, u16),
        /// The button currently held down (set on press, cleared on release), so a
        /// `CursorMoved` while it's held can be reported as a `:drag` carrying that
        /// button. Deliberately one button at a time — all a drag gesture needs: a
        /// fresh press overwrites it (last-press-wins) and any release clears it, so
        /// chording two buttons isn't tracked. Revisit only if multi-button drag is
        /// ever needed.
        held: Option<MouseButton>,
        /// The key currently held down (set on a fresh press, cleared on its release
        /// or focus loss), shared with the Brood side for `gui-held-key`. Also how the
        /// event loop suppresses auto-repeat: a press for the key already here is a
        /// repeat, dropped — reliable on Wayland where `ke.repeat` isn't (ADR-086).
        held_key: Arc<Mutex<Option<Key>>>,
        /// The PHYSICAL key of the currently-held key (set with `held_key` on a fresh
        /// press). A release is matched to the held key by *physical* key, not logical:
        /// a shifted chord (`(` = Shift+9) whose modifier is released *before* the key
        /// translates its release to a different logical key (`9`), which would never
        /// clear a logical `(` — leaving `held_key` stuck and the repeat running away.
        /// The physical key is the same down and up regardless of modifiers (ADR-086).
        held_physical: Option<PhysicalKey>,
        /// Cursor hot-zones from the last drawn frame (`Op::CursorZone`): cell rect
        /// `(x, y, w, h)` + the shape to show while the pointer is inside. Hit-tested
        /// on `CursorMoved`. (ADR-080.)
        zones: Vec<(u16, u16, u16, u16, super::CursorShape)>,
        /// The shape currently applied to the window, so we only call `set_cursor`
        /// when the hit-test result changes (not on every pointer move).
        shape: Option<super::CursorShape>,
    }

    /// Build a window + softbuffer surface + glyph renderer inside the running event
    /// loop. Errors (window / surface creation) propagate to the `open` caller.
    fn build_window(
        elwt: &ActiveEventLoop,
        subscriber: u64,
        title: Option<String>,
        size: Option<(f64, f64)>,
        families: Families,
        base_px: f32,
        default_family: Option<u32>,
    ) -> Result<Win, String> {
        let (w, h) = size.unwrap_or((840.0, 560.0));
        let window = elwt
            .create_window(
                Window::default_attributes()
                    .with_title(title.unwrap_or_else(|| "Brood".to_string()))
                    .with_inner_size(LogicalSize::new(w, h)),
            )
            .map_err(|e| format!("window: {e}"))?;
        let window = Rc::new(window);
        let context =
            softbuffer::Context::new(window.clone()).map_err(|e| format!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| format!("softbuffer surface: {e}"))?;
        let mut renderer = Renderer::new(window.scale_factor(), families, base_px);
        // honour a global default family set before this window opened
        if let Some(f) = default_family {
            renderer.set_font(Some(f), None);
        }
        Ok(Win {
            window,
            _context: context,
            surface,
            renderer,
            size: Arc::new(Mutex::new((80, 24))),
            subscriber,
            frame: Vec::new(),
            mods: ModifiersState::empty(),
            cursor: (0, 0),
            held: None,
            held_key: Arc::new(Mutex::new(None)),
            held_physical: None,
            zones: Vec::new(),
            shape: None,
        })
    }

    /// The single GUI thread's state — the window registry + the shared font
    /// config — driven by winit 0.30's [`ApplicationHandler`]. Lives entirely on
    /// the GUI thread, so its non-`Send` fields (`Families` is `Rc`-backed) are fine.
    struct GuiApp {
        /// Open windows keyed by winit's `WindowId` (for routing window events).
        wins: HashMap<WindowId, Win>,
        /// Our integer id (what `open` returns) → that `WindowId`.
        ids: HashMap<u64, WindowId>,
        /// Font-family registry shared by every window's renderer (so a
        /// `gui-font-register` reaches them all).
        families: Families,
        /// Global default cell font (family / px) applied to windows opened later.
        default_family: Option<u32>,
        default_px: f32,
        /// winit 0.30 only lets a window be created once the event loop is
        /// **resumed** (an `ActiveEventLoop` whose platform display is live). On
        /// desktop `resumed` fires before the first user event, but rather than
        /// rely on that ordering we gate window creation on this flag and **queue**
        /// any `Open` that arrives early, draining it in `resumed`. This is correct
        /// by construction on every platform. (Surface teardown/recreation across
        /// `suspended`/`resumed` is a *mobile* concern; this is a desktop tool, so
        /// windows simply persist — `resumed` only ever fires once here.)
        resumed: bool,
        /// `Open` requests received before `resumed`, drained when it fires.
        pending_open: Vec<(u64, Option<String>, Option<(f64, f64)>, Sender<Result<OpenReply, String>>)>,
    }

    impl GuiApp {
        /// Create a window for `subscriber` and register it, replying to the
        /// `open` caller with its id + shared size (or the build error). Shared by
        /// the `Open` user event and the `resumed` drain so the path is identical.
        fn open_window(
            &mut self,
            event_loop: &ActiveEventLoop,
            subscriber: u64,
            title: Option<String>,
            size: Option<(f64, f64)>,
            reply: Sender<Result<OpenReply, String>>,
        ) {
            let id = next_id();
            match build_window(
                event_loop,
                subscriber,
                title,
                size,
                self.families.clone(),
                self.default_px,
                self.default_family,
            ) {
                Ok(win) => {
                    update_cells(&win.window, &win.renderer, &win.size);
                    let wid = win.window.id();
                    let _ = reply.send(Ok(OpenReply {
                        id,
                        size: win.size.clone(),
                        held_key: win.held_key.clone(),
                    }));
                    self.ids.insert(id, wid);
                    self.wins.insert(wid, win);
                }
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
            }
        }
    }

    impl ApplicationHandler<UserEvent> for GuiApp {
        // The loop idles (`Wait`) until a proxy or window event arrives. Window
        // creation is now safe (the display is live), so drain any `Open` that
        // arrived before this fired.
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            event_loop.set_control_flow(ControlFlow::Wait);
            self.resumed = true;
            for (subscriber, title, size, reply) in std::mem::take(&mut self.pending_open) {
                self.open_window(event_loop, subscriber, title, size, reply);
            }
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
            match event {
                // Create now if the display is live, else queue until `resumed`.
                UserEvent::Open { subscriber, title, size, reply } => {
                    if self.resumed {
                        self.open_window(event_loop, subscriber, title, size, reply);
                    } else {
                        self.pending_open.push((subscriber, title, size, reply));
                    }
                }
                // Set a live window's OS title-bar text (behind gui-title!).
                UserEvent::Title { id, title } => {
                    if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                        w.window.set_title(&title);
                    }
                }
                // Set a live window's taskbar/title-bar icon (behind gui-icon!).
                UserEvent::Icon { id, rgba, w: iw, h: ih } => {
                    if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                        if let Ok(ic) = Icon::from_rgba(rgba, iw, ih) {
                            win.window.set_window_icon(Some(ic));
                        }
                    }
                }
                UserEvent::Draw { id, ops } => {
                    if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                        // Refresh the cursor hot-zones from this frame, then store it.
                        w.zones = ops
                            .iter()
                            .filter_map(|op| match op {
                                Op::CursorZone { x, y, w: zw, h, shape } => Some((*x, *y, *zw, *h, *shape)),
                                _ => None,
                            })
                            .collect();
                        w.frame = ops;
                        w.window.request_redraw();
                    }
                }
                UserEvent::Close { id } => {
                    if let Some(wid) = self.ids.remove(&id) {
                        self.wins.remove(&wid); // dropping the window closes it
                    }
                }
                // Un-minimise, then raise + focus, so a singleton window that's
                // already open is surfaced rather than re-spawned (behind gui-focus).
                UserEvent::Focus { id } => {
                    if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                        w.window.set_minimized(false);
                        w.window.focus_window();
                    }
                }
                // Confine the pointer to the window (or release it). `Confined` keeps
                // it inside but still moving (so absolute mouse-look maps edge-to-edge);
                // some platforms only offer `Locked`, so fall back to that.
                UserEvent::Grab { id, on } => {
                    if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                        let mode = if on {
                            CursorGrabMode::Confined
                        } else {
                            CursorGrabMode::None
                        };
                        if on && w.window.set_cursor_grab(mode).is_err() {
                            let _ = w.window.set_cursor_grab(CursorGrabMode::Locked);
                        } else if !on {
                            let _ = w.window.set_cursor_grab(CursorGrabMode::None);
                        }
                    }
                }
                // Cell font. `id: Some(w)` retunes just that window, leaving the
                // global default alone (so two windows can differ). `id: None` is
                // the global default: remembered for future windows and applied to
                // every open one. Either way a target window recomputes its grid +
                // republishes its size + redraws (`apply_font`).
                UserEvent::Font { id, family, px } => {
                    match id {
                        Some(target) => {
                            if let Some(w) =
                                self.ids.get(&target).and_then(|wid| self.wins.get_mut(wid))
                            {
                                apply_font(w, family, px);
                            }
                        }
                        None => {
                            if let Some(f) = family {
                                self.default_family = Some(f);
                            }
                            if let Some(p) = px {
                                self.default_px = p.max(1.0);
                            }
                            for w in self.wins.values_mut() {
                                apply_font(w, family, px);
                            }
                        }
                    }
                }
                // Register a font family from raw TTF bytes; parse here and share it
                // with every renderer. A bad font is dropped (the family stays
                // unregistered, so `:family` falls back to the default).
                UserEvent::RegisterFamily {
                    name,
                    regular,
                    bold,
                    italic,
                    bold_italic,
                } => {
                    self.families
                        .borrow_mut()
                        .register(name, regular, bold, italic, bold_italic);
                    // a re-registration replaces a family; clear caches keyed by the
                    // old glyphs and repaint.
                    for w in self.wins.values_mut() {
                        w.renderer.cache.clear();
                        w.window.request_redraw();
                    }
                }
            }
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            let Some(w) = self.wins.get_mut(&window_id) else {
                return;
            };
            match event {
                // The window's close button → a dedicated `:close` message,
                // distinct from the Escape *key* (`:escape`): a frontend signal
                // ("the user wants this window gone"), not a keystroke. The Brood
                // loop tears down (calling gui-close) on its own terms — `ui-run`
                // quits on `:close` automatically; a raw loop matches it like any
                // other input. Keeping it separate means an app that binds Escape
                // to cancel/normal-mode can still be closed by the X button.
                WindowEvent::CloseRequested => {
                    deliver(w.subscriber, Message::Keyword(value::intern("close")));
                }
                WindowEvent::ModifiersChanged(m) => w.mods = m.state(),
                WindowEvent::Resized(_) => {
                    update_cells(&w.window, &w.renderer, &w.size);
                    // Wake the app loop so it re-renders at the new (cols, rows)
                    // now, rather than after its (possibly long) poll timeout.
                    let (cols, rows) = *w.size.lock().unwrap();
                    deliver(w.subscriber, resize_message(cols, rows));
                    w.window.request_redraw();
                }
                // We deliberately ignore 0.30's `inner_size_writer` (which could
                // request a specific new inner size): the cell grid *reflows* to
                // whatever size the window is, so we just re-derive the scale and
                // recompute (cols, rows) from the current `inner_size()`.
                WindowEvent::ScaleFactorChanged { .. } => {
                    w.renderer.set_scale(w.window.scale_factor());
                    update_cells(&w.window, &w.renderer, &w.size);
                    let (cols, rows) = *w.size.lock().unwrap();
                    deliver(w.subscriber, resize_message(cols, rows));
                    w.window.request_redraw();
                }
                WindowEvent::KeyboardInput {
                    event: ke,
                    is_synthetic,
                    ..
                } => match ke.state {
                    // A fresh press goes through; an auto-repeat is dropped. We detect
                    // a repeat by TRANSITION, not winit's `ke.repeat` flag: on
                    // GNOME/Wayland that flag is unreliable — held keys arrive as a
                    // flood of `repeat == false` presses (ADR-086) — so a press for the
                    // key already in `held_key` (no release has cleared it) is the
                    // repeat, and we drop it. A genuine re-press (double-tap) comes only
                    // after a release, which clears `held_key`, so it still registers.
                    // Synthetic presses (winit replaying held keys on focus *gain*) are
                    // dropped too — they'd be phantom keystrokes. Relaying the flood was
                    // the original bug: it outran the mailbox drain, so a backlog kept
                    // "playing" after key-up (the cursor scrolling on past).
                    ElementState::Pressed if !is_synthetic => {
                        if let Some(k) = translate_key(&ke, w.mods) {
                            let mut hk = w.held_key.lock().unwrap();
                            if *hk != Some(k) {
                                *hk = Some(k);
                                drop(hk);
                                w.held_physical = Some(ke.physical_key);
                                deliver(w.subscriber, key_message(&k));
                            }
                        }
                    }
                    ElementState::Pressed => {} // synthetic press (focus-gain replay)
                    // Key release → clear `held_key` (so `gui-held-key` and the repeat
                    // stop) and deliver `[:key-up <held-key>]` as the fast-path stop
                    // signal. Match the release to the held key by its PHYSICAL key, not
                    // its logical one: a shifted chord (`(` = Shift+9) released
                    // modifier-first sends the *base* logical key (`9`) on release, which
                    // would never match a stored `(` — leaving the key stuck and the
                    // repeat running away. The physical key is invariant under modifiers,
                    // so it always matches; we then deliver the *held* logical key's
                    // key-up so the app's stop-by-key-name also fires. Other releases
                    // (a non-held key, a bare modifier) just relay their own key-up.
                    // Synthetic releases count too (winit emits them for a key let go
                    // while unfocused — exactly when we must stop).
                    ElementState::Released => {
                        if w.held_physical == Some(ke.physical_key) {
                            let mut hk = w.held_key.lock().unwrap();
                            let held = *hk;
                            *hk = None;
                            drop(hk);
                            w.held_physical = None;
                            if let Some(k) = held {
                                deliver(w.subscriber, key_up_message(&k));
                            }
                        } else if let Some(k) = translate_key(&ke, w.mods) {
                            deliver(w.subscriber, key_up_message(&k));
                        }
                    }
                },
                // Losing focus (Alt-Tab away mid-hold) is the case a key-up can go
                // missing entirely — the release happens in another window. Deliver
                // `:blur` so the app can drop any held key and stop repeating; a
                // belt-and-suspenders backstop beside the synthetic releases above
                // (ADR-086). Focus *gain* (`true`) needs no signal — the next real
                // press resumes input.
                WindowEvent::Focused(false) => {
                    // Drop the held key: we can't observe its release while unfocused,
                    // so `gui-held-key` must not keep reporting it (the poll-based stop)
                    // and the `:blur` is the event-based stop. Both, belt-and-braces.
                    *w.held_key.lock().unwrap() = None;
                    w.held_physical = None;
                    deliver(w.subscriber, Message::Keyword(value::intern("blur")));
                    // Same for a held mouse button: its release may land off-window /
                    // unfocused and never reach us, so synthesize one now (see CursorLeft).
                    if let Some(b) = w.held.take() {
                        let (col, row) = w.cursor;
                        deliver(w.subscriber, mouse_message(&release_of(b, col, row, &w.mods)));
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    // The pointer left the window. If a button was held, its release happens
                    // outside and we never see it — so the NEXT re-entry's motion would emit
                    // a phantom `:drag` and the app would think the button is still pressed.
                    // Synthesize the release + clear `held` (mirrors the keyboard blur fix).
                    if let Some(b) = w.held.take() {
                        let (col, row) = w.cursor;
                        deliver(w.subscriber, mouse_message(&release_of(b, col, row, &w.mods)));
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    // Track the pointer cell. Bare motion (no button) isn't emitted —
                    // no consumer, and a per-pixel event would flood + force redraws.
                    // But while a button is held, crossing into a NEW cell emits a
                    // `:drag` (cell-granular, so still bounded), which is how a divider
                    // drag is tracked (ADR-077).
                    let cell = px_to_cell(position, &w.renderer);
                    if cell != w.cursor {
                        w.cursor = cell;
                        let (col, row) = w.cursor;
                        // While a button is held this is a `:drag`; otherwise it's a
                        // bare `:move` (button nil). Either way it's cell-granular (only
                        // on crossing into a new cell), so it stays bounded — no per-pixel
                        // flood. Free `:move` is what lets an app do mouse-look / hover
                        // without requiring a click.
                        let action = if w.held.is_some() {
                            MouseAction::Drag
                        } else {
                            MouseAction::Move
                        };
                        deliver(
                            w.subscriber,
                            mouse_message(&Mouse {
                                action,
                                button: w.held,
                                row,
                                col,
                                ctrl: w.mods.control_key(),
                                alt: w.mods.alt_key(),
                                shift: w.mods.shift_key(),
                            }),
                        );
                        // Hover cursor: show a zone's shape (e.g. a resize cursor on
                        // a divider) while the pointer is over it. Locally handled —
                        // no event reaches the app, so no redraw flood. (ADR-080.)
                        let want = shape_at(&w.zones, col, row);
                        if want != w.shape {
                            w.shape = want;
                            w.window
                                .set_cursor(want.map(cursor_icon).unwrap_or(CursorIcon::Default));
                        }
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button,
                    ..
                } => {
                    if let Some(b) = translate_button(button) {
                        w.held = Some(b);
                        let (col, row) = w.cursor;
                        deliver(
                            w.subscriber,
                            mouse_message(&Mouse {
                                action: MouseAction::Press,
                                button: Some(b),
                                row,
                                col,
                                ctrl: w.mods.control_key(),
                                alt: w.mods.alt_key(),
                                shift: w.mods.shift_key(),
                            }),
                        );
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button,
                    ..
                } => {
                    if let Some(b) = translate_button(button) {
                        w.held = None;
                        let (col, row) = w.cursor;
                        deliver(
                            w.subscriber,
                            mouse_message(&Mouse {
                                action: MouseAction::Release,
                                button: Some(b),
                                row,
                                col,
                                ctrl: w.mods.control_key(),
                                alt: w.mods.alt_key(),
                                shift: w.mods.shift_key(),
                            }),
                        );
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    // Positive y scrolls up (away from the user).
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as f64,
                        MouseScrollDelta::PixelDelta(p) => p.y,
                    };
                    if dy != 0.0 {
                        let action = if dy > 0.0 {
                            MouseAction::ScrollUp
                        } else {
                            MouseAction::ScrollDown
                        };
                        let (col, row) = w.cursor;
                        deliver(
                            w.subscriber,
                            mouse_message(&Mouse {
                                action,
                                button: None,
                                row,
                                col,
                                ctrl: w.mods.control_key(),
                                alt: w.mods.alt_key(),
                                shift: w.mods.shift_key(),
                            }),
                        );
                    }
                }
                WindowEvent::RedrawRequested => {
                    paint(&mut w.surface, &w.window, &mut w.renderer, &w.frame)
                }
                _ => {}
            }
        }
    }

    /// The GUI thread body: build the one event loop, hand its proxy back to
    /// `start_thread`, then run winit's loop forever — opening / closing / painting
    /// windows from a registry as `UserEvent`s arrive. It never exits (winit can't
    /// restart an event loop), so it idles harmlessly when no windows are open.
    fn run_gui(ready: Sender<Result<EventLoopProxy<UserEvent>, String>>) {
        // winit normally requires the main thread; on Linux we explicitly allow
        // the dedicated GUI thread to own the loop.
        let mut builder = EventLoop::<UserEvent>::with_user_event();
        builder.with_any_thread(true);
        let event_loop = match builder.build() {
            Ok(el) => el,
            Err(e) => {
                let _ = ready.send(Err(format!("event loop: {e}")));
                return;
            }
        };
        let _ = ready.send(Ok(event_loop.create_proxy()));

        let mut app = GuiApp {
            wins: HashMap::new(),
            ids: HashMap::new(),
            families: default_families(),
            default_family: None,
            default_px: DEFAULT_PX,
            resumed: false,
            pending_open: Vec::new(),
        };
        let _ = event_loop.run_app(&mut app);
    }

    /// Retune one window's cell font (family and/or px), then recompute its grid,
    /// republish its size for `gui-size`, and request a repaint. Shared by the
    /// global-default and per-window arms of `UserEvent::Font`.
    fn apply_font(w: &mut Win, family: Option<u32>, px: Option<f32>) {
        w.renderer.set_font(family, px);
        update_cells(&w.window, &w.renderer, &w.size);
        w.window.request_redraw();
    }

    /// Recompute `(cols, rows)` from the window's physical size and the cell
    /// metrics, and publish it for `gui-size`.
    fn update_cells(window: &winit::window::Window, r: &Renderer, size: &Arc<Mutex<(u16, u16)>>) {
        let sz = window.inner_size();
        let cols = (sz.width as usize / r.cell_w.max(1))
            .max(1)
            .min(u16::MAX as usize) as u16;
        let rows = (sz.height as usize / r.cell_h.max(1))
            .max(1)
            .min(u16::MAX as usize) as u16;
        *size.lock().unwrap() = (cols, rows);
    }

    /// A window pixel position to a (col, row) character cell, clamped to u16.
    fn px_to_cell(pos: PhysicalPosition<f64>, r: &Renderer) -> (u16, u16) {
        let col = (pos.x.max(0.0) as usize / r.cell_w.max(1)).min(u16::MAX as usize) as u16;
        let row = (pos.y.max(0.0) as usize / r.cell_h.max(1)).min(u16::MAX as usize) as u16;
        (col, row)
    }

    fn translate_button(b: WMouseButton) -> Option<MouseButton> {
        match b {
            WMouseButton::Left => Some(MouseButton::Left),
            WMouseButton::Right => Some(MouseButton::Right),
            WMouseButton::Middle => Some(MouseButton::Middle),
            _ => None,
        }
    }

    /// The US-layout shifted form of a base character — `.` → `>`, `1` → `!`, etc.
    /// Used to re-apply Shift to a modifier chord whose base came from
    /// `key_without_modifiers()` (which drops Shift along with Ctrl/Alt). Letters and
    /// anything without a shifted punctuation form pass through unchanged (a letter's
    /// shift is just upper-case, and the chord is lower-cased anyway). Matches the glyphs
    /// the crossterm frontend reports for the same physical chord.
    fn shift_char(c: char) -> char {
        match c {
            '`' => '~',
            '1' => '!',
            '2' => '@',
            '3' => '#',
            '4' => '$',
            '5' => '%',
            '6' => '^',
            '7' => '&',
            '8' => '*',
            '9' => '(',
            '0' => ')',
            '-' => '_',
            '=' => '+',
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            ';' => ':',
            '\'' => '"',
            ',' => '<',
            '.' => '>',
            '/' => '?',
            other => other,
        }
    }

    #[cfg(test)]
    mod shift_char_tests {
        use super::shift_char;
        #[test]
        fn maps_us_shifted_punctuation() {
            assert_eq!(shift_char('.'), '>'); // Emacs M-> (end-of-buffer)
            assert_eq!(shift_char(','), '<'); // M-< (beginning-of-buffer)
            assert_eq!(shift_char('['), '{');
            assert_eq!(shift_char(']'), '}'); // M-{ / M-} paragraph motion
            assert_eq!(shift_char('5'), '%'); // M-% (query-replace)
            assert_eq!(shift_char('6'), '^'); // M-^ (join-line)
            assert_eq!(shift_char('f'), 'f'); // letters pass through (lower-cased later)
        }
    }

    fn translate_key(ke: &KeyEvent, mods: ModifiersState) -> Option<Key> {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        match &ke.logical_key {
            WKey::Named(n) => Some(match n {
                // Shift on a motion key is encoded (`:shift-up`, `:shift-home`, …) so the
                // editor binds shift-select (extend the region) distinctly from a plain
                // arrow; other named keys drop Shift as before.
                NamedKey::ArrowUp if mods.shift_key() => Key::Named("shift-up"),
                NamedKey::ArrowUp => Key::Named("up"),
                NamedKey::ArrowDown if mods.shift_key() => Key::Named("shift-down"),
                NamedKey::ArrowDown => Key::Named("down"),
                NamedKey::ArrowLeft if mods.shift_key() => Key::Named("shift-left"),
                NamedKey::ArrowLeft => Key::Named("left"),
                NamedKey::ArrowRight if mods.shift_key() => Key::Named("shift-right"),
                NamedKey::ArrowRight => Key::Named("right"),
                NamedKey::Enter => Key::Named("enter"),
                NamedKey::Escape => Key::Named("escape"),
                NamedKey::Backspace => Key::Named("backspace"),
                // Shift+Tab is back-tab — match the crossterm frontend's :back-tab.
                NamedKey::Tab if mods.shift_key() => Key::Named("back-tab"),
                NamedKey::Tab => Key::Named("tab"),
                NamedKey::Delete => Key::Named("delete"),
                NamedKey::Home if mods.shift_key() => Key::Named("shift-home"),
                NamedKey::Home => Key::Named("home"),
                NamedKey::End if mods.shift_key() => Key::Named("shift-end"),
                NamedKey::End => Key::Named("end"),
                NamedKey::PageUp => Key::Named("page-up"),
                NamedKey::PageDown => Key::Named("page-down"),
                // Space carries modifiers like a character key would, so Ctrl/Alt
                // survive (Emacs `C-SPC` set-mark → :ctrl- , `C-M-SPC` mark-sexp →
                // :ctrl-meta- , matching crossterm) rather than collapsing to a
                // self-inserted space. The Ctrl+Alt arm must come first.
                NamedKey::Space if mods.control_key() && mods.alt_key() => Key::CtrlAlt(' '),
                NamedKey::Space if mods.control_key() => Key::Ctrl(' '),
                NamedKey::Space if mods.alt_key() => Key::Alt(' '),
                NamedKey::Space => Key::Char(' '),
                _ => return None,
            }),
            WKey::Character(s) => {
                // For a Ctrl/Alt chord, read the key WITHOUT modifiers, so layout
                // composition (on some layouts Alt+`-` composes to en-dash `–`, Alt+
                // letters to accents) doesn't mangle the chord — the keymap binds the
                // BASE character (`-`, `f`). Plain typing keeps the composed/logical
                // char, so AltGr and dead keys still insert their glyph.
                let base = if mods.control_key() || mods.alt_key() {
                    match ke.key_without_modifiers() {
                        WKey::Character(b) => b.chars().next(),
                        _ => s.chars().next(),
                    }
                } else {
                    s.chars().next()
                }?;
                // `key_without_modifiers()` also strips SHIFT, so a shifted-punctuation
                // chord (Emacs `M->` = Alt+Shift+`.`) would lose its shift and arrive as
                // `alt-.` — never matching the `alt->` binding. Re-apply Shift via the
                // US-layout map so the chord reaches the shifted glyph it names (`>`, `<`,
                // `{`, `}`, `%`, `^`, …), matching what the crossterm frontend already
                // delivers (`builtins::key_to_value`). Letters are untouched here (they're
                // lowercased below); plain typing never reaches this (it keeps `s`).
                let c = if (mods.control_key() || mods.alt_key()) && mods.shift_key() {
                    shift_char(base)
                } else {
                    base
                };
                if mods.control_key() && mods.alt_key() {
                    Some(Key::CtrlAlt(c.to_ascii_lowercase()))
                } else if mods.control_key() {
                    Some(Key::Ctrl(c.to_ascii_lowercase()))
                } else if mods.alt_key() {
                    // Meta is case-SENSITIVE in Emacs (`M-O` open-line-above ≠ `M-o`): a
                    // shifted letter stays upper-case so the two are distinct, while an
                    // unshifted chord lower-cases (so Caps Lock / a stray Shift can't change
                    // the binding). Control chords (above) stay case-insensitive, as in Emacs.
                    Some(Key::Alt(if mods.shift_key() {
                        c.to_ascii_uppercase()
                    } else {
                        c.to_ascii_lowercase()
                    }))
                } else {
                    Some(Key::Char(c))
                }
            }
            _ => None,
        }
    }

    // ---- rasterising the cell grid ------------------------------------------

    /// A rasterised grapheme cluster, baked into a small RGBA canvas sized to its
    /// cell span (`width`×`height` px, the cluster's `display-width` cells wide). For
    /// a `color` cluster (emoji) the RGBA is the glyph's own colors; for a monochrome
    /// cluster the RGB is white and only the alpha carries coverage, so the caller
    /// recolors it with the face `fg` at blit time (syntax colors vary per op).
    struct CachedGlyph {
        color: bool,
        width: usize,
        height: usize,
        rgba: Vec<u8>, // width*height*4, straight (non-premultiplied) alpha
    }

    /// The grapheme-cluster part of a glyph-cache key. The vast majority of probes
    /// are single chars (one per cell, one repaint per keystroke), so they key on a
    /// `Char` and allocate nothing; only the rare multi-char cluster (ZWJ emoji,
    /// flag, accented base+mark) takes the `Str` path and allocates a `Box<str>`.
    #[derive(Clone, PartialEq, Eq, Hash)]
    enum ClusterKey {
        Char(char),
        Str(Box<str>),
    }

    impl ClusterKey {
        /// The cheapest key for a cluster: a lone char allocates nothing.
        fn of(g: &str) -> ClusterKey {
            let mut it = g.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => ClusterKey::Char(c),
                _ => ClusterKey::Str(g.into()),
            }
        }
    }

    /// The shared text engine on the single GUI thread: cosmic-text's `FontSystem`
    /// (font database + shaping + fallback) and `SwashCache` (glyph rasterisation,
    /// color and mono), plus the family-keyword → family-name map a `:family` resolves
    /// through. Shared by every window's renderer (so `gui-font-register` reaches them
    /// all), like the old family registry.
    struct FontShared {
        fs: FontSystem,
        swash: SwashCache,
        /// interned family keyword id → fontdb family name (`:mono` → "DejaVu Sans Mono").
        names: HashMap<u32, String>,
    }

    impl FontShared {
        fn new() -> Self {
            let src = |b: &'static [u8]| fontdb::Source::Binary(Rc2::new(b));
            // Load the bundled mono faces + the emoji fallback; no system fonts, so the
            // editor renders identically everywhere (self-contained, ADR-046).
            let fs = FontSystem::new_with_fonts([
                src(FONT_REGULAR),
                src(FONT_BOLD),
                src(FONT_ITALIC),
                src(FONT_BOLD_ITALIC),
                src(FONT_EMOJI),
            ]);
            let mut names = HashMap::new();
            names.insert(value::intern(DEFAULT_FAMILY), MONO_FAMILY.to_string());
            FontShared {
                fs,
                swash: SwashCache::new(),
                names,
            }
        }

        /// The fontdb family name for keyword id `id` (the bundled mono if unknown).
        fn name_of(&self, id: u32) -> String {
            self.names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| MONO_FAMILY.to_string())
        }

        /// Register a family from raw TTF bytes per style (behind `gui-font-register`):
        /// load all four faces into the db, and map `id` to the regular face's family
        /// name so an `Attrs` built for it picks the right faces (weight/style matched).
        fn register(
            &mut self,
            id: u32,
            regular: Vec<u8>,
            bold: Vec<u8>,
            italic: Vec<u8>,
            bold_italic: Vec<u8>,
        ) {
            let ids = self
                .fs
                .db_mut()
                .load_font_source(fontdb::Source::Binary(Rc2::new(regular)));
            let fam = ids
                .first()
                .and_then(|fid| self.fs.db().face(*fid))
                .and_then(|f| f.families.first().map(|(n, _)| n.clone()));
            for b in [bold, italic, bold_italic] {
                self.fs.db_mut().load_font_data(b);
            }
            if let Some(fam) = fam {
                self.names.insert(id, fam);
            }
        }
    }

    // fontdb's `Source::Binary` wants an `Arc<dyn AsRef<[u8]> + Send + Sync>`; alias it
    // so the bundled `&'static [u8]` and the registered `Vec<u8>` both drop straight in.
    use std::sync::Arc as Rc2;

    /// The shared text engine, behind the single GUI thread's `Rc<RefCell<…>>` (it
    /// never leaves that thread). Keeps the `families` field name the windowing code
    /// already threads around.
    type Families = Rc<RefCell<FontShared>>;

    /// Build the shared text engine seeded with the bundled `:mono` family + emoji.
    fn default_families() -> Families {
        Rc::new(RefCell::new(FontShared::new()))
    }

    struct Renderer {
        families: Families,
        default_family: u32,
        base_px: f32,
        scale: f64,
        px: f32,
        cell_w: usize,
        cell_h: usize,
        baseline: i32, // pixels from a cell's top to the text baseline
        // keyed by (cluster, family id, bold, italic, scale): the same cluster at a
        // different family/style/scale rasterises to a different baked canvas.
        cache: HashMap<(ClusterKey, u32, bool, bool, u16), CachedGlyph>,
    }

    impl Renderer {
        fn new(scale: f64, families: Families, base_px: f32) -> Self {
            let mut r = Renderer {
                families,
                default_family: value::intern(DEFAULT_FAMILY),
                base_px,
                scale,
                px: base_px,
                cell_w: 1,
                cell_h: 1,
                baseline: 0,
                cache: HashMap::new(),
            };
            r.recompute();
            r
        }

        /// Recompute the px size + cell metrics by shaping a reference glyph ('M') in
        /// the default family at the current size × HiDPI scale, dropping the cluster
        /// cache (baked at the old px). The grid stays uniform — a per-face
        /// `:family`/`:italic` only changes glyphs within the fixed cell.
        fn recompute(&mut self) {
            self.px = (self.base_px * self.scale as f32).max(1.0);
            self.cache.clear();
            let line_h = (self.px * 1.3).round().max(1.0);
            self.cell_h = line_h as usize;
            // `name_of` returns owned data, so the immutable borrow ends on this
            // line — letting the `borrow_mut` below succeed (don't make it borrow).
            let fam = self.families.borrow().name_of(self.default_family);
            let mut shared = self.families.borrow_mut();
            let shared = &mut *shared;
            let metrics = Metrics::new(self.px, line_h);
            let mut tb = CtBuffer::new(&mut shared.fs, metrics);
            tb.set_size(Some(line_h * 4.0), Some(line_h * 2.0));
            let attrs = Attrs::new().family(Family::Name(fam.as_str()));
            tb.set_text("M", &attrs, Shaping::Advanced, None);
            tb.shape_until_scroll(&mut shared.fs, false);
            let (mut cw, mut base) = (self.px, self.px);
            if let Some(run) = tb.layout_runs().next() {
                base = run.line_y;
                if let Some(gl) = run.glyphs.first() {
                    cw = gl.w;
                }
            }
            self.cell_w = cw.round().max(1.0) as usize;
            self.baseline = base.round() as i32;
        }

        /// Adjust for a new HiDPI scale factor (then recompute metrics).
        fn set_scale(&mut self, scale: f64) {
            self.scale = scale;
            self.recompute();
        }

        /// Set the global default cell font — family and/or pixel size — then
        /// recompute the grid. The whole-window knob behind `gui-font!`.
        fn set_font(&mut self, family: Option<u32>, px: Option<f32>) {
            if let Some(f) = family {
                self.default_family = f;
            }
            if let Some(p) = px {
                self.base_px = p.max(1.0);
            }
            self.recompute();
        }

        /// Shape + rasterise grapheme cluster `g` (cosmic-text + swash) into a small
        /// RGBA canvas sized to its cell span, cached by (cluster, family, style,
        /// scale). A color cluster (emoji) keeps its own colors; a monochrome cluster
        /// stores coverage in the alpha with white RGB, so the caller recolors it.
        fn build_cluster(&self, g: &str, fid: u32, bold: bool, italic: bool, scale: u16) -> CachedGlyph {
            let scale = scale.max(1) as usize;
            let px = (self.px * scale as f32).max(1.0);
            let cells = cluster_cells(g).max(1);
            let cw = (self.cell_w * scale * cells).max(1);
            let ch = (self.cell_h * scale).max(1);
            let line_h = ch as f32;
            // The grid baseline (from the mono 'M', not this cluster's own line) so a
            // fallback glyph aligns with the surrounding text rather than floating to
            // wherever its own font's line metrics put it.
            let baseline = (self.baseline * scale as i32).max(0);
            // `name_of` returns owned data, so the immutable borrow ends on this
            // line — letting the `borrow_mut` below succeed (don't make it borrow).
            let fam = self.families.borrow().name_of(fid);
            let mut shared = self.families.borrow_mut();
            let shared = &mut *shared;
            let attrs = |()| {
                let mut a = Attrs::new().family(Family::Name(fam.as_str()));
                if bold {
                    a = a.weight(Weight::BOLD);
                }
                if italic {
                    a = a.style(Style::Italic);
                }
                a
            };
            // Shape once at the text size to see whether the cluster fell back to a
            // *color* font (an emoji). Text/symbol glyphs are mono.
            let tb = shape_cluster(shared, g, attrs(()), px, cw as f32, line_h);
            let mut rgba = vec![0u8; cw * ch * 4];
            let color = if first_glyph_is_color(shared, &tb) {
                // Emoji: render big enough to fill the cell block and center it — color
                // glyphs have no useful text baseline, so baseline-aligning them looks
                // low and cramped. Size to the smaller block dimension so the (square)
                // glyph fits its `cells`-wide span.
                let epx = ch.min(cw) as f32;
                let tb2 = shape_cluster(shared, g, attrs(()), epx, cw as f32, epx);
                composite_cluster(shared, &tb2, &mut rgba, cw, ch, Placement::Center);
                true
            } else {
                composite_cluster(shared, &tb, &mut rgba, cw, ch, Placement::Baseline(baseline));
                false
            };
            CachedGlyph { color, width: cw, height: ch, rgba }
        }

        /// Blit grapheme cluster `g` into the framebuffer at cell-pixel `(left, top)`,
        /// alpha-compositing over the cell background. A color cluster (emoji) draws in
        /// its own colors; a monochrome one is recolored with the face `fg`. The cluster
        /// occupies `display-width` cells (the caller advances the cursor to match).
        #[allow(clippy::too_many_arguments)]
        fn draw_cluster(
            &mut self,
            buf: &mut [u32],
            fb_w: usize,
            fb_h: usize,
            left: usize,
            top: usize,
            g: &str,
            family: Option<u32>,
            bold: bool,
            italic: bool,
            scale: u16,
            fg: [u8; 3],
        ) {
            if g == " " {
                return;
            }
            let fid = family.unwrap_or(self.default_family);
            // The common single-char cluster keys via `ClusterKey::Char` with no
            // allocation; only a rare multi-char cluster allocates (a `Box<str>`).
            let key = (ClusterKey::of(g), fid, bold, italic, scale.max(1));
            if !self.cache.contains_key(&key) {
                let baked = self.build_cluster(g, fid, bold, italic, scale);
                self.cache.insert(key.clone(), baked);
            }
            let cg = &self.cache[&key];
            for ry in 0..cg.height {
                let py = top + ry;
                if py >= fb_h {
                    break;
                }
                let row = py * fb_w;
                for rx in 0..cg.width {
                    let pxx = left + rx;
                    if pxx >= fb_w {
                        break;
                    }
                    let i = (ry * cg.width + rx) * 4;
                    let a = cg.rgba[i + 3];
                    if a == 0 {
                        continue;
                    }
                    let src = if cg.color {
                        [cg.rgba[i], cg.rgba[i + 1], cg.rgba[i + 2]]
                    } else {
                        fg
                    };
                    buf[row + pxx] = blend(buf[row + pxx], src, a);
                }
            }
        }
    }

    /// Source-over composite a straight-alpha pixel into the RGBA cluster canvas at
    /// `(x, y)` (a no-op off-canvas / at zero alpha). Used to bake a shaped cluster's
    /// glyphs into one canvas before it's cached.
    #[allow(clippy::too_many_arguments)]
    fn canvas_over(rgba: &mut [u8], cw: usize, ch: usize, x: i32, y: i32, sr: u8, sg: u8, sb: u8, sa: u8) {
        if sa == 0 || x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 {
            return;
        }
        let idx = (y as usize * cw + x as usize) * 4;
        let (sa, da) = (sa as u32, rgba[idx + 3] as u32);
        let out_a = sa + da * (255 - sa) / 255;
        if out_a == 0 {
            return;
        }
        let mix = |s: u8, d: u8| (((s as u32 * sa) + (d as u32 * da * (255 - sa) / 255)) / out_a) as u8;
        rgba[idx] = mix(sr, rgba[idx]);
        rgba[idx + 1] = mix(sg, rgba[idx + 1]);
        rgba[idx + 2] = mix(sb, rgba[idx + 2]);
        rgba[idx + 3] = out_a as u8;
    }

    /// Where a cluster's glyphs sit in its baked canvas. `Baseline(y)` puts the text
    /// baseline at row `y` (the shared grid baseline, so a fallback symbol aligns with
    /// the surrounding text). `Center` ignores the baseline and centers the glyph's
    /// bounding box in the canvas — for color emoji, which have no useful text baseline.
    enum Placement {
        Baseline(i32),
        Center,
    }

    /// Shape grapheme cluster `g` into a fresh cosmic-text buffer at `px` / line height
    /// `line_h`, in family/style `attrs`. The layout box is generous so a wide glyph
    /// isn't wrapped or clipped during shaping.
    fn shape_cluster(shared: &mut FontShared, g: &str, attrs: Attrs, px: f32, w: f32, line_h: f32) -> CtBuffer {
        let mut tb = CtBuffer::new(&mut shared.fs, Metrics::new(px.max(1.0), line_h.max(1.0)));
        tb.set_size(Some(w + px), Some(line_h + px));
        tb.set_text(g, &attrs, Shaping::Advanced, None);
        tb.shape_until_scroll(&mut shared.fs, false);
        tb
    }

    /// True if the cluster's first rasterised glyph is a *color* (emoji) bitmap — the
    /// signal to size + center it rather than baseline-align it as text.
    fn first_glyph_is_color(shared: &mut FontShared, tb: &CtBuffer) -> bool {
        for run in tb.layout_runs() {
            for gl in run.glyphs.iter() {
                let phys = gl.physical((0.0, 0.0), 1.0);
                if let Some(img) = shared.swash.get_image(&mut shared.fs, phys.cache_key) {
                    return matches!(img.content, SwashContent::Color);
                }
            }
        }
        false
    }

    /// Composite a shaped cluster's glyphs into the RGBA canvas `rgba` (`cw`×`ch`).
    /// `Baseline` lays each glyph at its pen position on the shared baseline (text);
    /// `Center` puts the glyph's bounding box in the middle of the canvas (emoji).
    /// Color glyphs keep their own RGBA; mask glyphs store coverage as white + alpha
    /// (the caller recolors them with the face fg).
    fn composite_cluster(shared: &mut FontShared, tb: &CtBuffer, rgba: &mut [u8], cw: usize, ch: usize, place: Placement) {
        for run in tb.layout_runs() {
            for gl in run.glyphs.iter() {
                let phys = gl.physical((0.0, 0.0), 1.0);
                let img = match shared.swash.get_image(&mut shared.fs, phys.cache_key) {
                    Some(img) => img,
                    None => continue,
                };
                let (iw, ih) = (img.placement.width as i32, img.placement.height as i32);
                let (ox, oy) = match place {
                    Placement::Baseline(b) => {
                        (phys.x + img.placement.left, b + phys.y - img.placement.top)
                    }
                    Placement::Center => ((cw as i32 - iw) / 2, (ch as i32 - ih) / 2),
                };
                match img.content {
                    SwashContent::Mask => {
                        for ry in 0..ih {
                            for rx in 0..iw {
                                let a = img.data[(ry * iw + rx) as usize];
                                canvas_over(rgba, cw, ch, ox + rx, oy + ry, 255, 255, 255, a);
                            }
                        }
                    }
                    SwashContent::Color => {
                        for ry in 0..ih {
                            for rx in 0..iw {
                                let i = ((ry * iw + rx) * 4) as usize;
                                canvas_over(
                                    rgba, cw, ch, ox + rx, oy + ry,
                                    img.data[i], img.data[i + 1], img.data[i + 2], img.data[i + 3],
                                );
                            }
                        }
                    }
                    SwashContent::SubpixelMask => {}
                }
            }
        }
    }

    fn pack(rgb: [u8; 3]) -> u32 {
        ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32
    }

    /// Alpha-composite `fg` over destination pixel `dst` with coverage `cov` (0..=255).
    fn blend(dst: u32, fg: [u8; 3], cov: u8) -> u32 {
        let a = cov as u32;
        let inv = 255 - a;
        let dr = (dst >> 16) & 0xff;
        let dg = (dst >> 8) & 0xff;
        let db = dst & 0xff;
        let r = (fg[0] as u32 * a + dr * inv) / 255;
        let g = (fg[1] as u32 * a + dg * inv) / 255;
        let b = (fg[2] as u32 * a + db * inv) / 255;
        (r << 16) | (g << 8) | b
    }

    fn fill_cell(
        buf: &mut [u32],
        fb_w: usize,
        fb_h: usize,
        left: usize,
        top: usize,
        w: usize,
        h: usize,
        color: u32,
    ) {
        for y in top..(top + h).min(fb_h) {
            let row = y * fb_w;
            for x in left..(left + w).min(fb_w) {
                buf[row + x] = color;
            }
        }
    }

    /// Draw the text cursor at a cell per its `style`:
    ///   * `Block` — overlay 50% white on the whole cell, so the glyph under it
    ///     stays faintly visible (the terminal-style caret);
    ///   * `Bar` — a thin, solid vertical line on the cell's left edge (a modern
    ///     GUI insertion caret) that doesn't obscure the glyph;
    ///   * `Underline` — a thin solid rule along the cell bottom.
    /// The bar/underline thickness scales with the cell so it stays proportional on
    /// HiDPI (≥2 physical px). They paint solid `CURSOR_FG` rather than a blend, so a
    /// 2px caret reads crisply.
    fn cursor_cell(
        buf: &mut [u32],
        fb_w: usize,
        fb_h: usize,
        left: usize,
        top: usize,
        w: usize,
        h: usize,
        style: super::CursorStyle,
    ) {
        match style {
            super::CursorStyle::Block => {
                for y in top..(top + h).min(fb_h) {
                    let row = y * fb_w;
                    for x in left..(left + w).min(fb_w) {
                        buf[row + x] = blend(buf[row + x], [0xff, 0xff, 0xff], 128);
                    }
                }
            }
            super::CursorStyle::Bar => {
                let thickness = (w / 8).max(2);
                fill_cell(buf, fb_w, fb_h, left, top, thickness, h, pack(CURSOR_FG));
            }
            super::CursorStyle::Underline => {
                let thickness = (h / 10).max(2);
                let uy = top + h.saturating_sub(thickness);
                fill_cell(buf, fb_w, fb_h, left, uy, w, thickness, pack(CURSOR_FG));
            }
        }
    }

    fn paint(
        surface: &mut softbuffer::Surface<Rc<winit::window::Window>, Rc<winit::window::Window>>,
        window: &winit::window::Window,
        r: &mut Renderer,
        frame: &[Op],
    ) {
        let sz = window.inner_size();
        let (w, h) = (sz.width.max(1), sz.height.max(1));
        if surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .is_err()
        {
            return;
        }
        let mut buf = match surface.buffer_mut() {
            Ok(b) => b,
            Err(_) => return,
        };
        let (fb_w, fb_h) = (w as usize, h as usize);
        let bg0 = pack(DEFAULT_BG);
        // Coordinate contract: `r.cell_w`/`cell_h` are PHYSICAL (post-scale) pixels;
        // `Op` row/col are BASE cells (top-left pixel = col*cell_w, row*cell_h); a
        // face `:scale n` multiplies into that same physical grid (n×n base cells).
        //
        // The conventional frame opens with a full `:clear`, which already paints the
        // whole buffer with `bg0` — so skip the unconditional pre-clear in that case
        // to avoid a redundant full-buffer write every frame. We still pre-clear when
        // the frame does NOT start with a full clear, so the background is clean.
        if !matches!(frame.first(), Some(Op::Clear)) {
            for p in buf.iter_mut() {
                *p = bg0;
            }
        }
        let (cw, ch) = (r.cell_w, r.cell_h);
        for op in frame {
            match op {
                Op::Clear => {
                    for p in buf.iter_mut() {
                        *p = bg0;
                    }
                }
                Op::Text { row, col, s, face } => {
                    let (mut fg, mut bg) =
                        (face.fg.unwrap_or(DEFAULT_FG), face.bg.unwrap_or(DEFAULT_BG));
                    if face.reverse {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    // `:scale n` draws each glyph n× larger, occupying an n×n block
                    // of base cells anchored at this op's (row, col); positions stay
                    // in base-cell units, so a scaled cell advances `scale` columns.
                    // We walk *grapheme clusters* (not codepoints), so a ZWJ emoji /
                    // flag / accented char is one unit, advancing its `display-width`
                    // cells — a wide glyph (emoji, CJK) takes two.
                    let scale = face.scale.max(1) as usize;
                    let ch_s = ch * scale;
                    let top = *row as usize * ch;
                    let mut cx = *col as usize;
                    let bg_packed = pack(bg);
                    for g in s.graphemes(true) {
                        let cells = cluster_cells(g);
                        if cells == 0 {
                            // zero-width (a lone combining mark): nothing to advance.
                            continue;
                        }
                        let block_w = cells * cw * scale; // the cluster's pixel span
                        let left = cx * cw;
                        fill_cell(&mut buf, fb_w, fb_h, left, top, block_w, ch_s, bg_packed);
                        r.draw_cluster(
                            &mut buf, fb_w, fb_h, left, top, g, face.family, face.bold,
                            face.italic, face.scale, fg,
                        );
                        if face.underline {
                            // a rule near the block bottom, in the text colour
                            // (scaled with the glyph so it stays proportional).
                            let uy = top + ch_s.saturating_sub(2 * scale);
                            fill_cell(&mut buf, fb_w, fb_h, left, uy, block_w, scale, pack(fg));
                        }
                        cx += cells * scale;
                    }
                }
                Op::Cursor { row, col, style } => {
                    // Always one base cell — the cursor op carries no face, so it
                    // ignores `:scale`; it draws at the single base cell at (row, col),
                    // shaped by `style` (block / bar / underline).
                    cursor_cell(
                        &mut buf,
                        fb_w,
                        fb_h,
                        *col as usize * cw,
                        *row as usize * ch,
                        cw,
                        ch,
                        *style,
                    );
                }
                // Not painted — a cursor zone is hover metadata, hit-tested on
                // pointer-move in the window event handler (ADR-080).
                Op::CursorZone { .. } => {}
                Op::VSpans { row0, col0, cols } => {
                    let top0 = *row0 as usize * ch;
                    for (i, segs) in cols.iter().enumerate() {
                        let left = (*col0 as usize + i) * cw;
                        let mut y = top0;
                        for (h, color) in segs {
                            let span_h = *h as usize * ch;
                            if let Some(rgb) = color {
                                fill_cell(&mut buf, fb_w, fb_h, left, y, cw, span_h, pack(*rgb));
                            }
                            y += span_h;
                        }
                    }
                }
            }
        }
        let _ = buf.present();
    }
}
