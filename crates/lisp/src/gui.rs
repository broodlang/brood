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
/// keywords by the caller, which has heap access), the attribute flags, and the
/// optional font family (an interned `:family` keyword id, resolved to a loaded
/// font set by the renderer; `None` = the default family).
#[derive(Clone, Copy, Default)]
pub struct Face {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub family: Option<u32>,
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

/// What the mouse did. `Scroll*` carry no button. Deliberately minimal — exactly
/// what a frontend consumer needs today (a click and the wheel); release / drag /
/// bare motion are deferred (ADR-056/011) since nothing consumes them and emitting
/// them per-pixel would flood the input channel. The crossterm frontend maps to
/// this same set, so one `[:mouse …]` shape covers both.
#[derive(Clone, Copy)]
pub enum MouseAction {
    Press,
    ScrollUp,
    ScrollDown,
}

/// A mouse event at a character-cell position; the Brood side turns it into a
/// `[:mouse action button row col]` vector (`button` is nil for scroll).
#[derive(Clone, Copy)]
pub struct Mouse {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub row: u16,
    pub col: u16,
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
    pub fn font(_family: Option<u32>, _px: Option<f32>) -> Result<(), String> {
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

    use winit::dpi::{LogicalSize, PhysicalPosition};
    use winit::event::{
        ElementState, Event, KeyEvent, MouseButton as WMouseButton, MouseScrollDelta, WindowEvent,
    };
    use winit::event_loop::{
        ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget,
    };
    use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
    use winit::platform::wayland::EventLoopBuilderExtWayland;
    use winit::window::{Window, WindowBuilder, WindowId};

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
        /// Set the global default cell font — family and/or pixel size — applied to
        /// every open window and remembered for windows opened later. The
        /// whole-window knob behind `gui-font!`; `None` fields are left unchanged.
        Font {
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

    /// `(gui-font! …)` — set the global default cell font (family and/or pixel
    /// size), applied to every open window and remembered for ones opened later.
    /// No-op (silently) if the GUI thread never started.
    pub fn font(family: Option<u32>, px: Option<f32>) -> Result<(), String> {
        if let Ok(g) = gui() {
            let _ = g
                .lock()
                .unwrap()
                .send_event(UserEvent::Font { family, px });
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

    /// A mouse event as the shared `[:mouse action button row col]` vector, built as
    /// a `Message` (no heap). Mirrors `builtins::mouse_to_value`'s shape so the two
    /// frontends stay identical.
    fn mouse_message(m: &Mouse) -> Message {
        let action = match m.action {
            MouseAction::Press => "press",
            MouseAction::ScrollUp => "scroll-up",
            MouseAction::ScrollDown => "scroll-down",
        };
        let button = match m.button {
            Some(MouseButton::Left) => Message::Keyword(value::intern("left")),
            Some(MouseButton::Right) => Message::Keyword(value::intern("right")),
            Some(MouseButton::Middle) => Message::Keyword(value::intern("middle")),
            None => Message::Nil,
        };
        Message::Vector(vec![
            Message::Keyword(value::intern("mouse")),
            Message::Keyword(value::intern(action)),
            button,
            Message::Int(m.row as i64),
            Message::Int(m.col as i64),
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
    }

    /// Build a window + softbuffer surface + glyph renderer inside the running event
    /// loop. Errors (window / surface creation) propagate to the `open` caller.
    fn build_window(
        elwt: &EventLoopWindowTarget<UserEvent>,
        id: u64,
        subscriber: u64,
        families: Families,
        base_px: f32,
        default_family: Option<u32>,
    ) -> Result<Win, String> {
        let window = WindowBuilder::new()
            .with_title(format!("brood observer #{id}"))
            .with_inner_size(LogicalSize::new(840.0, 560.0))
            .build(elwt)
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
        })
    }

    /// The GUI thread body: build the one event loop, hand its proxy back to
    /// `start_thread`, then run winit's loop forever — opening / closing / painting
    /// windows from a registry as `UserEvent`s arrive. It never exits (winit can't
    /// restart an event loop), so it idles harmlessly when no windows are open.
    fn run_gui(ready: Sender<Result<EventLoopProxy<UserEvent>, String>>) {
        let mut builder = EventLoopBuilder::<UserEvent>::with_user_event();
        // winit normally requires the main thread; on Linux we explicitly allow
        // the dedicated GUI thread to own the loop.
        builder.with_any_thread(true);
        let event_loop = match builder.build() {
            Ok(el) => el,
            Err(e) => {
                let _ = ready.send(Err(format!("event loop: {e}")));
                return;
            }
        };
        let _ = ready.send(Ok(event_loop.create_proxy()));

        // Open windows keyed by winit's WindowId (for routing window events), plus a
        // map from our integer id (what `open` returns) to that WindowId.
        let mut wins: HashMap<WindowId, Win> = HashMap::new();
        let mut ids: HashMap<u64, WindowId> = HashMap::new();
        // The font-family registry, shared by every window's renderer (so a
        // `gui-font-register` reaches them all), plus the global default cell font
        // (family / px) applied to windows opened later.
        let families: Families = default_families();
        let mut default_family: Option<u32> = None;
        let mut default_px: f32 = DEFAULT_PX;

        let _ = event_loop.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::UserEvent(UserEvent::Open { subscriber, reply }) => {
                    let id = next_id();
                    match build_window(elwt, id, subscriber, families.clone(), default_px, default_family) {
                        Ok(win) => {
                            update_cells(&win.window, &win.renderer, &win.size);
                            let wid = win.window.id();
                            let _ = reply.send(Ok(OpenReply {
                                id,
                                size: win.size.clone(),
                            }));
                            ids.insert(id, wid);
                            wins.insert(wid, win);
                        }
                        Err(e) => {
                            let _ = reply.send(Err(e));
                        }
                    }
                }
                Event::UserEvent(UserEvent::Draw { id, ops }) => {
                    if let Some(w) = ids.get(&id).and_then(|wid| wins.get_mut(wid)) {
                        w.frame = ops;
                        w.window.request_redraw();
                    }
                }
                Event::UserEvent(UserEvent::Close { id }) => {
                    if let Some(wid) = ids.remove(&id) {
                        wins.remove(&wid); // dropping the window closes it
                    }
                }
                // Global default cell font: remember it for future windows and apply
                // it to every open one (recompute the grid + republish size + redraw).
                Event::UserEvent(UserEvent::Font { family, px }) => {
                    if let Some(f) = family {
                        default_family = Some(f);
                    }
                    if let Some(p) = px {
                        default_px = p.max(1.0);
                    }
                    for w in wins.values_mut() {
                        w.renderer.set_font(family, px);
                        update_cells(&w.window, &w.renderer, &w.size);
                        w.window.request_redraw();
                    }
                }
                // Register a font family from raw TTF bytes; parse here and share it
                // with every renderer. A bad font is dropped (the family stays
                // unregistered, so `:family` falls back to the default).
                Event::UserEvent(UserEvent::RegisterFamily {
                    name,
                    regular,
                    bold,
                    italic,
                    bold_italic,
                }) => {
                    if let Ok(set) = FontSet::from_bytes(&regular, &bold, &italic, &bold_italic) {
                        families.borrow_mut().insert(name, Rc::new(set));
                        // a re-registration replaces a family; clear caches keyed by
                        // the old glyphs and repaint.
                        for w in wins.values_mut() {
                            w.renderer.cache.clear();
                            w.window.request_redraw();
                        }
                    }
                }
                Event::WindowEvent { window_id, event } => {
                    let Some(w) = wins.get_mut(&window_id) else {
                        return;
                    };
                    match event {
                        // The window's close button → a quit key, so the Brood loop
                        // tears down (calling gui-close) on its own terms.
                        WindowEvent::CloseRequested => {
                            deliver(w.subscriber, key_message(&Key::Named("escape")));
                        }
                        WindowEvent::ModifiersChanged(m) => w.mods = m.state(),
                        WindowEvent::Resized(_) => {
                            update_cells(&w.window, &w.renderer, &w.size);
                            w.window.request_redraw();
                        }
                        WindowEvent::ScaleFactorChanged { .. } => {
                            w.renderer.set_scale(w.window.scale_factor());
                            update_cells(&w.window, &w.renderer, &w.size);
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
                            // Track the pointer cell so a later press/scroll reports
                            // it; bare motion isn't emitted (no consumer, and a
                            // per-pixel event would flood + force redraws).
                            w.cursor = px_to_cell(position, &w.renderer);
                        }
                        // Press only — release isn't in the vocabulary (MouseAction).
                        WindowEvent::MouseInput {
                            state: ElementState::Pressed,
                            button,
                            ..
                        } => {
                            if let Some(b) = translate_button(button) {
                                let (col, row) = w.cursor;
                                deliver(
                                    w.subscriber,
                                    mouse_message(&Mouse {
                                        action: MouseAction::Press,
                                        button: Some(b),
                                        row,
                                        col,
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
                _ => {}
            }
        });
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
        // keyed by (char, family id, bold, italic): the same glyph at a different
        // family/style rasterises differently.
        cache: HashMap<(char, u32, bool, bool), Glyph>,
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
        /// `family`/style.
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
            fg: [u8; 3],
        ) {
            if c == ' ' {
                return;
            }
            let px = self.px;
            let ascent = self.ascent;
            let fid = family.unwrap_or(self.default_family);
            let set = self.family_of(fid);
            let g = self.cache.entry((c, fid, bold, italic)).or_insert_with(|| {
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
                    let top = *row as usize * ch;
                    let mut cx = *col as usize;
                    let bg_packed = pack(bg);
                    for c in s.chars() {
                        let left = cx * cw;
                        fill_cell(&mut buf, fb_w, fb_h, left, top, cw, ch, bg_packed);
                        r.draw_char(
                            &mut buf, fb_w, fb_h, left, top, c, face.family, face.bold,
                            face.italic, fg,
                        );
                        if face.underline {
                            // a 1px rule near the cell bottom, in the text colour
                            let uy = top + ch.saturating_sub(2);
                            fill_cell(&mut buf, fb_w, fb_h, left, uy, cw, 1, pack(fg));
                        }
                        cx += 1;
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
            }
        }
        let _ = buf.present();
    }
}
