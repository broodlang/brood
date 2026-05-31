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
}

/// A keystroke, in a backend-neutral shape the Brood side turns into the same
/// values `term-poll` yields: `Char` → a 1-char string, the rest → keywords
/// (`:ctrl-c`, `:alt-f`, `:up`, …).
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
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
    pub fn open(_subscriber: u64) -> Result<u64, String> {
        Err(NOT_COMPILED.into())
    }
    pub fn close(_id: u64) -> Result<(), String> {
        Err(NOT_COMPILED.into())
    }
    pub fn size(_id: u64) -> Result<(u16, u16), String> {
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
pub use disabled::{close, draw, font, open, register_family, size};

#[cfg(feature = "gui")]
pub use backend::{close, draw, font, open, register_family, size};

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
    use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
    use winit::platform::wayland::EventLoopBuilderExtWayland;
    use winit::window::{CursorIcon, Window, WindowId};

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
    // The default font family keyword (`:mono`), and the default cell pixel size.
    const DEFAULT_FAMILY: &str = "mono";
    const DEFAULT_PX: f32 = 15.0;

    // A terminal-ish dark theme for unstyled cells.
    const DEFAULT_BG: [u8; 3] = [0x10, 0x14, 0x18];
    const DEFAULT_FG: [u8; 3] = [0xcd, 0xd6, 0xe0];

    /// Messages the Brood side pushes to the single GUI thread via the event-loop
    /// proxy. Each carries the window id it targets: winit allows only one event
    /// loop per process (ADR-056), so one thread multiplexes every window.
    enum UserEvent {
        /// Open a new window whose input is delivered to process `subscriber`'s
        /// mailbox; reply with its id + shared size (or a build error).
        Open {
            subscriber: u64,
            reply: Sender<Result<OpenReply, String>>,
        },
        /// Replace window `id`'s frame and repaint it.
        Draw { id: u64, ops: Vec<Op> },
        /// Destroy window `id`.
        Close { id: u64 },
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
    }

    /// What the Brood side keeps per open window (keyed by the id `open` returns) —
    /// just the shared cell size for `gui-size`. Input arrives as mailbox messages,
    /// so there is no receiver to keep here (ADR-058).
    struct WinHandle {
        size: Arc<Mutex<(u16, u16)>>,
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
    pub fn open(subscriber: u64) -> Result<u64, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        // Send under the proxy lock, then drop it before awaiting the reply so a
        // slow window build can't block other windows' sends.
        gui()?
            .lock()
            .unwrap()
            .send_event(UserEvent::Open {
                subscriber,
                reply: reply_tx,
            })
            .map_err(|_| "gui thread is gone".to_string())?;
        let OpenReply { id, size } = reply_rx
            .recv()
            .map_err(|_| "gui thread did not reply".to_string())??;
        windows().lock().unwrap().insert(id, WinHandle { size });
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

    /// `(gui-size id)` — window `id`'s size in character cells.
    pub fn size(id: u64) -> Result<(u16, u16), String> {
        let w = windows().lock().unwrap();
        let h = w.get(&id).ok_or("gui window not open")?;
        let size = *h.size.lock().unwrap();
        Ok(size)
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
            Key::Named(s) => Message::Keyword(value::intern(s)),
        }
    }

    /// A mouse event as the shared `[:mouse action button row col mods]` vector,
    /// built as a `Message` (no heap). Mirrors `builtins::mouse_to_value`'s shape so
    /// the two frontends stay identical.
    fn mouse_message(m: &Mouse) -> Message {
        let action = match m.action {
            MouseAction::Press => "press",
            MouseAction::Release => "release",
            MouseAction::Drag => "drag",
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
        /// button. One button at a time is all a drag gesture needs.
        held: Option<MouseButton>,
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
        id: u64,
        subscriber: u64,
        families: Families,
        base_px: f32,
        default_family: Option<u32>,
    ) -> Result<Win, String> {
        let window = elwt
            .create_window(
                Window::default_attributes()
                    .with_title(format!("brood observer #{id}"))
                    .with_inner_size(LogicalSize::new(840.0, 560.0)),
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
        pending_open: Vec<(u64, Sender<Result<OpenReply, String>>)>,
    }

    impl GuiApp {
        /// Create a window for `subscriber` and register it, replying to the
        /// `open` caller with its id + shared size (or the build error). Shared by
        /// the `Open` user event and the `resumed` drain so the path is identical.
        fn open_window(
            &mut self,
            event_loop: &ActiveEventLoop,
            subscriber: u64,
            reply: Sender<Result<OpenReply, String>>,
        ) {
            let id = next_id();
            match build_window(
                event_loop,
                id,
                subscriber,
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
            for (subscriber, reply) in std::mem::take(&mut self.pending_open) {
                self.open_window(event_loop, subscriber, reply);
            }
        }

        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
            match event {
                // Create now if the display is live, else queue until `resumed`.
                UserEvent::Open { subscriber, reply } => {
                    if self.resumed {
                        self.open_window(event_loop, subscriber, reply);
                    } else {
                        self.pending_open.push((subscriber, reply));
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
                    if let Ok(set) = FontSet::from_bytes(&regular, &bold, &italic, &bold_italic) {
                        self.families.borrow_mut().insert(name, Rc::new(set));
                        // a re-registration replaces a family; clear caches keyed by
                        // the old glyphs and repaint.
                        for w in self.wins.values_mut() {
                            w.renderer.cache.clear();
                            w.window.request_redraw();
                        }
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
                    is_synthetic: false,
                    ..
                } => {
                    if ke.state == ElementState::Pressed {
                        if let Some(k) = translate_key(&ke, w.mods) {
                            deliver(w.subscriber, key_message(&k));
                        }
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
                        if let Some(b) = w.held {
                            deliver(
                                w.subscriber,
                                mouse_message(&Mouse {
                                    action: MouseAction::Drag,
                                    button: Some(b),
                                    row,
                                    col,
                                    ctrl: w.mods.control_key(),
                                    alt: w.mods.alt_key(),
                                    shift: w.mods.shift_key(),
                                }),
                            );
                        }
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

    fn translate_key(ke: &KeyEvent, mods: ModifiersState) -> Option<Key> {
        match &ke.logical_key {
            WKey::Named(n) => Some(match n {
                NamedKey::ArrowUp => Key::Named("up"),
                NamedKey::ArrowDown => Key::Named("down"),
                NamedKey::ArrowLeft => Key::Named("left"),
                NamedKey::ArrowRight => Key::Named("right"),
                NamedKey::Enter => Key::Named("enter"),
                NamedKey::Escape => Key::Named("escape"),
                NamedKey::Backspace => Key::Named("backspace"),
                // Shift+Tab is back-tab — match the crossterm frontend's :back-tab.
                NamedKey::Tab if mods.shift_key() => Key::Named("back-tab"),
                NamedKey::Tab => Key::Named("tab"),
                NamedKey::Delete => Key::Named("delete"),
                NamedKey::Home => Key::Named("home"),
                NamedKey::End => Key::Named("end"),
                NamedKey::PageUp => Key::Named("page-up"),
                NamedKey::PageDown => Key::Named("page-down"),
                // Space carries modifiers like a character key would, so Ctrl/Alt
                // survive (Emacs `C-SPC` set-mark → :ctrl- , matching crossterm)
                // rather than collapsing to a self-inserted space.
                NamedKey::Space if mods.control_key() => Key::Ctrl(' '),
                NamedKey::Space if mods.alt_key() => Key::Alt(' '),
                NamedKey::Space => Key::Char(' '),
                _ => return None,
            }),
            WKey::Character(s) => {
                let c = s.chars().next()?;
                if mods.control_key() {
                    Some(Key::Ctrl(c.to_ascii_lowercase()))
                } else if mods.alt_key() {
                    Some(Key::Alt(c.to_ascii_lowercase()))
                } else {
                    Some(Key::Char(c))
                }
            }
            _ => None,
        }
    }

    // ---- rasterising the cell grid ------------------------------------------

    struct Glyph {
        metrics: fontdue::Metrics,
        bitmap: Vec<u8>,
    }

    /// One font family's four styles. A face's `:bold`/`:italic` pick the style;
    /// `:family` picks the set.
    struct FontSet {
        regular: fontdue::Font,
        bold: fontdue::Font,
        italic: fontdue::Font,
        bold_italic: fontdue::Font,
    }

    impl FontSet {
        /// Parse four TTF byte slices into a family (errors propagate to the caller).
        fn from_bytes(
            regular: &[u8],
            bold: &[u8],
            italic: &[u8],
            bold_italic: &[u8],
        ) -> Result<FontSet, String> {
            let opts = fontdue::FontSettings::default();
            let f = |b| fontdue::Font::from_bytes(b, opts).map_err(|e| e.to_string());
            Ok(FontSet {
                regular: f(regular)?,
                bold: f(bold)?,
                italic: f(italic)?,
                bold_italic: f(bold_italic)?,
            })
        }
        fn pick(&self, bold: bool, italic: bool) -> &fontdue::Font {
            match (bold, italic) {
                (false, false) => &self.regular,
                (true, false) => &self.bold,
                (false, true) => &self.italic,
                (true, true) => &self.bold_italic,
            }
        }
    }

    /// The font-family registry, shared by every window's renderer on the single
    /// GUI thread (so `gui-font-register` is visible everywhere at once). Keyed by
    /// the interned family keyword id; `:mono` (bundled) is always present.
    type Families = Rc<RefCell<HashMap<u32, Rc<FontSet>>>>;

    /// Build the families registry seeded with the bundled `:mono` family.
    fn default_families() -> Families {
        let mono = FontSet::from_bytes(FONT_REGULAR, FONT_BOLD, FONT_ITALIC, FONT_BOLD_ITALIC)
            .expect("bundled mono font");
        let mut m = HashMap::new();
        m.insert(value::intern(DEFAULT_FAMILY), Rc::new(mono));
        Rc::new(RefCell::new(m))
    }

    struct Renderer {
        families: Families,
        default_family: u32,
        base_px: f32,
        scale: f64,
        px: f32,
        cell_w: usize,
        cell_h: usize,
        ascent: i32,
        // keyed by (char, family id, bold, italic, scale): the same glyph at a
        // different family/style/scale rasterises differently.
        cache: HashMap<(char, u32, bool, bool, u16), Glyph>,
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
                ascent: 0,
                cache: HashMap::new(),
            };
            r.recompute();
            r
        }

        /// The family for `id` (or the default if unknown / `None`), as a cheap
        /// `Rc` clone so it can be held across a `&mut self` cache borrow.
        fn family_of(&self, id: u32) -> Rc<FontSet> {
            let fams = self.families.borrow();
            fams.get(&id)
                .or_else(|| fams.get(&self.default_family))
                .cloned()
                .expect("default family present")
        }

        /// Recompute the px size + cell metrics from the default family at the
        /// current size × HiDPI scale, dropping the glyph cache (rasterised at the
        /// old px). The grid stays uniform — sized to the default family — so a
        /// per-face `:family`/`:italic` only changes glyphs within the fixed cell.
        fn recompute(&mut self) {
            self.px = self.base_px * self.scale as f32;
            self.cache.clear();
            let set = self.family_of(self.default_family);
            let lm = set
                .regular
                .horizontal_line_metrics(self.px)
                .expect("line metrics");
            self.ascent = lm.ascent.round() as i32;
            self.cell_h = lm.new_line_size.round().max(1.0) as usize;
            // Monospace: every glyph advances the same; 'M' is a safe probe.
            self.cell_w = set
                .regular
                .metrics('M', self.px)
                .advance_width
                .round()
                .max(1.0) as usize;
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

        /// Blit one glyph's coverage into the framebuffer, alpha-compositing `fg`
        /// over whatever is already there (the cell background), in the given
        /// `family`/style. `scale` (≥1) rasterises the glyph at `scale`× the cell
        /// px, anchoring it into the `scale`×`scale` block whose top-left is
        /// `(left, top)` — so a `:scale 3` op draws a glyph filling a 3×3 cell block.
        #[allow(clippy::too_many_arguments)]
        fn draw_char(
            &mut self,
            buf: &mut [u32],
            fb_w: usize,
            fb_h: usize,
            left: usize,
            top: usize,
            c: char,
            family: Option<u32>,
            bold: bool,
            italic: bool,
            scale: u16,
            fg: [u8; 3],
        ) {
            if c == ' ' {
                return;
            }
            let scale = scale.max(1);
            let px = self.px * scale as f32;
            let ascent = self.ascent * scale as i32;
            let fid = family.unwrap_or(self.default_family);
            let set = self.family_of(fid);
            let g = self
                .cache
                .entry((c, fid, bold, italic, scale))
                .or_insert_with(|| {
                    let (metrics, bitmap) = set.pick(bold, italic).rasterize(c, px);
                    Glyph { metrics, bitmap }
                });
            let baseline = top as i32 + ascent;
            let x0 = left as i32 + g.metrics.xmin;
            let y0 = baseline - g.metrics.ymin - g.metrics.height as i32;
            for gy in 0..g.metrics.height {
                let py = y0 + gy as i32;
                if py < 0 || py >= fb_h as i32 {
                    continue;
                }
                let row = py as usize * fb_w;
                for gx in 0..g.metrics.width {
                    let px_x = x0 + gx as i32;
                    if px_x < 0 || px_x >= fb_w as i32 {
                        continue;
                    }
                    let cov = g.bitmap[gy * g.metrics.width + gx];
                    if cov == 0 {
                        continue;
                    }
                    let idx = row + px_x as usize;
                    buf[idx] = blend(buf[idx], fg, cov);
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

    /// A block cursor: overlay 50% white on the cell so any glyph under it stays
    /// faintly visible.
    fn cursor_cell(
        buf: &mut [u32],
        fb_w: usize,
        fb_h: usize,
        left: usize,
        top: usize,
        w: usize,
        h: usize,
    ) {
        for y in top..(top + h).min(fb_h) {
            let row = y * fb_w;
            for x in left..(left + w).min(fb_w) {
                buf[row + x] = blend(buf[row + x], [0xff, 0xff, 0xff], 128);
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
        for p in buf.iter_mut() {
            *p = bg0;
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
                    // in base-cell units, so a scaled char advances `scale` columns.
                    let scale = face.scale.max(1) as usize;
                    let (cw_s, ch_s) = (cw * scale, ch * scale);
                    let top = *row as usize * ch;
                    let mut cx = *col as usize;
                    let bg_packed = pack(bg);
                    for c in s.chars() {
                        let left = cx * cw;
                        fill_cell(&mut buf, fb_w, fb_h, left, top, cw_s, ch_s, bg_packed);
                        r.draw_char(
                            &mut buf, fb_w, fb_h, left, top, c, face.family, face.bold,
                            face.italic, face.scale, fg,
                        );
                        if face.underline {
                            // a rule near the block bottom, in the text colour
                            // (scaled with the glyph so it stays proportional).
                            let uy = top + ch_s.saturating_sub(2 * scale);
                            fill_cell(&mut buf, fb_w, fb_h, left, uy, cw_s, scale, pack(fg));
                        }
                        cx += scale;
                    }
                }
                Op::Cursor { row, col } => {
                    cursor_cell(
                        &mut buf,
                        fb_w,
                        fb_h,
                        *col as usize * cw,
                        *row as usize * ch,
                        cw,
                        ch,
                    );
                }
                // Not painted — a cursor zone is hover metadata, hit-tested on
                // pointer-move in the window event handler (ADR-080).
                Op::CursorZone { .. } => {}
            }
        }
        let _ = buf.present();
    }
}
