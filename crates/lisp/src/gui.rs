//! The windowed (GUI) frontend for the M3 display/input seam — ADR-046's second
//! frontend, alongside the terminal (crossterm) one in `builtins.rs`.
//!
//! The display *protocol* is unchanged: a frame is a vector of render ops
//! (`[:clear]`, `[:text row col s face]`, `[:cursor row col]`) — plain Brood data.
//! This module paints that frame to a native window instead of a terminal, and
//! reads keystrokes back in the same encoding (`"a"`, `:up`, `:ctrl-c`, …). So
//! `std/observe.blsp`, the REPL editor, and the future editor drive it through
//! the identical `gui-*` ⇆ `term-*` surface and never know which backend is live.
//!
//! ## Threading
//!
//! A GUI toolkit insists on owning a thread + event loop, which collides with
//! Brood's model: the observer runs in the *root* process and *blocks* on
//! `(gui-poll ms)` for a key. We reconcile the two by running winit on a
//! dedicated **GUI thread** and bridging with channels — the same shape the
//! synchronous `term-*` seam has, with the toolkit's loop-ownership contained
//! entirely behind these primitives:
//!
//! * `gui-draw` extracts the frame into plain `Op`s (it has heap access) and ships
//!   them to the GUI thread via an `EventLoopProxy` user-event; the GUI thread
//!   stores the frame and repaints.
//! * `gui-poll` blocks on a key channel the GUI thread feeds from winit key events.
//! * `gui-size` reads a shared `(cols, rows)` the GUI thread updates on resize.
//! * `gui-enter`/`gui-leave` start/stop the thread.
//!
//! Only Send data crosses the channels (`Op`/`Key` are plain values); the window,
//! surface, and glyph cache never leave the GUI thread. The whole backend is
//! behind the `gui` cargo feature; without it the primitives return a clear
//! "rebuild with --features gui" error so the symbols still exist uniformly.

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
    Text { row: u16, col: u16, s: String, face: Face },
    Cursor { row: u16, col: u16 },
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

const NOT_COMPILED: &str = "gui backend not compiled in; rebuild with `--features gui`";

#[cfg(not(feature = "gui"))]
mod disabled {
    use super::{Key, Op, NOT_COMPILED};
    pub fn enter() -> Result<(), String> { Err(NOT_COMPILED.into()) }
    pub fn leave() -> Result<(), String> { Err(NOT_COMPILED.into()) }
    pub fn size() -> Result<(u16, u16), String> { Err(NOT_COMPILED.into()) }
    pub fn draw(_ops: Vec<Op>) -> Result<(), String> { Err(NOT_COMPILED.into()) }
    pub fn poll(_ms: u64) -> Result<Option<Key>, String> { Err(NOT_COMPILED.into()) }
}

#[cfg(not(feature = "gui"))]
pub use disabled::{draw, enter, leave, poll, size};

#[cfg(feature = "gui")]
pub use backend::{draw, enter, leave, poll, size};

#[cfg(feature = "gui")]
mod backend {
    use super::{Face, Key, Op};
    use std::collections::HashMap;
    use std::num::NonZeroU32;
    use std::rc::Rc;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, Event, KeyEvent, WindowEvent};
    use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
    use winit::platform::wayland::EventLoopBuilderExtWayland;
    use winit::window::WindowBuilder;

    // Bundled monospace font (see assets/README.md) — no system font discovery.
    const FONT_REGULAR: &[u8] = include_bytes!("../assets/DejaVuSansMono.ttf");
    const FONT_BOLD: &[u8] = include_bytes!("../assets/DejaVuSansMono-Bold.ttf");

    // A terminal-ish dark theme for unstyled cells.
    const DEFAULT_BG: [u8; 3] = [0x10, 0x14, 0x18];
    const DEFAULT_FG: [u8; 3] = [0xcd, 0xd6, 0xe0];

    /// Messages the Brood side pushes to the GUI thread via the event-loop proxy.
    enum UserEvent {
        Draw(Vec<Op>),
        Leave,
    }

    /// The live-window handle the Brood side holds (all fields are Send): the
    /// proxy to push frames, the key channel to poll, the shared cell size, and
    /// the GUI thread join handle.
    struct Handle {
        proxy: EventLoopProxy<UserEvent>,
        keys: Receiver<Key>,
        size: Arc<Mutex<(u16, u16)>>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    fn slot() -> &'static Mutex<Option<Handle>> {
        static HANDLE: OnceLock<Mutex<Option<Handle>>> = OnceLock::new();
        HANDLE.get_or_init(|| Mutex::new(None))
    }

    pub fn enter() -> Result<(), String> {
        let mut g = slot().lock().unwrap();
        if g.is_some() {
            return Ok(()); // already up — idempotent like term-enter
        }
        let size = Arc::new(Mutex::new((80u16, 24u16)));
        let (key_tx, key_rx) = mpsc::channel::<Key>();
        // The GUI thread reports back the proxy once the loop/window is built (or
        // an error if creating either failed), so `enter` blocks until the window
        // is actually on screen and its cell size is known.
        let (ready_tx, ready_rx) = mpsc::channel::<Result<EventLoopProxy<UserEvent>, String>>();
        let size_for_thread = size.clone();
        let join = std::thread::Builder::new()
            .name("brood-gui".into())
            .spawn(move || run_gui(size_for_thread, key_tx, ready_tx))
            .map_err(|e| e.to_string())?;
        let proxy = ready_rx
            .recv()
            .map_err(|_| "gui thread exited during init".to_string())??;
        *g = Some(Handle { proxy, keys: key_rx, size, join: Some(join) });
        Ok(())
    }

    pub fn leave() -> Result<(), String> {
        let mut g = slot().lock().unwrap();
        if let Some(mut h) = g.take() {
            let _ = h.proxy.send_event(UserEvent::Leave);
            if let Some(j) = h.join.take() {
                let _ = j.join();
            }
        }
        Ok(())
    }

    pub fn size() -> Result<(u16, u16), String> {
        let g = slot().lock().unwrap();
        let h = g.as_ref().ok_or("gui not started")?;
        Ok(*h.size.lock().unwrap())
    }

    pub fn draw(ops: Vec<Op>) -> Result<(), String> {
        let g = slot().lock().unwrap();
        let h = g.as_ref().ok_or("gui not started")?;
        h.proxy
            .send_event(UserEvent::Draw(ops))
            .map_err(|_| "gui window closed".to_string())
    }

    pub fn poll(ms: u64) -> Result<Option<Key>, String> {
        // The single observer thread calls size/draw/poll in sequence, so holding
        // the slot lock across the blocking recv is fine (no other caller contends;
        // the GUI thread never touches this lock).
        let g = slot().lock().unwrap();
        let h = g.as_ref().ok_or("gui not started")?;
        match h.keys.recv_timeout(Duration::from_millis(ms)) {
            Ok(k) => Ok(Some(k)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            // The window/thread is gone — surface it as a quit key so the loop ends.
            Err(RecvTimeoutError::Disconnected) => Ok(Some(Key::Named("escape"))),
        }
    }

    /// The GUI thread body: build the event loop + window + surface + glyph cache,
    /// report the proxy to `enter`, then run winit's loop until `Leave`.
    fn run_gui(
        size: Arc<Mutex<(u16, u16)>>,
        key_tx: Sender<Key>,
        ready: Sender<Result<EventLoopProxy<UserEvent>, String>>,
    ) {
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
        let proxy = event_loop.create_proxy();
        let window = match WindowBuilder::new()
            .with_title("brood")
            .with_inner_size(LogicalSize::new(840.0, 560.0))
            .build(&event_loop)
        {
            Ok(w) => Rc::new(w),
            Err(e) => {
                let _ = ready.send(Err(format!("window: {e}")));
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                let _ = ready.send(Err(format!("softbuffer context: {e}")));
                return;
            }
        };
        let mut surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                let _ = ready.send(Err(format!("softbuffer surface: {e}")));
                return;
            }
        };

        let mut renderer = Renderer::new(window.scale_factor());
        update_cells(&window, &renderer, &size);
        let _ = ready.send(Ok(proxy));

        let mut frame: Vec<Op> = Vec::new();
        let mut mods = ModifiersState::empty();

        let _ = event_loop.run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            match event {
                Event::UserEvent(UserEvent::Leave) => elwt.exit(),
                Event::UserEvent(UserEvent::Draw(ops)) => {
                    frame = ops;
                    window.request_redraw();
                }
                Event::WindowEvent { event, .. } => match event {
                    // Treat the window's close button as a quit key, so the Brood
                    // loop tears down (calling gui-leave) on its own terms.
                    WindowEvent::CloseRequested => {
                        let _ = key_tx.send(Key::Named("escape"));
                    }
                    WindowEvent::ModifiersChanged(m) => mods = m.state(),
                    WindowEvent::Resized(_) => {
                        update_cells(&window, &renderer, &size);
                        window.request_redraw();
                    }
                    WindowEvent::ScaleFactorChanged { .. } => {
                        renderer.set_scale(window.scale_factor());
                        update_cells(&window, &renderer, &size);
                        window.request_redraw();
                    }
                    WindowEvent::KeyboardInput { event: ke, is_synthetic: false, .. } => {
                        if ke.state == ElementState::Pressed {
                            if let Some(k) = translate_key(&ke, mods) {
                                let _ = key_tx.send(k);
                            }
                        }
                    }
                    WindowEvent::RedrawRequested => paint(&mut surface, &window, &mut renderer, &frame),
                    _ => {}
                },
                _ => {}
            }
        });
    }

    /// Recompute `(cols, rows)` from the window's physical size and the cell
    /// metrics, and publish it for `gui-size`.
    fn update_cells(window: &winit::window::Window, r: &Renderer, size: &Arc<Mutex<(u16, u16)>>) {
        let sz = window.inner_size();
        let cols = (sz.width as usize / r.cell_w.max(1)).max(1).min(u16::MAX as usize) as u16;
        let rows = (sz.height as usize / r.cell_h.max(1)).max(1).min(u16::MAX as usize) as u16;
        *size.lock().unwrap() = (cols, rows);
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
            let regular = fontdue::Font::from_bytes(FONT_REGULAR, opts).expect("bundled regular font");
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
            let lm = self.regular.horizontal_line_metrics(self.px).expect("line metrics");
            self.ascent = lm.ascent.round() as i32;
            self.cell_h = lm.new_line_size.round().max(1.0) as usize;
            // Monospace: every glyph advances the same; 'M' is a safe probe.
            self.cell_w = self.regular.metrics('M', self.px).advance_width.round().max(1.0) as usize;
        }

        /// Blit one glyph's coverage into the framebuffer, alpha-compositing `fg`
        /// over whatever is already there (the cell background).
        fn draw_char(&mut self, buf: &mut [u32], fb_w: usize, fb_h: usize, left: usize, top: usize, c: char, bold: bool, fg: [u8; 3]) {
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

    fn fill_cell(buf: &mut [u32], fb_w: usize, fb_h: usize, left: usize, top: usize, w: usize, h: usize, color: u32) {
        for y in top..(top + h).min(fb_h) {
            let row = y * fb_w;
            for x in left..(left + w).min(fb_w) {
                buf[row + x] = color;
            }
        }
    }

    /// A block cursor: overlay 50% white on the cell so any glyph under it stays
    /// faintly visible.
    fn cursor_cell(buf: &mut [u32], fb_w: usize, fb_h: usize, left: usize, top: usize, w: usize, h: usize) {
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
                    let (mut fg, mut bg) = (face.fg.unwrap_or(DEFAULT_FG), face.bg.unwrap_or(DEFAULT_BG));
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
                    cursor_cell(&mut buf, fb_w, fb_h, *col as usize * cw, *row as usize * ch, cw, ch);
                }
            }
        }
        let _ = buf.present();
    }
}
