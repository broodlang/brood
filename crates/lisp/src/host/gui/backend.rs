//! The windowed backend (feature `gui`): the winit event loop on its own thread (or the
//! process main thread where the platform demands it — ADR-324), the window registry
//! the `%gui-*` primitives address by id, and the messages a window delivers to its
//! subscribing process. Input translation, text rendering and frame painting are the
//! child modules.

mod input;
mod paint;
mod render;

use input::*;
use paint::*;
pub(crate) use render::Renderer;
pub use render::TextAa;
use render::*;

use super::{Key, Mouse, MouseAction, MouseButton, Op, WindowSpec};

use crate::core::value;

use crate::process::{deliver, Message};

use std::cell::RefCell;

use std::collections::HashMap;

use std::num::NonZeroU32;

use std::rc::Rc;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use std::sync::mpsc::{self, Sender};

use std::sync::{Arc, Mutex, OnceLock};

use std::thread::JoinHandle;

use std::time::Duration;

use web_time::Instant;

use winit::application::ApplicationHandler;

use winit::dpi::{LogicalSize, PhysicalPosition};

use winit::event::{
    ElementState, KeyEvent, MouseButton as WMouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};

use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

use winit::keyboard::{Key as WKey, ModifiersState, NamedKey, PhysicalKey};

// Wayland/X11 only: this is the extension trait providing `with_any_thread`, which
// has NO macOS (or Windows) equivalent — see `DEDICATED_THREAD_OK` below.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
use winit::platform::wayland::EventLoopBuilderExtWayland;

use winit::window::{CursorGrabMode, CursorIcon, Fullscreen, Icon, Window, WindowId};

use cosmic_text::{
    fontdb, Attrs, Buffer as CtBuffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    SwashContent, Weight,
};

use unicode_segmentation::UnicodeSegmentation;

use crate::host::text_width::cluster_cells;

/// The winit cursor for a frontend-neutral `CursorShape`.
fn cursor_icon(shape: super::CursorShape) -> CursorIcon {
    match shape {
        super::CursorShape::ColResize => CursorIcon::EwResize, // ↔ side-by-side divider
        super::CursorShape::RowResize => CursorIcon::NsResize, // ↕ stacked divider
        super::CursorShape::Pointer => CursorIcon::Pointer,    // 👆 a clickable link
    }
}

/// The cursor shape for the pointer at cell `(col, row)`, given the window's
/// zones — the first zone containing the point, or None (default cursor).
fn shape_at(
    zones: &[(u16, u16, u16, u16, super::CursorShape)],
    col: u16,
    row: u16,
) -> Option<super::CursorShape> {
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
const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/DejaVuSansMono.ttf");

const FONT_BOLD: &[u8] = include_bytes!("../../../assets/DejaVuSansMono-Bold.ttf");

const FONT_ITALIC: &[u8] = include_bytes!("../../../assets/DejaVuSansMono-Oblique.ttf");

const FONT_BOLD_ITALIC: &[u8] = include_bytes!("../../../assets/DejaVuSansMono-BoldOblique.ttf");

// Bundled color emoji font (CBDT), loaded only as a *fallback*: a cluster the mono
// font can't cover (an emoji, a flag, a CJK char, …) is shaped + rasterised from
// here by cosmic-text/swash, in color. Not a selectable `:family`. ~11 MB.
const FONT_EMOJI: &[u8] = include_bytes!("../../../assets/NotoColorEmoji.ttf");

// The family name fontdb assigns the bundled mono faces — what we pass as the
// primary `Attrs` family; cosmic-text's fallback list then reaches the emoji font.
const MONO_FAMILY: &str = "DejaVu Sans Mono";

// The default font family keyword (`:mono`), and the default cell pixel size.
const DEFAULT_FAMILY: &str = "mono";

const DEFAULT_PX: f32 = 15.0;

// Cell height as a multiple of the font px. A touch looser than a terminal's
// typical ~1.2 so text gets vertical breathing room and the grid reads as an
// editor, not a console. Drives `cell_h` (and so the row count) in `recompute`.
const LINE_HEIGHT: f32 = 1.4;

/// The velocity a wheel notch of `dy` (±1 per notch on most mice) leaves the kinetic
/// scroll with, on top of the `carried` velocity of a glide already running the same
/// way: the notch's impulse, boosted by its place in a `streak` of quick notches.
fn wheel_velocity(carried: f64, streak: u32, dy: f64) -> f64 {
    let boost = (1.0 + WHEEL_STREAK_BOOST * streak as f64).min(WHEEL_STREAK_CAP);
    carried + WHEEL_IMPULSE * boost * dy.signum() * dy.abs().max(1.0)
}

/// Where a glide starting at `v0` lines per tick ends up, in lines, decaying at `decay`
/// per tick until the `about_to_wait` cutoff — the geometric sum, stepped as the ticker
/// steps it (the test's oracle for the calibration constants).
#[cfg(test)]
fn glide_distance(mut v: f64, decay: f64) -> f64 {
    let mut total = 0.0;
    while v.abs() >= 0.0005 {
        total += v;
        v *= decay;
    }
    total
}

#[cfg(test)]
mod wheel_tests {
    use super::*;

    /// One notch glides about three lines — the distance a plain wheel notch scrolled
    /// before it glided — and a notch against a running glide starts over.
    #[test]
    fn a_single_notch_glides_about_three_lines() {
        let v0 = wheel_velocity(0.0, 0, 1.0);
        let lines = glide_distance(v0, WHEEL_DECAY);
        assert!((2.7..3.3).contains(&lines), "a notch glides {lines} lines");
        assert!(wheel_velocity(0.0, 0, -1.0) < 0.0);
    }

    /// A streak accelerates slightly and is capped: the fifth quick notch scrolls
    /// farther than the first, the twentieth no farther than the cap allows.
    #[test]
    fn a_streak_accelerates_slightly_and_is_capped() {
        let first = wheel_velocity(0.0, 0, 1.0);
        let fifth = wheel_velocity(0.0, 4, 1.0);
        let twentieth = wheel_velocity(0.0, 19, 1.0);
        assert!(
            fifth > first * 1.5 && fifth < first * 2.0,
            "fifth: {fifth} vs first {first}"
        );
        assert!(
            (twentieth - first * WHEEL_STREAK_CAP).abs() < 1e-9,
            "the cap holds"
        );
        // and a notch on top of a running glide adds to it rather than replacing it
        assert!(wheel_velocity(0.3, 0, 1.0) > first);
    }
}

/// Kinetic scrolling (`about_to_wait`): the velocity decay per nominal 12 ms tick. A
/// trackpad flick coasts long; a wheel notch glides for about half a second.
const TRACKPAD_DECAY: f64 = 0.97;
const WHEEL_DECAY: f64 = 0.85;
/// Lines one wheel notch scrolls in total, as the geometric sum of its glide:
/// `WHEEL_IMPULSE / (1 - WHEEL_DECAY)` — 0.45 per tick decaying at 0.85 is 3 lines.
const WHEEL_IMPULSE: f64 = 0.45;
/// A notch within this many ms of the previous one joins a streak; each notch of a
/// streak scrolls `1 + WHEEL_STREAK_BOOST × streak` as far, up to `WHEEL_STREAK_CAP` —
/// the slight acceleration of a spun wheel, without a flung page.
const WHEEL_STREAK_MS: u128 = 220;
const WHEEL_STREAK_BOOST: f64 = 0.18;
const WHEEL_STREAK_CAP: f64 = 2.2;

// The rendering *mechanism's* fallback colours — used ONLY when Brood supplies
// none: `Op::Clear` / the inset-margin fill when no `gui-bg!` is set, and a face
// with no `:bg`/`:fg`. Brood owns the actual palette as *policy* — every render op
// carries its own `[r g b]`/hex colour resolved through `std/editor/face.blsp`, so
// these are defaults, not a duplicated palette (there is no `theme.blsp`). Catppuccin
// Mocha base/text tones, chosen so an un-styled window still reads as an editor.
const DEFAULT_BG: [u8; 3] = [0x1e, 0x1e, 0x2e];

const DEFAULT_FG: [u8; 3] = [0xcd, 0xd6, 0xf4];

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
        spec: WindowSpec,
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
    Icon {
        id: u64,
        rgba: Vec<u8>,
        w: u32,
        h: u32,
    },
    /// Raise window `id` to the front and give it OS keyboard focus (un-
    /// minimising it first). Behind `gui-focus` — surfaces an already-open
    /// singleton window instead of opening a duplicate.
    Focus { id: u64 },
    /// Confine the pointer to window `id` (`on`) or release it. Behind
    /// `gui-grab-cursor` — keeps the cursor inside the window for mouse-look so
    /// it can't slip out and click another app.
    Grab { id: u64, on: bool },
    /// Maximise window `id` (`on`) or restore it. Unlike fullscreen this keeps
    /// the title bar / decorations — it just fills the screen's work area. Behind
    /// `gui-maximize!` — what an editor `init.blsp` toggles to open big.
    Maximize { id: u64, on: bool },
    /// Iconify window `id`. The counterpart of `Maximize` for an app that draws
    /// its own window controls and therefore has no OS minimise button.
    Minimize { id: u64 },
    /// Hand window `id` to the window manager for an interactive **move** —
    /// the gesture an OS title bar would have provided. A borderless window
    /// (`decorations: false`) has no title bar to grab, so the app nominates a
    /// region of its own chrome (a browser's tab strip) and calls this on press.
    DragMove { id: u64 },
    /// Hand window `id` to the window manager for an interactive **resize** from
    /// `dir` — the gesture an OS window frame would have provided.
    DragResize { id: u64, dir: String },
    /// Make window `id` borderless-fullscreen (`on`) or restore it — fills the
    /// whole monitor with no title bar / decorations (distraction-free). Behind
    /// `gui-fullscreen!`; the title-keeping sibling is `Maximize`.
    Fullscreen { id: u64, on: bool },
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
    /// Set the content inset (logical px) — the margin before the cell grid on
    /// every window edge — for every open window and ones opened later. Behind
    /// `gui-inset!`. The grid loses `2*inset` px of usable area per axis, so its
    /// cell count shrinks; the window re-renders at the new size.
    Inset { px: f32 },
    /// Set the window background — the fill for `Op::Clear`, the pre-clear, and the
    /// inset margin / snap remainder outside the cell grid — for every open window
    /// and ones opened later. Behind `gui-bg!`; `None` restores `DEFAULT_BG`. Pure
    /// repaint, no metric change.
    Background { rgb: Option<[u8; 3]> },
    /// Set the cell height as a multiple of the font px — for every open window and
    /// ones opened later. Behind `gui-line-height!`. A metric change, like a font
    /// change: the row count moves, so each window is told its new grid and re-renders.
    LineHeight { mult: f32 },
    /// Set how monochrome text is anti-aliased (gray / subpixel / auto) — for every
    /// open window and ones opened later. Behind `gui-text-aa!`. A pure repaint.
    TextAa { mode: TextAa },
    /// Set the text contrast exponent — for every open window and ones opened later.
    /// Behind `gui-text-contrast!`. A pure repaint.
    TextContrast { gamma: f32 },
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

/// HEADLESS mode (`BROOD_GUI_HEADLESS=1`): a gui-built runtime that opens NO real
/// OS window — `gui-open` hands back a fake window with a fixed cell grid, every
/// draw/window op is a silent no-op, and no key/mouse events ever arrive. A
/// windowed app's loop runs unchanged (paced by its own `(after)` timer), so it
/// can be tested / soak-run / CI'd with no popup. Read once at first use.
fn headless() -> bool {
    static H: OnceLock<bool> = OnceLock::new();
    *H.get_or_init(|| {
        std::env::var("BROOD_GUI_HEADLESS")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    })
}

/// The cell grid a headless window reports for a requested logical pixel `size`
/// (default 840×560 like a real `gui-open`), using a nominal 8×16 px cell.
fn headless_cells(size: Option<(f64, f64)>) -> (u16, u16) {
    let (w, h) = size.unwrap_or((840.0, 560.0));
    (((w / 8.0) as u16).max(1), ((h / 16.0) as u16).max(1))
}

/// Can the event loop live on a thread we choose, or must it own the **process main
/// thread**?
///
/// Wayland/X11 let a loop run anywhere, which is why the GUI has always been a
/// dedicated `brood-gui` thread. macOS does not: AppKit's run loop is main-thread-only
/// and winit exposes no `with_any_thread` there — so the gui feature simply did not
/// compile for macOS until 2026-09-11 (KI-125). Windows is the same shape. Anything not
/// on the permissive list therefore hosts the loop on the main thread.
const DEDICATED_THREAD_OK: bool = cfg!(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
));

/// `BROOD_GUI_MAIN_THREAD=1` forces the main-thread path on a platform that does not
/// need it. It exists for one reason and it is not configuration: the main-thread path
/// is the ONLY path macOS can take, and there is no macOS in this project's CI beyond a
/// compile check — so without a lever it would ship having never once been run. With it
/// the identical code is exercised on Linux, where the GUI is actually tested.
fn main_thread_hosting_required() -> bool {
    static R: OnceLock<bool> = OnceLock::new();
    *R.get_or_init(|| {
        hosting_required_from(
            DEDICATED_THREAD_OK,
            std::env::var("BROOD_GUI_MAIN_THREAD").ok().as_deref(),
        )
    })
}

/// The decision itself, kept free of `cfg!`, the environment and the cache above so it
/// can be tested for every platform from whichever one the tests happen to run on —
/// which matters here more than usual, since the case that motivated this code
/// (`dedicated_ok == false`) is a platform CI only ever *compiles*.
fn hosting_required_from(dedicated_ok: bool, lever: Option<&str>) -> bool {
    if !dedicated_ok {
        // Not a preference. macOS/Windows have nowhere else to put the loop, so the
        // lever cannot switch this off — `BROOD_GUI_MAIN_THREAD=0` there would only
        // turn a working GUI into one that fails to start.
        return true;
    }
    matches!(lever, Some(v) if v != "0" && !v.is_empty())
}

/// What the process main thread can be asked to do while the runtime runs elsewhere.
enum MainMsg {
    /// Build and run the one event loop *here*, replying with its proxy. Never returns:
    /// winit's `run_app` owns the thread from then on.
    HostGui(Sender<Result<EventLoopProxy<UserEvent>, String>>),
    /// The runtime thread finished without ever wanting a window; return normally.
    RuntimeDone,
}

/// The channel `host_main_thread` is listening on, published so `start_thread` can
/// reach it. `None` until a binary reserves its main thread — a library embedding the
/// runtime need not, and gets a clear error rather than a deadlock.
fn main_slot() -> &'static Mutex<Option<Sender<MainMsg>>> {
    static S: OnceLock<Mutex<Option<Sender<MainMsg>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Set once the main thread has been handed to winit, so the joiner below knows a
/// returning runtime can no longer be reported back through it.
static GUI_OWNS_MAIN: AtomicBool = AtomicBool::new(false);

const MAIN_THREAD_UNRESERVED: &str =
    "gui: this platform requires the event loop on the process main thread, and this \
     binary did not reserve it. A host embedding the brood runtime must run its work \
     through `cli_support::run_on_main_stack`, which parks the main thread for exactly \
     this.";

/// Run the runtime to completion while keeping the **process main thread** available
/// for a GUI event loop, and return whatever the runtime returned.
///
/// `handle` is the already-spawned runtime thread (`cli_support::run_on_main_stack`
/// sized it; that is the whole reason the runtime is not on the main thread in the
/// first place). On a platform where a dedicated GUI thread is fine this is exactly the
/// `join()` it replaces. Where it is not, this parks here instead: a small joiner
/// thread waits on the runtime, and the main thread blocks until either the runtime
/// finishes (return normally) or a `gui-open` asks for the loop — in which case winit
/// takes this thread for the life of the process.
///
/// The process then ends the way it always did, through the `std::process::exit` the
/// runtime's own exit path calls. The one case needing help is a runtime that *returns*
/// while winit owns this thread: nothing would notice, so the joiner exits the process
/// as a returning `main` would have.
pub fn host_main_thread<T: Send + 'static>(handle: JoinHandle<T>, name: &str) -> T {
    if !main_thread_hosting_required() {
        return handle
            .join()
            .unwrap_or_else(|_| panic!("{name} thread panicked"));
    }
    let (tx, rx) = mpsc::channel::<MainMsg>();
    *main_slot().lock().unwrap() = Some(tx.clone());

    let result: Arc<Mutex<Option<std::thread::Result<T>>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&result);
    let joined = std::thread::Builder::new()
        .name("brood-main-join".into())
        .spawn(move || {
            let r = handle.join();
            *sink.lock().unwrap() = Some(r);
            if GUI_OWNS_MAIN.load(Ordering::SeqCst) {
                // winit is never giving this thread back, so a returning runtime has to
                // end the process itself. Exit 0 is what a `main` that returned would
                // have produced; every non-zero path already went through
                // `std::process::exit` before reaching here.
                std::process::exit(0);
            }
            let _ = tx.send(MainMsg::RuntimeDone);
        });
    if let Err(e) = joined {
        panic!("spawn brood-main-join thread: {e}");
    }

    loop {
        match rx.recv() {
            Ok(MainMsg::HostGui(ready)) => run_gui(ready),
            Ok(MainMsg::RuntimeDone) | Err(_) => break,
        }
    }
    let outcome = result
        .lock()
        .unwrap()
        .take()
        .expect("runtime finished without recording a result");
    outcome.unwrap_or_else(|_| panic!("{name} thread panicked"))
}

/// Spawn the GUI thread + build the (single) event loop; return a proxy to it.
fn start_thread() -> Result<EventLoopProxy<UserEvent>, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<EventLoopProxy<UserEvent>, String>>();
    if main_thread_hosting_required() {
        // Hand the loop to the parked main thread rather than spawning one. Safe to set
        // the flag after the send succeeds and before awaiting the proxy: the caller is
        // blocked in `recv` below, so the runtime cannot finish in the window between.
        let tx = main_slot()
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| MAIN_THREAD_UNRESERVED.to_string())?;
        tx.send(MainMsg::HostGui(ready_tx))
            .map_err(|_| "gui: the process main thread is gone".to_string())?;
        GUI_OWNS_MAIN.store(true, Ordering::SeqCst);
    } else {
        std::thread::Builder::new()
            .name("brood-gui".into())
            .spawn(move || run_gui(ready_tx))
            .map_err(|e| e.to_string())?;
    }
    ready_rx
        .recv()
        .map_err(|_| "gui thread exited during init".to_string())?
}

/// `(gui-open subscriber)` — open a new window whose key/mouse input is
/// delivered to process `subscriber`'s mailbox; return the window id. Starts the
/// GUI thread on the first call. Each call is an independent window.
pub fn open(subscriber: u64, spec: WindowSpec) -> Result<u64, String> {
    // Headless: register a fake window (fixed cell grid, no input) without ever
    // starting winit, so nothing pops up.
    if headless() {
        let id = next_id();
        windows().lock().unwrap().insert(
            id,
            WinHandle {
                size: Arc::new(Mutex::new(headless_cells(spec.size))),
                held_key: Arc::new(Mutex::new(None)),
            },
        );
        return Ok(id);
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    // Send under the proxy lock, then drop it before awaiting the reply so a
    // slow window build can't block other windows' sends.
    gui()?
        .lock()
        .unwrap()
        .send_event(UserEvent::Open {
            subscriber,
            spec,
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
    if headless() {
        return Ok(());
    }
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
    if headless() {
        return Ok(());
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
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap()
        .send_event(UserEvent::Grab { id, on })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-maximize! id on)` — maximise window `id` (`on` true) or restore it,
/// keeping the title bar. Dispatched to the GUI thread like `grab`.
pub fn maximize(id: u64, on: bool) -> Result<(), String> {
    {
        let w = windows().lock().unwrap();
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap()
        .send_event(UserEvent::Maximize { id, on })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-minimize! id)` — iconify window `id`. Dispatched like `maximize`.
pub fn minimize(id: u64) -> Result<(), String> {
    {
        let w = windows().lock().unwrap();
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap()
        .send_event(UserEvent::Minimize { id })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-drag-move id)` — start an interactive window move.
pub fn drag_move(id: u64) -> Result<(), String> {
    {
        let w = windows().lock().unwrap();
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap()
        .send_event(UserEvent::DragMove { id })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-drag-resize id dir)` — start an interactive resize from edge/corner `dir`.
pub fn drag_resize(id: u64, dir: &str) -> Result<(), String> {
    {
        let w = windows().lock().unwrap();
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap()
        .send_event(UserEvent::DragResize {
            id,
            dir: dir.to_string(),
        })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-fullscreen! id on)` — make window `id` borderless-fullscreen (`on`
/// true) or restore it. Dispatched to the GUI thread like `grab`.
pub fn fullscreen(id: u64, on: bool) -> Result<(), String> {
    {
        let w = windows().lock().unwrap();
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap()
        .send_event(UserEvent::Fullscreen { id, on })
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
    if headless() {
        return Ok(());
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
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g
            .lock()
            .unwrap()
            .send_event(UserEvent::Font { id, family, px });
    }
    Ok(())
}

/// `(gui-inset! px)` — set the content inset (logical px) on every window + the
/// global default for ones opened later. No-op (silently) if the GUI thread never
/// started.
pub fn inset(px: f32) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g.lock().unwrap().send_event(UserEvent::Inset { px });
    }
    Ok(())
}

/// `(gui-bg! rgb)` — set the window background (clear / inset-margin / snap-remainder
/// fill) on every window + the default for ones opened later. `None` restores
/// `DEFAULT_BG`. No-op (silently) if the GUI thread never started.
pub fn bg(rgb: Option<[u8; 3]>) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g.lock().unwrap().send_event(UserEvent::Background { rgb });
    }
    Ok(())
}

/// `(gui-line-height! mult)` — set the cell height as a multiple of the font px on
/// every window + the default for ones opened later. No-op (silently) if the GUI
/// thread never started.
pub fn line_height(mult: f32) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g.lock().unwrap().send_event(UserEvent::LineHeight { mult });
    }
    Ok(())
}

/// `(gui-text-aa! mode)` — set how monochrome text is anti-aliased on every window +
/// the default for ones opened later. No-op (silently) if the GUI thread never started.
pub fn text_aa(mode: TextAa) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g.lock().unwrap().send_event(UserEvent::TextAa { mode });
    }
    Ok(())
}

/// `(gui-text-contrast! gamma)` — set the text contrast exponent on every window + the
/// default for ones opened later. No-op (silently) if the GUI thread never started.
pub fn text_contrast(gamma: f32) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g
            .lock()
            .unwrap()
            .send_event(UserEvent::TextContrast { gamma });
    }
    Ok(())
}

/// `(gui-title! id text)` — set window `id`'s title-bar text at runtime. Routed
/// through the event-loop proxy like `font`; a no-op (silently) if the GUI thread
/// never started or `id` isn't a live window.
pub fn title(id: u64, title: String) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g.lock().unwrap().send_event(UserEvent::Title { id, title });
    }
    Ok(())
}

/// `(gui-icon! id rgba w h)` — set window `id`'s taskbar/title-bar icon from raw
/// RGBA pixels at runtime. Routed through the proxy like `title`; a silent no-op
/// if the GUI thread never started or `id` isn't a live window.
pub fn icon(id: u64, rgba: Vec<u8>, w: u32, h: u32) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g
            .lock()
            .unwrap()
            .send_event(UserEvent::Icon { id, rgba, w, h });
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
    if headless() {
        return Ok(());
    }
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

/// One open window's GUI-thread-side state.
/// A window's render backend, chosen ONCE at `build_window`. The default `gui` build
/// only has `Cpu` (softbuffer). The `gui-gpu` build also has `Gpu` and picks it when
/// `BROOD_GUI_GPU` is set in the environment — so ONE installed binary defaults to the
/// CPU softbuffer (safe for every app) and a single project opts into the GPU
/// per-process via that env var, without a separate build.
enum Backend {
    Cpu {
        // Keeps the softbuffer display connection alive for `surface`'s lifetime.
        _context: softbuffer::Context<Rc<Window>>,
        surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    },
    // Boxed: `GlWindow` is ~6 KB (GL context + programs + glyph cache), which
    // would bloat every `Backend` — including the common `Cpu` one — to that
    // size. The box keeps the enum a couple of words; the GPU path is cold.
    #[cfg(feature = "gui-gpu")]
    Gpu(Box<crate::host::gui::gpu::GlWindow>),
}

#[cfg(feature = "gui-gpu")]
fn gpu_enabled() -> bool {
    std::env::var("BROOD_GUI_GPU")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(false)
}

fn cpu_backend(window: &Rc<Window>) -> Result<Backend, String> {
    let context =
        softbuffer::Context::new(window.clone()).map_err(|e| format!("softbuffer context: {e}"))?;
    let surface = softbuffer::Surface::new(&context, window.clone())
        .map_err(|e| format!("softbuffer surface: {e}"))?;
    Ok(Backend::Cpu {
        _context: context,
        surface,
    })
}

struct Win {
    window: Rc<Window>,
    backend: Backend,
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
    /// The last press's click-chain state — `(count, button, cell, when)` — so a
    /// fresh press in the same cell with the same button within `MULTI_CLICK_MS`
    /// increments the count (double/triple-click); anything else restarts at 1.
    last_click: Option<(u8, MouseButton, (u16, u16), Instant)>,
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
    /// EMA of the scroll velocity (signed lines/step) tracked during a trackpad gesture.
    /// On `TouchPhase::Ended` this seeds the kinetic-momentum animation driven by
    /// `about_to_wait`. On `Started`, any existing momentum velocity is carried forward
    /// so successive swipes accelerate rather than reset.
    scroll_velocity: f64,
    /// `true` while kinetic momentum is running after a gesture lift-off. Cleared by the
    /// next `Started/Moved/LineDelta` event or when velocity decays below threshold.
    scroll_momentum_active: bool,
    /// The per-12 ms velocity decay of the running momentum: `TRACKPAD_DECAY` after a
    /// gesture lift-off (a long coast), `WHEEL_DECAY` for a wheel notch (a short glide).
    scroll_decay: f64,
    /// When the last wheel notch arrived and how many came in quick succession —
    /// the streak that gives a spun wheel its slight acceleration.
    wheel_last: Option<Instant>,
    wheel_streak: u32,
    /// The earliest wall-clock time the next momentum step may fire. Guards against
    /// about_to_wait being called too frequently (e.g. on every UserEvent::Draw).
    scroll_next_tick: Instant,
    /// Time the previous momentum step was delivered, used for time-proportional decay
    /// (`0.97 ^ (elapsed_ms / 12)`). Keeps the physics correct when about_to_wait
    /// fires late so the coast speed is render-throughput independent.
    scroll_last_tick: Instant,
    /// `true` while a momentum scroll event is in-flight — i.e. delivered to Brood
    /// but the corresponding UserEvent::Draw has not yet arrived. A new step is never
    /// delivered while this is set, so the queue never grows beyond one pending event
    /// even when Brood's render pipeline is slower than the 12 ms tick rate.
    scroll_pending: bool,
}

/// The global renderer settings a window opens with — each behind a `gui-*!`
/// primitive whose `id`-less form sets the default for windows opened later as well
/// as every open one. One struct rather than a growing positional tail through
/// `build_window` (the `WindowSpec` lesson).
#[derive(Clone, Copy)]
struct RenderDefaults {
    /// Default cell font family (interned keyword id); `None` = the bundled mono.
    family: Option<u32>,
    /// Default cell font size, logical px.
    px: f32,
    /// Content inset (logical px) before the grid on every edge.
    inset: f32,
    /// Window background; `None` = `DEFAULT_BG`.
    bg: Option<[u8; 3]>,
    /// Cell height as a multiple of the font px.
    line_height: f32,
    /// How monochrome text is anti-aliased.
    text_aa: TextAa,
    /// The text contrast exponent (1.0 = the plain linear-light blend).
    text_contrast: f32,
}

impl Default for RenderDefaults {
    fn default() -> Self {
        RenderDefaults {
            family: None,
            px: DEFAULT_PX,
            inset: 0.0,
            bg: None,
            line_height: LINE_HEIGHT,
            text_aa: TextAa::Auto,
            text_contrast: 1.0,
        }
    }
}

/// Build a window + softbuffer surface + glyph renderer inside the running event
/// loop. Errors (window / surface creation) propagate to the `open` caller.
fn build_window(
    elwt: &ActiveEventLoop,
    subscriber: u64,
    spec: WindowSpec,
    families: Families,
    defaults: RenderDefaults,
) -> Result<Win, String> {
    let (w, h) = spec.size.unwrap_or((840.0, 560.0));
    let attributes = Window::default_attributes()
        .with_title(spec.title.unwrap_or_else(|| "Brood".to_string()))
        .with_decorations(spec.decorations)
        .with_inner_size(LogicalSize::new(w, h));
    // The desktop app id, on the platforms that have one. There is no protocol for
    // changing it afterwards, which is why it belongs to the build and not to a
    // `gui-*!` op. winit keeps ONE `platform_specific.name`, read by whichever
    // backend is live, so the Wayland trait covers X11's `WM_CLASS` too — the
    // `general` half is Wayland's `app_id` and X11's class, `instance` is X11's
    // res-name (a no-op on Wayland).
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    let attributes = match spec.app_id.as_deref() {
        Some(app_id) => {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attributes.with_name(app_id, app_id)
        }
        None => attributes,
    };
    let window = elwt
        .create_window(attributes)
        .map_err(|e| format!("window: {e}"))?;
    let window = Rc::new(window);
    // The GPU backend only when built AND opted-in via the env; everything else (the
    // default build, or no env) is the CPU softbuffer — so other apps stay on CPU.
    #[cfg(feature = "gui-gpu")]
    let backend = if gpu_enabled() {
        eprintln!("brood gui: GPU (OpenGL) backend active");
        Backend::Gpu(Box::new(crate::host::gui::gpu::GlWindow::new(
            window.clone(),
        )?))
    } else {
        cpu_backend(&window)?
    };
    #[cfg(not(feature = "gui-gpu"))]
    let backend = cpu_backend(&window)?;
    let mut renderer = Renderer::new(window.scale_factor(), families, defaults.px);
    // honour the global defaults set before this window opened
    if let Some(f) = defaults.family {
        renderer.set_font(Some(f), None);
    }
    renderer.set_inset(defaults.inset);
    renderer.set_bg(defaults.bg);
    renderer.set_line_height(defaults.line_height);
    renderer.set_text_aa(defaults.text_aa);
    renderer.set_text_contrast(defaults.text_contrast);
    Ok(Win {
        window,
        backend,
        renderer,
        size: Arc::new(Mutex::new((80, 24))),
        subscriber,
        frame: Vec::new(),
        mods: ModifiersState::empty(),
        cursor: (0, 0),
        held: None,
        last_click: None,
        held_key: Arc::new(Mutex::new(None)),
        held_physical: None,
        zones: Vec::new(),
        shape: None,
        scroll_velocity: 0.0,
        scroll_momentum_active: false,
        scroll_decay: TRACKPAD_DECAY,
        wheel_last: None,
        wheel_streak: 0,
        scroll_next_tick: Instant::now(),
        scroll_last_tick: Instant::now(),
        scroll_pending: false,
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
    /// The renderer settings windows opened later start with (font, inset,
    /// background, line height, text AA) — each `gui-*!` default arm updates it.
    defaults: RenderDefaults,
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
    pending_open: Vec<(u64, WindowSpec, Sender<Result<OpenReply, String>>)>,
}

impl GuiApp {
    /// Create a window for `subscriber` and register it, replying to the
    /// `open` caller with its id + shared size (or the build error). Shared by
    /// the `Open` user event and the `resumed` drain so the path is identical.
    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        subscriber: u64,
        spec: WindowSpec,
        reply: Sender<Result<OpenReply, String>>,
    ) {
        let id = next_id();
        match build_window(
            event_loop,
            subscriber,
            spec,
            self.families.clone(),
            self.defaults,
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
        for (subscriber, spec, reply) in std::mem::take(&mut self.pending_open) {
            self.open_window(event_loop, subscriber, spec, reply);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            // Create now if the display is live, else queue until `resumed`.
            UserEvent::Open {
                subscriber,
                spec,
                reply,
            } => {
                if self.resumed {
                    self.open_window(event_loop, subscriber, spec, reply);
                } else {
                    self.pending_open.push((subscriber, spec, reply));
                }
            }
            // Set a live window's OS title-bar text (behind gui-title!).
            UserEvent::Title { id, title } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_title(&title);
                }
            }
            // Set a live window's taskbar/title-bar icon (behind gui-icon!).
            UserEvent::Icon {
                id,
                rgba,
                w: iw,
                h: ih,
            } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    if let Ok(ic) = Icon::from_rgba(rgba, iw, ih) {
                        win.window.set_window_icon(Some(ic));
                    }
                }
            }
            UserEvent::Draw { id, ops } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    // Brood finished rendering; the next momentum step may fire.
                    w.scroll_pending = false;
                    // A frame identical to the one on screen — a timer that fired and
                    // changed nothing, a model turn whose view is unchanged — costs no
                    // repaint at all. The op vocabulary is plain data, so equality is exact.
                    if ops == w.frame {
                        return;
                    }
                    // Refresh the cursor hot-zones from this frame, then store it.
                    w.zones = ops
                        .iter()
                        .filter_map(|op| match op {
                            Op::CursorZone {
                                x,
                                y,
                                w: zw,
                                h,
                                shape,
                            } => Some((*x, *y, *zw, *h, *shape)),
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
            // Maximise the window (fill the work area, keep the title bar) or
            // restore it. The resize that follows republishes the new cell grid
            // like any other, so the loop re-renders at the bigger size.
            UserEvent::Minimize { id } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_minimized(true);
                }
            }
            // Both drags hand the gesture to the window manager, which then owns
            // it until the button is released — so a failure here (an unsupported
            // platform, or no button actually held) is ignored rather than raised:
            // the app asked politely and the WM declined.
            UserEvent::DragMove { id } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    let _ = w.window.drag_window();
                }
            }
            UserEvent::DragResize { id, dir } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    use winit::window::ResizeDirection as RD;
                    let d = match dir.as_str() {
                        "north" => RD::North,
                        "south" => RD::South,
                        "east" => RD::East,
                        "west" => RD::West,
                        "north-east" => RD::NorthEast,
                        "north-west" => RD::NorthWest,
                        "south-east" => RD::SouthEast,
                        _ => RD::SouthWest,
                    };
                    let _ = w.window.drag_resize_window(d);
                }
            }
            UserEvent::Maximize { id, on } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_maximized(on);
                }
            }
            // Borderless-fullscreen (`Borderless(None)` = the current monitor) or
            // restore. Like Maximize the resize that follows re-renders the grid.
            UserEvent::Fullscreen { id, on } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_fullscreen(if on {
                        Some(Fullscreen::Borderless(None))
                    } else {
                        None
                    });
                }
            }
            // Cell font. `id: Some(w)` retunes just that window, leaving the
            // global default alone (so two windows can differ). `id: None` is
            // the global default: remembered for future windows and applied to
            // every open one. Either way a target window recomputes its grid +
            // republishes its size + redraws (`apply_font`).
            UserEvent::Font { id, family, px } => match id {
                Some(target) => {
                    if let Some(w) = self.ids.get(&target).and_then(|wid| self.wins.get_mut(wid)) {
                        apply_font(w, family, px);
                    }
                }
                None => {
                    if let Some(f) = family {
                        self.defaults.family = Some(f);
                    }
                    if let Some(p) = px {
                        self.defaults.px = p.max(1.0);
                    }
                    for w in self.wins.values_mut() {
                        apply_font(w, family, px);
                    }
                }
            },
            UserEvent::Inset { px } => {
                self.defaults.inset = px.max(0.0);
                for w in self.wins.values_mut() {
                    w.renderer.set_inset(px);
                    update_cells(&w.window, &w.renderer, &w.size);
                    w.window.request_redraw();
                }
            }
            UserEvent::Background { rgb } => {
                self.defaults.bg = rgb;
                for w in self.wins.values_mut() {
                    w.renderer.set_bg(rgb);
                    // Background is a pure repaint — no metric change, so no
                    // `update_cells`/snap.
                    w.window.request_redraw();
                }
            }
            UserEvent::LineHeight { mult } => {
                self.defaults.line_height = mult;
                for w in self.wins.values_mut() {
                    w.renderer.set_line_height(mult);
                    // The cell height moved, so the grid did: the same path as a font
                    // change — new (cols, rows) to the app, then a repaint.
                    update_cells(&w.window, &w.renderer, &w.size);
                    let (cols, rows) = *w.size.lock().unwrap();
                    deliver(w.subscriber, resize_message(cols, rows));
                    w.window.request_redraw();
                }
            }
            UserEvent::TextAa { mode } => {
                self.defaults.text_aa = mode;
                for w in self.wins.values_mut() {
                    w.renderer.set_text_aa(mode);
                    w.window.request_redraw();
                }
            }
            UserEvent::TextContrast { gamma } => {
                self.defaults.text_contrast = gamma;
                for w in self.wins.values_mut() {
                    w.renderer.set_text_contrast(gamma);
                    w.window.request_redraw();
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
                // old glyphs, forget the retained frame (its ops are unchanged but
                // their glyphs are not), and repaint.
                for w in self.wins.values_mut() {
                    w.renderer.cache.clear();
                    w.renderer.invalidate();
                    w.window.request_redraw();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
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
                // The sub-cell remainder is placed at paint time (`grid_origin`:
                // centred horizontally, anchored top vertically), so there's no
                // window-resize snap to do here — it works on every WM.
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
                    deliver(
                        w.subscriber,
                        mouse_message(&release_of(b, col, row, &w.mods)),
                    );
                }
            }
            WindowEvent::CursorLeft { .. } => {
                // The pointer left the window. If a button was held, its release happens
                // outside and we never see it — so the NEXT re-entry's motion would emit
                // a phantom `:drag` and the app would think the button is still pressed.
                // Synthesize the release + clear `held` (mirrors the keyboard blur fix).
                if let Some(b) = w.held.take() {
                    let (col, row) = w.cursor;
                    deliver(
                        w.subscriber,
                        mouse_message(&release_of(b, col, row, &w.mods)),
                    );
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Track the pointer cell. Bare motion (no button) isn't emitted —
                // no consumer, and a per-pixel event would flood + force redraws.
                // But while a button is held, crossing into a NEW cell emits a
                // `:drag` (cell-granular, so still bounded), which is how a divider
                // drag is tracked (ADR-077).
                let psz = w.window.inner_size();
                let cell = px_to_cell(
                    position,
                    &w.renderer,
                    psz.width as usize,
                    psz.height as usize,
                );
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
                            count: 0,
                            scroll_dy: 0.0,
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
                    // Click-chain count: a fresh press in the same cell, with the same
                    // button, within MULTI_CLICK_MS of the previous extends the chain
                    // (double/triple-click); anything else restarts at 1.
                    let now = Instant::now();
                    let count = match w.last_click {
                        Some((c, lb, lcell, lt))
                            if lb == b
                                && lcell == (col, row)
                                && now.duration_since(lt).as_millis() <= MULTI_CLICK_MS =>
                        {
                            c.saturating_add(1)
                        }
                        _ => 1,
                    };
                    w.last_click = Some((count, b, (col, row), now));
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
                            count,
                            scroll_dy: 0.0,
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
                            count: 0,
                            scroll_dy: 0.0,
                        }),
                    );
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                // Positive y scrolls up (away from the user). LineDelta is in discrete
                // line units; PixelDelta (trackpad) is in physical pixels — normalised
                // here to line units so Brood gets a consistent float in both cases.
                //
                // Momentum is only for PixelDelta (smooth trackpad). LineDelta (mouse
                // wheel clicks) always delivers immediately and stops any running momentum.
                // Momentum itself is driven by `about_to_wait` with `WaitUntil(12ms)` so
                // it fires once per event-loop cycle after the queue is drained — no
                // flooding, no background thread.
                let ch = w.renderer.cell_h.max(1);
                match delta {
                    MouseScrollDelta::LineDelta(_, y) => {
                        // A wheel notch is an IMPULSE into the same kinetic scroll a
                        // trackpad flick runs, not a three-line jump: the glide reaches
                        // the notch's distance over ~half a second of fractional steps
                        // (`WHEEL_DECAY`), so the text moves rather than snaps. Notches in
                        // quick succession form a streak that scrolls slightly farther
                        // each — a spun wheel accelerates — capped so it never flings.
                        // A notch against the running direction stops the glide first.
                        let dy = y as f64;
                        if dy == 0.0 {
                            return;
                        }
                        let now = Instant::now();
                        let streak = match w.wheel_last {
                            Some(t) if now.duration_since(t).as_millis() <= WHEEL_STREAK_MS => {
                                w.wheel_streak.saturating_add(1)
                            }
                            _ => 0,
                        };
                        w.wheel_last = Some(now);
                        w.wheel_streak = streak;
                        let same_way = w.scroll_momentum_active && w.scroll_velocity * dy > 0.0;
                        let carried = if same_way { w.scroll_velocity } else { 0.0 };
                        w.scroll_velocity = wheel_velocity(carried, streak, dy);
                        w.scroll_decay = WHEEL_DECAY;
                        w.scroll_pending = false;
                        w.scroll_momentum_active = true;
                        w.scroll_next_tick = now;
                        w.scroll_last_tick = now;
                        event_loop.set_control_flow(ControlFlow::WaitUntil(now));
                    }
                    MouseScrollDelta::PixelDelta(p) => {
                        let dy = p.y / ch as f64;
                        match phase {
                            TouchPhase::Cancelled => {
                                w.scroll_momentum_active = false;
                                w.scroll_pending = false;
                                w.scroll_velocity = 0.0;
                            }
                            TouchPhase::Ended => {
                                // Gesture lift-off. Apply the final delta (non-zero on
                                // some platforms), then let `about_to_wait` run the coast.
                                if dy != 0.0 {
                                    deliver_scroll(w, dy);
                                }
                                if w.scroll_velocity.abs() > 0.01 {
                                    let now = Instant::now();
                                    w.scroll_decay = TRACKPAD_DECAY;
                                    w.scroll_momentum_active = true;
                                    w.scroll_next_tick = now;
                                    w.scroll_last_tick = now;
                                } else {
                                    w.scroll_momentum_active = false;
                                    w.scroll_velocity = 0.0;
                                }
                            }
                            TouchPhase::Started => {
                                // Carry momentum only when the new gesture is in the
                                // same direction. Opposite-direction gestures reset so
                                // carried velocity can't briefly scroll the wrong way.
                                let carry =
                                    if w.scroll_momentum_active && w.scroll_velocity * dy > 0.0 {
                                        w.scroll_velocity
                                    } else {
                                        0.0
                                    };
                                w.scroll_momentum_active = false;
                                w.scroll_pending = false; // new gesture takes over; any in-flight event is superseded
                                w.scroll_velocity = carry + dy;
                                if dy != 0.0 {
                                    deliver_scroll(w, dy);
                                }
                            }
                            _ => {
                                // Moved: user has direct control; stop any running
                                // momentum and continue the EMA for this gesture.
                                w.scroll_momentum_active = false;
                                w.scroll_velocity = 0.8 * w.scroll_velocity + 0.2 * dy;
                                if dy != 0.0 {
                                    deliver_scroll(w, dy);
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                // Stall trace (BROOD_STALL_MS): the GUI paint runs on this native
                // thread, outside the green-process scheduler — a slow frame (glyph
                // rasterization, softbuffer present, vsync block) is invisible to the
                // "quantum" guard, so it gets its own guard here.
                let _sg = crate::core::heap::stall_guard("gui-paint");
                match &mut w.backend {
                    Backend::Cpu { surface, .. } => {
                        paint(surface, &w.window, &mut w.renderer, &w.frame)
                    }
                    #[cfg(feature = "gui-gpu")]
                    Backend::Gpu(gl) => {
                        gl.paint(&w.frame, &mut w.renderer);
                    }
                }
            }
            _ => {}
        }
    }

    // `about_to_wait` fires after the event queue is fully drained and before
    // winit sleeps. This drives kinetic momentum. IMPORTANT: `about_to_wait` is
    // called on *every* event cycle, including each `UserEvent::Draw` that Brood
    // sends back after rendering a scroll step. Without a time guard this would
    // flood: deliver → Draw → about_to_wait → deliver → Draw → ∞. We gate each
    // delivery on `scroll_next_tick` (set to `now` when momentum starts, then
    // advanced by 12 ms after each step) so at most one step fires per 12 ms
    // regardless of how many times `about_to_wait` is called.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next_wakeup: Option<Instant> = None;
        for w in self.wins.values_mut() {
            if !w.scroll_momentum_active {
                continue;
            }
            // Skip until the minimum interval AND until Brood has finished
            // rendering the previous step. scroll_pending is cleared by the
            // UserEvent::Draw handler when Brood's pipeline completes.
            if now < w.scroll_next_tick || w.scroll_pending {
                next_wakeup = Some(match next_wakeup {
                    Some(t) => t.min(w.scroll_next_tick),
                    None => w.scroll_next_tick,
                });
                continue;
            }
            // Time-proportional decay: `scroll_decay` per 12 ms nominal (a trackpad's
            // long coast or a wheel notch's short glide), scaled by actual elapsed so
            // the curve is independent of render throughput.
            let elapsed_ms = now.duration_since(w.scroll_last_tick).as_secs_f64() * 1000.0;
            let decay = w.scroll_decay.powf((elapsed_ms / 12.0).clamp(0.5, 4.0));
            w.scroll_velocity *= decay;
            if w.scroll_velocity.abs() < 0.0005 {
                w.scroll_momentum_active = false;
                w.scroll_velocity = 0.0;
                continue;
            }
            w.scroll_last_tick = now;
            w.scroll_next_tick = now + Duration::from_millis(12);
            w.scroll_pending = true;
            deliver_scroll(w, w.scroll_velocity);
            next_wakeup = Some(match next_wakeup {
                Some(t) => t.min(w.scroll_next_tick),
                None => w.scroll_next_tick,
            });
        }
        event_loop.set_control_flow(match next_wakeup {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

/// The GUI thread body: build the one event loop, hand its proxy back to
/// `start_thread`, then run winit's loop forever — opening / closing / painting
/// windows from a registry as `UserEvent`s arrive. It never exits (winit can't
/// restart an event loop), so it idles harmlessly when no windows are open.
fn run_gui(ready: Sender<Result<EventLoopProxy<UserEvent>, String>>) {
    // Two hosting modes, decided by `main_thread_hosting_required()`:
    //
    //   * Wayland/X11 — this runs on the dedicated `brood-gui` thread, and
    //     `with_any_thread(true)` is what permits a loop off the main thread.
    //   * everywhere else (macOS, Windows) — this runs ON the process main thread,
    //     because AppKit's run loop may live nowhere else and winit offers no escape
    //     hatch. `with_any_thread` does not exist on those platforms at all, which is
    //     why the call is gated rather than merely unnecessary (KI-125).
    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
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
        defaults: RenderDefaults::default(),
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
    // A new font changes the cell size, hence the (cols, rows) grid — exactly like a
    // window resize. Wake the app loop with the new grid so it RE-RENDERS its content at
    // the new size; without this the redraw below only repaints the stale old frame and
    // the buffer body goes blank until the next unrelated input/timer (the font-switch UI
    // break). The sub-cell remainder is re-placed at paint time (`grid_origin`).
    let (cols, rows) = *w.size.lock().unwrap();
    deliver(w.subscriber, resize_message(cols, rows));
    w.window.request_redraw();
}

/// Recompute `(cols, rows)` from the window's physical size and the cell
/// metrics, and publish it for `gui-size`.
fn update_cells(window: &winit::window::Window, r: &Renderer, size: &Arc<Mutex<(u16, u16)>>) {
    let sz = window.inner_size();
    // The inset eats a margin on each edge, so the usable area is the window
    // minus `2*inset` before it's divided into cells.
    let inset = r.inset();
    let usable_w = (sz.width as usize).saturating_sub(2 * inset);
    let usable_h = (sz.height as usize).saturating_sub(2 * inset);
    let cols = (usable_w / r.cell_w.max(1)).max(1).min(u16::MAX as usize) as u16;
    let rows = (usable_h / r.cell_h.max(1)).max(1).min(u16::MAX as usize) as u16;
    *size.lock().unwrap() = (cols, rows);
}

/// A window pixel position to a (col, row) character cell, clamped to u16. The grid
/// origin (`grid_origin` — inset plus the remainder placement, the same the grid is
/// painted with) is subtracted first, so a click lands on the cell painted under it;
/// a click in the surrounding margin clamps to the edge cell.
fn px_to_cell(pos: PhysicalPosition<f64>, r: &Renderer, w_px: usize, h_px: usize) -> (u16, u16) {
    let (ox, oy) = r.grid_origin(w_px, h_px);
    let col =
        ((pos.x - ox as f64).max(0.0) as usize / r.cell_w.max(1)).min(u16::MAX as usize) as u16;
    let row =
        ((pos.y - oy as f64).max(0.0) as usize / r.cell_h.max(1)).min(u16::MAX as usize) as u16;
    (col, row)
}

/// The shared text engine, behind the single GUI thread's `Rc<RefCell<…>>` (it
/// never leaves that thread). Keeps the `families` field name the windowing code
/// already threads around.
type Families = Rc<RefCell<FontShared>>;

/// sRGB byte (0..=255) → linear-light (0..=1). Built once. A glyph's coverage is
/// a *linear* quantity (fraction of the pixel the stroke covers), so correct
/// anti-aliased compositing has to happen in linear light — which means decoding
/// the stored sRGB colours here first. See [`blend`].
static SRGB_TO_LINEAR: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    let mut t = [0.0f32; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *e = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    t
});

/// How many recent frames' damage we keep — the cap on the buffer `age` we can
/// safely cover with a damage union (typical double/triple buffering is ≤ 3).
const DAMAGE_HISTORY: usize = 8;

/// The furthest a `[:scroll-region …]` may shift its contents, in pixels. Any real
/// framebuffer is thousands of pixels tall, so 16.7 M is "fully scrolled away" many
/// times over in either direction — clamping there changes no frame anyone draws.
///
/// It exists because a frame is ORDINARY BROOD DATA: `dy-frac` is whatever float the
/// app's own arithmetic produced, and every op's top is computed as
/// `oy + row*ch - scroll_dy`. `-1e30` saturates the `as isize` cast to `isize::MIN`,
/// and subtracting *that* overflows — `attempt to subtract with overflow` on the GUI
/// thread under debug-assertions, wrapped (wildly wrong) coordinates without. A GUI
/// thread panic takes the window's event loop with it and no Brood `try` is anywhere
/// on that stack, so this has to be impossible rather than unlikely.
const MAX_SCROLL_PX: isize = 1 << 24;

/// Frames are ORDINARY BROOD DATA an application builds, so every number in one is
/// whatever the app's own arithmetic produced — a scroll offset straight out of a
/// physics step, a rect sized from a division that hit zero. `render_ops` must
/// paint something, or nothing, for any of them and must never panic: it runs on
/// the GUI thread, where a panic takes the window's event loop with it and no
/// Brood `try` is anywhere on the stack to catch it.
///
/// These run the REAL renderer, which `BROOD_GUI_HEADLESS=1` cannot: headless makes
/// every draw op a silent no-op, so an in-language headless GUI test proves exactly
/// nothing about this code. Hence a Rust test that calls `render_ops` directly.
#[cfg(test)]
mod main_thread_hosting {
    use super::hosting_required_from;

    /// macOS/Windows: the loop has one legal home and the lever must not be able to
    /// move it. A `=0` that *was* honoured would not read as a config mistake — the GUI
    /// would simply fail to start, on the platform with no second path to fall back to.
    #[test]
    fn a_platform_with_nowhere_else_always_hosts_on_main() {
        for lever in [None, Some("0"), Some(""), Some("1")] {
            assert!(
                hosting_required_from(false, lever),
                "lever {lever:?} must not move the loop off the main thread"
            );
        }
    }

    /// Wayland/X11 keep the dedicated `brood-gui` thread by default — this is the path
    /// that has always worked and the one every existing GUI app runs on.
    #[test]
    fn wayland_and_x11_keep_the_dedicated_thread_by_default() {
        assert!(!hosting_required_from(true, None));
        assert!(!hosting_required_from(true, Some("0")));
        assert!(!hosting_required_from(true, Some("")));
    }

    /// ...but can opt in, which is the only reason the main-thread path is runnable
    /// anywhere this project actually tests. Without this, the macOS code would ship
    /// having been compiled and never executed.
    #[test]
    fn the_lever_opts_a_permissive_platform_into_the_main_thread_path() {
        assert!(hosting_required_from(true, Some("1")));
        assert!(hosting_required_from(true, Some("yes")));
    }
}
