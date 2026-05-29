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
//! * `gui-open` asks the thread (via an `EventLoopProxy` user-event) to create a
//!   window and replies with its integer id + a per-window input channel; the
//!   thread starts lazily on the first call.
//! * `gui-draw id` ships the frame as plain `Op`s to that window; the thread stores
//!   it and repaints. `gui-poll id` blocks on that window's input channel.
//! * `gui-size id` reads a shared `(cols, rows)` the thread updates on resize.
//! * `gui-close id` destroys one window. The thread itself never exits (winit can't
//!   restart a loop); it idles when no windows are open.
//!
//! Each window is independent, so `(observe)` can spawn one observer process per
//! window. Only Send data crosses the channels (`Op`/`Input` are plain values); the
//! windows, surfaces, and glyph caches never leave the GUI thread. The whole
//! backend is behind the `gui` cargo feature; without it the primitives return a
//! clear "rebuild with --features gui" error so the symbols still exist uniformly.

/// A resolved text face: colours as RGB (already mapped from `:fg`/`:bg`
/// keywords by the caller, which has heap access), plus the attribute flags.
#[derive(Clone, Copy, Default)]
pub struct Face {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub bold: bool,
    pub reverse: bool,
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

/// One input event the backend hands back from `poll`: a key or a mouse event.
/// Keeps `gui-poll` a single call that yields either (mirroring `term-poll`).
pub enum Input {
    Key(Key),
    Mouse(Mouse),
}

#[cfg(not(feature = "gui"))]
const NOT_COMPILED: &str = "gui backend not compiled in; rebuild with `--features gui`";

#[cfg(not(feature = "gui"))]
mod disabled {
    use super::{Input, Op, NOT_COMPILED};
    pub fn open() -> Result<u64, String> {
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
    pub fn poll(_id: u64, _ms: u64) -> Result<Option<Input>, String> {
        Err(NOT_COMPILED.into())
    }
}

#[cfg(not(feature = "gui"))]
pub use disabled::{close, draw, open, poll, size};

#[cfg(feature = "gui")]
pub use backend::{close, draw, open, poll, size};

#[cfg(feature = "gui")]
mod backend {
    use super::{Input, Key, Mouse, MouseAction, MouseButton, Op};
    use std::collections::HashMap;
    use std::num::NonZeroU32;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

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

    // Bundled monospace font (see assets/README.md) — no system font discovery.
    const FONT_REGULAR: &[u8] = include_bytes!("../assets/DejaVuSansMono.ttf");
    const FONT_BOLD: &[u8] = include_bytes!("../assets/DejaVuSansMono-Bold.ttf");

    // A terminal-ish dark theme for unstyled cells.
    const DEFAULT_BG: [u8; 3] = [0x10, 0x14, 0x18];
    const DEFAULT_FG: [u8; 3] = [0xcd, 0xd6, 0xe0];

    /// Messages the Brood side pushes to the single GUI thread via the event-loop
    /// proxy. Each carries the window id it targets: winit allows only one event
    /// loop per process (ADR-056), so one thread multiplexes every window.
    enum UserEvent {
        /// Open a new window; reply with its wiring (or a build error).
        Open {
            reply: Sender<Result<OpenReply, String>>,
        },
        /// Replace window `id`'s frame and repaint it.
        Draw { id: u64, ops: Vec<Op> },
        /// Destroy window `id`.
        Close { id: u64 },
    }

    /// A freshly opened window's wiring, handed back to the Brood side: its id, the
    /// shared cell size the GUI thread keeps current, and the input channel to poll.
    struct OpenReply {
        id: u64,
        size: Arc<Mutex<(u16, u16)>>,
        input: Receiver<Input>,
    }

    /// What the Brood side keeps per open window (keyed by the id `open` returns).
    /// The input receiver is behind its own `Arc<Mutex>` so `poll` can block on one
    /// window without holding the registry lock — otherwise one window's blocking
    /// poll would stall every other window's draw/poll. Only one process ever polls
    /// a given window, so that per-window mutex is uncontended.
    struct WinHandle {
        size: Arc<Mutex<(u16, u16)>>,
        input: Arc<Mutex<Receiver<Input>>>,
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

    /// `(gui-open)` — open a new window and return its id. Starts the GUI thread on
    /// the first call. Each call is an independent window.
    pub fn open() -> Result<u64, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        // Send under the proxy lock, then drop it before awaiting the reply so a
        // slow window build can't block other windows' sends.
        gui()?
            .lock()
            .unwrap()
            .send_event(UserEvent::Open { reply: reply_tx })
            .map_err(|_| "gui thread is gone".to_string())?;
        let OpenReply { id, size, input } = reply_rx
            .recv()
            .map_err(|_| "gui thread did not reply".to_string())??;
        windows().lock().unwrap().insert(
            id,
            WinHandle {
                size,
                input: Arc::new(Mutex::new(input)),
            },
        );
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

    /// `(gui-poll id ms)` — wait up to `ms` for an input event on window `id`.
    pub fn poll(id: u64, ms: u64) -> Result<Option<Input>, String> {
        // Clone the per-window receiver Arc under the registry lock, then release
        // the registry lock so other windows stay drawable/pollable while we block.
        let rx = {
            let w = windows().lock().unwrap();
            w.get(&id).ok_or("gui window not open")?.input.clone()
        };
        let rx = rx.lock().unwrap();
        match rx.recv_timeout(Duration::from_millis(ms)) {
            Ok(ev) => Ok(Some(ev)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            // The window/thread is gone — surface it as a quit key so the loop ends.
            Err(RecvTimeoutError::Disconnected) => Ok(Some(Input::Key(Key::Named("escape")))),
        }
    }

    /// One open window's GUI-thread-side state.
    struct Win {
        window: Rc<Window>,
        // Keeps the softbuffer display connection alive for `surface`'s lifetime.
        _context: softbuffer::Context<Rc<Window>>,
        surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
        renderer: Renderer,
        size: Arc<Mutex<(u16, u16)>>,
        input: Sender<Input>,
        frame: Vec<Op>,
        mods: ModifiersState,
        cursor: (u16, u16),
    }

    /// Build a window + softbuffer surface + glyph renderer inside the running event
    /// loop. Errors (window / surface creation) propagate to the `open` caller.
    fn build_window(
        elwt: &EventLoopWindowTarget<UserEvent>,
        id: u64,
        input: Sender<Input>,
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
        let renderer = Renderer::new(window.scale_factor());
        Ok(Win {
            window,
            _context: context,
            surface,
            renderer,
            size: Arc::new(Mutex::new((80, 24))),
            input,
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

        let _ = event_loop.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::UserEvent(UserEvent::Open { reply }) => {
                    let id = next_id();
                    let (tx, rx) = mpsc::channel::<Input>();
                    match build_window(elwt, id, tx) {
                        Ok(win) => {
                            update_cells(&win.window, &win.renderer, &win.size);
                            let wid = win.window.id();
                            let _ = reply.send(Ok(OpenReply {
                                id,
                                size: win.size.clone(),
                                input: rx,
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
                Event::WindowEvent { window_id, event } => {
                    let Some(w) = wins.get_mut(&window_id) else {
                        return;
                    };
                    match event {
                        // The window's close button → a quit key, so the Brood loop
                        // tears down (calling gui-close) on its own terms.
                        WindowEvent::CloseRequested => {
                            let _ = w.input.send(Input::Key(Key::Named("escape")));
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
                                    let _ = w.input.send(Input::Key(k));
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
                                let _ = w.input.send(Input::Mouse(Mouse {
                                    action: MouseAction::Press,
                                    button: Some(b),
                                    row,
                                    col,
                                }));
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
                                let _ = w.input.send(Input::Mouse(Mouse {
                                    action,
                                    button: None,
                                    row,
                                    col,
                                }));
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

    struct Renderer {
        regular: fontdue::Font,
        bold: fontdue::Font,
        base_px: f32,
        px: f32,
        cell_w: usize,
        cell_h: usize,
        ascent: i32,
        cache: HashMap<(char, bool), Glyph>,
    }

    impl Renderer {
        fn new(scale: f64) -> Self {
            let opts = fontdue::FontSettings::default();
            let regular =
                fontdue::Font::from_bytes(FONT_REGULAR, opts).expect("bundled regular font");
            let bold = fontdue::Font::from_bytes(FONT_BOLD, opts).expect("bundled bold font");
            let mut r = Renderer {
                regular,
                bold,
                base_px: 15.0,
                px: 15.0,
                cell_w: 1,
                cell_h: 1,
                ascent: 0,
                cache: HashMap::new(),
            };
            r.set_scale(scale);
            r
        }

        /// Recompute the px size + cell metrics for a HiDPI scale factor, and drop
        /// the glyph cache (it's rasterised at the old px).
        fn set_scale(&mut self, scale: f64) {
            self.px = self.base_px * scale as f32;
            self.cache.clear();
            let lm = self
                .regular
                .horizontal_line_metrics(self.px)
                .expect("line metrics");
            self.ascent = lm.ascent.round() as i32;
            self.cell_h = lm.new_line_size.round().max(1.0) as usize;
            // Monospace: every glyph advances the same; 'M' is a safe probe.
            self.cell_w = self
                .regular
                .metrics('M', self.px)
                .advance_width
                .round()
                .max(1.0) as usize;
        }

        /// Blit one glyph's coverage into the framebuffer, alpha-compositing `fg`
        /// over whatever is already there (the cell background).
        fn draw_char(
            &mut self,
            buf: &mut [u32],
            fb_w: usize,
            fb_h: usize,
            left: usize,
            top: usize,
            c: char,
            bold: bool,
            fg: [u8; 3],
        ) {
            if c == ' ' {
                return;
            }
            let px = self.px;
            let ascent = self.ascent;
            let (reg, bold_f) = (&self.regular, &self.bold);
            let g = self.cache.entry((c, bold)).or_insert_with(|| {
                let font = if bold { bold_f } else { reg };
                let (metrics, bitmap) = font.rasterize(c, px);
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
                        r.draw_char(&mut buf, fb_w, fb_h, left, top, c, face.bold, fg);
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
