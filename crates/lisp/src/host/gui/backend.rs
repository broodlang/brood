//! The windowed backend (feature `gui`): the winit event loop on its own thread (or the
//! process main thread where the platform demands it — ADR-324), the window registry
//! the `%gui-*` primitives address by id, and the messages a window delivers to its
//! subscribing process. Input translation, text rendering and frame painting are the
//! child modules.

mod bleed;
#[cfg(all(test, feature = "gui-gpu"))]
mod gpu_replay;
mod input;
mod keyboard;
mod paint;
mod render;

use input::*;
use keyboard::*;
use paint::*;
pub(crate) use render::Renderer;
pub use render::TextAa;
use render::*;
// The GPU render target (`gpu.rs`) paints with the same text metric and hairline snap.
#[cfg(feature = "gui-gpu")]
pub(crate) use {
    paint::holds_blinking_caret, paint::paint_stall_ms, paint::text_px_width, render::caret_cells,
    render::caret_glyph, render::cursor_rim, render::luma, render::snap_hairline,
    render::text_units, render::unit_cells,
};

use super::{
    CursorShape, Key, KeyRepeat, Keystroke, LatencySample, Mouse, MouseAction, MouseButton, Op,
    OptionMeta, PAINT_PHASES, RepeatCounts, WheelMode, WindowSpec,
};

use crate::core::sync::lock;
use crate::core::value;

// Everything a window delivers is the user's input, so all of it rides the mailbox's
// PRIORITY lane (ADR-490): a UI loop takes it ahead of any background backlog.
use crate::process::{
    Message, deliver_priority as deliver, deliver_priority_credited as deliver_credited,
};

use std::cell::RefCell;

use std::collections::{HashMap, VecDeque};

use std::num::NonZeroU32;

use std::rc::Rc;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use std::sync::mpsc::{self, Sender};

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

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
    Attrs, Buffer as CtBuffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    SwashContent, Weight, fontdb,
};

use unicode_segmentation::UnicodeSegmentation;

// A RAW tab in a text op advances to the next SCREEN stop of the fixed `TAB_WIDTH`, as a
// terminal's does: the painter has no line column and no Brood heap, so it cannot read a
// program's `*tab-width*` (ADR-537). A program that lays tabs out at its own width expands
// them before it paints (`string/expand-tabs`, which does read it) — bedit's view does, so
// no raw tab reaches this code from it.
use crate::host::text_width::{TAB_WIDTH, cluster_cells, cluster_cells_at};

/// The winit cursor for a frontend-neutral `CursorShape`.
fn cursor_icon(shape: CursorShape) -> CursorIcon {
    match shape {
        CursorShape::ColResize => CursorIcon::EwResize, // ↔ side-by-side divider
        CursorShape::RowResize => CursorIcon::NsResize, // ↕ stacked divider
        CursorShape::Pointer => CursorIcon::Pointer,    // 👆 a clickable link
    }
}

/// The cursor shape for the pointer at grid-relative pixel `(x, y)`, given the
/// window's zones — the first zone containing the point, or None (default cursor).
fn shape_at(zones: &[Zone], x: isize, y: isize) -> Option<CursorShape> {
    zones
        .iter()
        .find_map(|zone| zone.contains(x, y).then_some(zone.shape))
}

// Bundled monospace font, four styles (see assets/README.md) — the default
// `:mono` family; a face's :bold/:italic pick the style. Installed fonts are found
// by name only when asked for (`gui-font-register` with `:family-name`, ADR-442).
const FONT_REGULAR: &[u8] = include_bytes!("../../../assets/DejaVuSansMono.ttf");

const FONT_BOLD: &[u8] = include_bytes!("../../../assets/DejaVuSansMono-Bold.ttf");

const FONT_ITALIC: &[u8] = include_bytes!("../../../assets/DejaVuSansMono-Oblique.ttf");

const FONT_BOLD_ITALIC: &[u8] = include_bytes!("../../../assets/DejaVuSansMono-BoldOblique.ttf");

// Bundled color emoji font (CBDT), loaded only as a *fallback*: a cluster the mono
// font can't cover (an emoji, a flag, …) is shaped + rasterised from
// here by cosmic-text/swash, in color. Not a selectable `:family`. ~11 MB.
const FONT_EMOJI: &[u8] = include_bytes!("../../../assets/NotoColorEmoji.ttf");

// The family name fontdb assigns the bundled mono faces — what we pass as the
// primary `Attrs` family; cosmic-text's fallback list then reaches the emoji font.
const MONO_FAMILY: &str = "DejaVu Sans Mono";

// The family name fontdb assigns the bundled emoji font — a bundled face a missing
// glyph is looked for in before any system font (`FontShared::fallback_family`).
const EMOJI_FAMILY: &str = "Noto Color Emoji";

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
/// Lines one wheel notch scrolls under `{:wheel :lines}` — the distance a glide covers
/// too, so switching modes changes how a notch moves, not how far.
const WHEEL_LINES: f64 = 3.0;
/// How long a `{:wheel :lines}` scroll waits for the app to draw the last one before the
/// backlog goes anyway — a raw loop that never draws must not lose its wheel.
const WHEEL_HOLD: Duration = Duration::from_millis(100);

/// How often a window asks whether its app's loop is busy, from an input until the loop is
/// idle again (ADR-491): the busy cursor appears at most this long after the loop's quit
/// key would interrupt it, and goes as soon after the turn ends.
const BUSY_PROBE: Duration = Duration::from_millis(100);

/// The busy cursor (ADR-491): while the loop that takes this window's input has been in one
/// `update` or `view` past its quit key's busy-ms, show the wait cursor — the C-g that would
/// stop it is live — and put the hover shape back when the turn ends. Asked only from an
/// input until the loop is idle again (`process::interrupt_busy` answers None), so it costs
/// a window nothing at rest. When to ask next, if at all.
fn busy_cursor(w: &mut Win, now: Instant) -> Option<Instant> {
    let due = w.busy_check?;
    if now < due {
        return Some(due);
    }
    let busy = crate::process::interrupt_busy(w.subscriber);
    w.busy_local = busy == Some(true);
    show_busy(w);
    // idle (no window open): stop asking until the next input; a busy cursor showing is
    // never left behind, since that answer also cleared it above
    w.busy_check = busy.map(|_| now + BUSY_PROBE);
    w.busy_check
}

/// Show the wait cursor while the loop is busy — the one this window asks (`busy_local`) or
/// a thin client's daemon, which says so over the link (`busy_remote`) — and the hover
/// shape otherwise. Only a change calls `set_cursor`.
fn show_busy(w: &mut Win) {
    let show = w.busy_local || w.busy_remote;
    if show != w.busy_shown {
        w.busy_shown = show;
        w.window.set_cursor(if show {
            CursorIcon::Wait
        } else {
            w.shape.map(cursor_icon).unwrap_or(CursorIcon::Default)
        });
    }
}

/// Deliver a `{:wheel :lines}` window's wheel backlog as ONE scroll message, unless the
/// last one is still undrawn (and younger than `WHEEL_HOLD`) — then it waits and keeps
/// summing. True when nothing is left waiting.
fn flush_wheel(w: &mut Win, now: Instant) -> bool {
    if w.wheel_backlog == 0.0 {
        return true;
    }
    let undrawn = w.scroll_pending
        && w.wheel_sent
            .is_some_and(|sent| now.duration_since(sent) < WHEEL_HOLD);
    if undrawn {
        return false;
    }
    deliver_scroll(w, w.wheel_backlog);
    w.wheel_backlog = 0.0;
    w.scroll_pending = true;
    w.wheel_sent = Some(now);
    true
}

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
pub(super) const CURSOR_FG: [u8; 3] = [0xf5, 0xf5, 0xf5];

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
    /// Window `id`'s app finished a turn with nothing new to draw (ADR-444): what a Draw
    /// would have told the window about the app's progress, without the frame.
    Settle { id: u64 },
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
    /// Upload a texture for window `id`'s `:sprite` ops: `rgba` is `w*h*4` bytes,
    /// row-major straight alpha, and `tex` the handle the Brood side already returned
    /// for it (allocated there from a counter, so `gui-texture` needs no round trip).
    /// The GPU target keeps it on the device; the CPU target has no sprite arm and
    /// drops it. Behind `gui-texture`.
    Texture {
        id: u64,
        tex: u32,
        rgba: Vec<u8>,
        w: u32,
        h: u32,
    },
    /// Release texture `tex` of window `id` (a later `:sprite` naming it draws nothing).
    /// Behind `gui-texture-free`.
    TextureFree { id: u64, tex: u32 },
    /// Switch window `id`'s mouse input between cells and PIXELS at runtime (the
    /// `{:input :pixels}` open option, after the fact). The window republishes its size
    /// in the new unit so the app re-lays itself out. Behind `gui-input!` — how a window a
    /// generic thin client opened is told, by the app it attached to, which unit it wants.
    InputMode { id: u64, pixels: bool },
    /// Keep a decoded sound for window `id`'s `[:sound …]` ops under handle `snd`
    /// (allocated on the Brood side, like a texture handle). Behind `gui-sound`.
    Sound {
        id: u64,
        snd: u32,
        data: std::sync::Arc<crate::host::audio::SoundData>,
    },
    /// Forget sound `snd` of window `id`. Behind `gui-sound-free`.
    SoundFree { id: u64, snd: u32 },
    /// Raise window `id` to the front and give it OS keyboard focus (un-
    /// minimising it first). Behind `gui-focus` — surfaces an already-open
    /// singleton window instead of opening a duplicate.
    Focus { id: u64 },
    /// Confine the pointer to window `id` (`on`) or release it. Behind
    /// `gui-grab-cursor` — keeps the cursor inside the window for mouse-look so
    /// it can't slip out and click another app.
    Grab { id: u64, on: bool },
    /// The loop this window's input feeds is busy elsewhere (`on`) or no longer — a thin
    /// client's daemon, which says so over the link (ADR-491). Behind `%gui-busy`.
    Busy { id: u64, on: bool },
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
    /// Measure the cell metrics font size `px` would produce in window `id` (`None` = the
    /// size the window is already using) and reply with `(cell_w, cell_h)` in PHYSICAL px.
    /// Answered on the GUI thread because only it holds the renderer and the font engine;
    /// memoised there. An app laying out a `CellRegion` asks this to learn how many of that
    /// region's cells fit the rect it has — per-buffer zoom's one unavoidable question.
    Metrics {
        id: u64,
        px: Option<f32>,
        reply: Sender<Result<(u16, u16), String>>,
    },
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
        faces: Box<[Vec<u8>; 4]>,
        paths: [String; 4],
    },
    /// Register a font family (interned `name`) by an INSTALLED family name, found in
    /// the system font database (scanned on first use) with each style picked by weight
    /// and slant. Behind `gui-font-register` with `:family-name` (ADR-442).
    RegisterInstalledFamily { name: u32, family_name: String },
    /// Report where family `family` (`None` = the global default) got its faces, and
    /// the fallback fonts loaded so far. Behind `gui-font-info`.
    FontInfo {
        family: Option<u32>,
        reply: Sender<super::FontInfo>,
    },
    /// Shape runs of punctuation as one unit so a font's ligatures form — for every
    /// open window and ones opened later. Behind `gui-ligatures!`. A pure repaint.
    Ligatures { on: bool },
}

/// A freshly opened window's wiring, handed back to the Brood side: its id and
/// the shared cell size the GUI thread keeps current. Input is *not* polled — the
/// GUI thread delivers it straight to the subscriber's mailbox (ADR-059).
struct OpenReply {
    id: u64,
    size: Arc<Mutex<(u16, u16)>>,
    size_px: Arc<Mutex<(u32, u32)>>,
    keys: Arc<Mutex<KeyReport>>,
    latency: Arc<Mutex<VecDeque<LatencySample>>>,
}

/// What the Brood side keeps per open window (keyed by the id `open` returns) —
/// just the shared cell size for `gui-size`. Input arrives as mailbox messages,
/// so there is no receiver to keep here (ADR-059).
struct WinHandle {
    size: Arc<Mutex<(u16, u16)>>,
    /// The window's inner size in PHYSICAL pixels, republished on every resize, for
    /// `gui-size-px` — the frame a pixel-space app lays itself out in.
    size_px: Arc<Mutex<(u32, u32)>>,
    /// What the window's keyboard reports (`Keyboard::report`): the keystroke held down,
    /// for `gui-held-key` (the source of truth for a held key — immune to a missed key-up,
    /// ADR-086), and how its repeats went, for `gui-key-repeats`.
    keys: Arc<Mutex<KeyReport>>,
    /// The last `LATENCY_SAMPLES` input-to-present samples, newest last, for
    /// `gui-latency` (ADR-440).
    latency: Arc<Mutex<VecDeque<LatencySample>>>,
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
/// can be tested / soak-run / CI'd with no popup. Read once at first use. Public as
/// `gui/headless?` (ADR-557): a question with no font engine behind it — `gui-font-info`,
/// `gui-cell-size` — errors headless, so an app asks this before asking it.
pub fn headless() -> bool {
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

/// The pixel size a headless window reports for a requested logical `size` (default
/// 840×560), at a nominal scale of 1.
fn headless_px(size: Option<(f64, f64)>) -> (u32, u32) {
    let (w, h) = size.unwrap_or((840.0, 560.0));
    ((w.max(1.0)) as u32, (h.max(1.0)) as u32)
}

/// Can the event loop live on a thread we choose, or must it own the **process main
/// thread**?
///
/// Wayland/X11 let a loop run anywhere, so there the GUI is a dedicated `brood-gui`
/// thread. macOS does not: AppKit's run loop is main-thread-only and winit exposes no
/// `with_any_thread` there (KI-125). Windows is the same shape. Anything not
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

const MAIN_THREAD_UNRESERVED: &str = "gui: this platform requires the event loop on the process main thread, and this \
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
    *lock(main_slot()) = Some(tx.clone());

    let result: Arc<Mutex<Option<std::thread::Result<T>>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&result);
    let joined = std::thread::Builder::new()
        .name("brood-main-join".into())
        .spawn(move || {
            let r = handle.join();
            *lock(&sink) = Some(r);
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
        .unwrap_or_else(PoisonError::into_inner)
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
            .unwrap_or_else(PoisonError::into_inner)
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
        lock(windows()).insert(
            id,
            WinHandle {
                size: Arc::new(Mutex::new(headless_cells(spec.size))),
                size_px: Arc::new(Mutex::new(headless_px(spec.size))),
                keys: Arc::default(),
                latency: Arc::default(),
            },
        );
        return Ok(id);
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    // Send under the proxy lock, then drop it before awaiting the reply so a
    // slow window build can't block other windows' sends.
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::Open {
            subscriber,
            spec,
            reply: reply_tx,
        })
        .map_err(|_| "gui thread is gone".to_string())?;
    let OpenReply {
        id,
        size,
        size_px,
        keys,
        latency,
    } = reply_rx
        .recv()
        .map_err(|_| "gui thread did not reply".to_string())??;
    lock(windows()).insert(
        id,
        WinHandle {
            size,
            size_px,
            keys,
            latency,
        },
    );
    Ok(id)
}

/// `(gui-close id)` — destroy window `id` (idempotent; unknown id is a no-op).
pub fn close(id: u64) -> Result<(), String> {
    lock(windows()).remove(&id);
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = lock(g).send_event(UserEvent::Close { id });
    }
    Ok(())
}

/// `(gui-focus id)` — raise window `id` and request OS keyboard focus (un-
/// minimising it). The window lives on the GUI thread, so this routes the
/// request through the event-loop proxy like `close`/`draw`; the actual
/// `focus_window` runs there. Errors only if the id isn't a live window.
pub fn focus(id: u64) -> Result<(), String> {
    dispatch(id, UserEvent::Focus { id })
}

/// Send `event` for window `id` to the GUI thread — the shape every window op here
/// shares: an error when `id` is not a live window, a no-op headless.
fn dispatch(id: u64, event: UserEvent) -> Result<(), String> {
    if !lock(windows()).contains_key(&id) {
        return Err("gui window not open".into());
    }
    if headless() {
        return Ok(());
    }
    lock(gui()?)
        .send_event(event)
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-grab-cursor id on)` — confine the pointer to window `id` (`on` true) or
/// release it. Dispatched to the GUI thread like `focus`.
pub fn grab(id: u64, on: bool) -> Result<(), String> {
    dispatch(id, UserEvent::Grab { id, on })
}

/// `(%gui-busy id on)` — show window `id`'s busy cursor because the loop it feeds is busy
/// somewhere this window cannot ask (a served session's daemon), or stop. Dispatched like
/// `grab`.
pub fn busy(id: u64, on: bool) -> Result<(), String> {
    dispatch(id, UserEvent::Busy { id, on })
}

/// `(gui-maximize! id on)` — maximise window `id` (`on` true) or restore it,
/// keeping the title bar. Dispatched to the GUI thread like `grab`.
pub fn maximize(id: u64, on: bool) -> Result<(), String> {
    dispatch(id, UserEvent::Maximize { id, on })
}

/// `(gui-minimize! id)` — iconify window `id`. Dispatched like `maximize`.
pub fn minimize(id: u64) -> Result<(), String> {
    dispatch(id, UserEvent::Minimize { id })
}

/// `(gui-drag-move id)` — start an interactive window move.
pub fn drag_move(id: u64) -> Result<(), String> {
    dispatch(id, UserEvent::DragMove { id })
}

/// `(gui-drag-resize id dir)` — start an interactive resize from edge/corner `dir`.
pub fn drag_resize(id: u64, dir: &str) -> Result<(), String> {
    let dir = dir.to_string();
    dispatch(id, UserEvent::DragResize { id, dir })
}

/// `(gui-fullscreen! id on)` — make window `id` borderless-fullscreen (`on`
/// true) or restore it. Dispatched to the GUI thread like `grab`.
pub fn fullscreen(id: u64, on: bool) -> Result<(), String> {
    dispatch(id, UserEvent::Fullscreen { id, on })
}

/// `(gui-size id)` — window `id`'s size in character cells.
pub fn size(id: u64) -> Result<(u16, u16), String> {
    let w = lock(windows());
    let h = w.get(&id).ok_or("gui window not open")?;
    let size = *lock(&h.size);
    Ok(size)
}

/// `(gui-cell-size id [px])` — the cell metrics of window `id` in PHYSICAL pixels:
/// `(cell_w, cell_h)` for the size the window is using, or for the font size `px`
/// (logical) a `cell-region` would paint with.
///
/// The question an app has to ask before it can lay out a region at another size: how
/// many of THAT size's cells fit the rect it has, in this window's cells. It cannot be
/// derived from the px alone — a cell is whatever shaping the reference glyph produces,
/// rounded to whole pixels — so it is measured, on the GUI thread, and memoised there.
///
/// Headless (and with no GUI thread) there is no font engine, so this fails rather than
/// inventing a number; a caller that can run headless falls back to the size ratio.
pub fn cell_size(id: u64, px: Option<f32>) -> Result<(u16, u16), String> {
    {
        let w = lock(windows());
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    if headless() {
        return Err("gui is headless — no font engine to measure a cell with".into());
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::Metrics {
            id,
            px,
            reply: reply_tx,
        })
        .map_err(|_| "gui thread is gone".to_string())?;
    reply_rx
        .recv()
        .map_err(|_| "gui thread did not reply".to_string())?
}

/// `(gui-held-key id)` — the key window `id` currently sees as physically held,
/// or `None` when none is. Read from the shared state the event loop keeps current
/// from press/release transitions (not winit's unreliable `ke.repeat`), so an app
/// can confirm a key is still down before repeating — the source of truth that
/// makes a missed key-up unable to cause runaway repeat (ADR-086).
pub fn held_key(id: u64) -> Result<Option<Keystroke>, String> {
    let w = lock(windows());
    let h = w.get(&id).ok_or("gui window not open")?;
    let held = lock(&h.keys).held.clone();
    Ok(held)
}

/// `(gui-key-repeats id)` — how window `id`'s held keys have repeated since it opened
/// (`RepeatCounts`): repeats delivered and dropped, and the desktop's delay and interval
/// as the last key that repeated showed them. All zero headless.
pub fn key_repeats(id: u64) -> Result<RepeatCounts, String> {
    let w = lock(windows());
    let h = w.get(&id).ok_or("gui window not open")?;
    let repeats = lock(&h.keys).repeats;
    Ok(repeats)
}

/// `(gui-latency id)` — window `id`'s recent input-to-present samples, oldest first
/// (at most `LATENCY_SAMPLES`; empty headless, where nothing is painted). ADR-440.
pub fn latency(id: u64) -> Result<Vec<LatencySample>, String> {
    let w = lock(windows());
    let h = w.get(&id).ok_or("gui window not open")?;
    let samples = lock(&h.latency).iter().copied().collect();
    Ok(samples)
}

/// `(gui-draw id ops)` — paint a frame to window `id`.
pub fn draw(id: u64, ops: Vec<Op>) -> Result<(), String> {
    {
        let w = lock(windows());
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::Draw { id, ops })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-settle id)` — window `id`'s app finished a turn with nothing to draw (ADR-444).
pub fn settle(id: u64) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::Settle { id })
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
            .unwrap_or_else(PoisonError::into_inner)
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
        let _ = lock(g).send_event(UserEvent::Inset { px });
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
        let _ = lock(g).send_event(UserEvent::Background { rgb });
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
        let _ = lock(g).send_event(UserEvent::LineHeight { mult });
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
        let _ = lock(g).send_event(UserEvent::TextAa { mode });
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
            .unwrap_or_else(PoisonError::into_inner)
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
        let _ = lock(g).send_event(UserEvent::Title { id, title });
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
            .unwrap_or_else(PoisonError::into_inner)
            .send_event(UserEvent::Icon { id, rgba, w, h });
    }
    Ok(())
}

/// `(gui-texture id tex rgba w h)` — upload `rgba` (`w*h*4` bytes) as texture `tex` of
/// window `id`, for its `:sprite` ops. The handle was allocated by the caller
/// (`next_texture_id`), so this is fire-and-forget like `icon`: no reply to wait for.
pub fn texture(id: u64, tex: u32, rgba: Vec<u8>, w: u32, h: u32) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = lock(g).send_event(UserEvent::Texture {
            id,
            tex,
            rgba,
            w,
            h,
        });
    }
    Ok(())
}

/// `(gui-texture-free id tex)` — release texture `tex` of window `id`.
pub fn texture_free(id: u64, tex: u32) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send_event(UserEvent::TextureFree { id, tex });
    }
    Ok(())
}

/// `(gui-sound id snd rate channels pcm)` — keep a PCM16 sound under handle `snd` for
/// window `id`'s `[:sound …]` ops. Fire-and-forget like `texture`.
pub fn sound(id: u64, snd: u32, rate: u32, channels: u16, pcm: Vec<u8>) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    let data = std::sync::Arc::new(crate::host::audio::SoundData::from_pcm16(
        rate, channels, &pcm,
    ));
    if let Ok(g) = gui() {
        let _ = g
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send_event(UserEvent::Sound { id, snd, data });
    }
    Ok(())
}

/// Make window `w`'s track what the frame `ops` asks for: its first `[:music …]` op's
/// sound, at that op's volume — the one playing kept (only its volume set) when the handle
/// is the same, replaced when it differs, stopped when the frame has none or names a sound
/// this window does not hold.
fn reconcile_music(w: &mut Win, ops: &[Op]) {
    let wanted = ops.iter().find_map(|op| match op {
        Op::Music { id, vol } => Some((*id, *vol)),
        _ => None,
    });
    let Some((snd, vol)) = wanted.filter(|(snd, _)| w.sounds.contains_key(snd)) else {
        w.music = None;
        return;
    };
    match &w.music {
        Some((playing, music)) if *playing == snd => music.set_volume(vol),
        _ => {
            w.music = None;
            w.music = crate::host::audio::music(&w.sounds[&snd], vol).map(|music| (snd, music));
        }
    }
}

/// A window's blinking caret (ADR-469): the `Op::Cursor` that asked to blink, how, and
/// when its current run of blinks began. `steady` holds it shown — a window without
/// focus does not blink, as Emacs's unselected frames do not.
struct CaretBlink {
    caret: Op,
    spec: crate::host::gui::CursorBlink,
    since: Instant,
    steady: bool,
}

impl CaretBlink {
    /// The clock for `caret`, starting now, shown; `None` for an op that does not blink.
    fn of(caret: &Op) -> Option<CaretBlink> {
        match caret {
            Op::Cursor {
                blink: Some(spec), ..
            } => Some(CaretBlink {
                caret: caret.clone(),
                spec: *spec,
                since: Instant::now(),
                steady: false,
            }),
            _ => None,
        }
    }

    /// Whether the caret is in its hidden half at `now`.
    fn hidden(&self, now: Instant) -> bool {
        !self.steady
            && self
                .spec
                .hidden_after(now.saturating_duration_since(self.since))
    }

    /// When it next changes visibility after `now`, or `None` while it is held still or
    /// has blinked its `blinks`.
    fn next_toggle(&self, now: Instant) -> Option<Instant> {
        if self.steady {
            return None;
        }
        self.spec.next_toggle(self.since, now)
    }
}

/// The first blinking caret of `ops`, walking into the regions and fragments (where a
/// pane's caret is).
fn blinking_caret(ops: &[Op]) -> Option<&Op> {
    ops.iter().find_map(|op| match op {
        Op::Cursor { blink: Some(_), .. } => Some(op),
        Op::ScrollRegion { ops, .. } | Op::CellRegion { ops, .. } => blinking_caret(ops),
        Op::Fragment(fragment) => blinking_caret(&fragment.0),
        _ => None,
    })
}

/// Show `w`'s blinking caret now and restart its blinks — `blinking` false holds it
/// shown until the next wake (focus lost). A caret that was hidden is repainted.
fn wake_caret(w: &mut Win, blinking: bool) {
    if let Some(clock) = &mut w.caret_blink {
        if w.renderer.caret_hidden {
            w.window.request_redraw();
        }
        clock.since = Instant::now();
        clock.steady = !blinking;
    }
}

/// `(gui-sound-free id snd)` — forget sound `snd` of window `id`.
pub fn sound_free(id: u64, snd: u32) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send_event(UserEvent::SoundFree { id, snd });
    }
    Ok(())
}

/// `(gui-input! id mode)` — deliver window `id`'s mouse input in pixels (`pixels`) or
/// cells from now on; the window then reports its size in that unit.
pub fn input_mode(id: u64, pixels: bool) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = g
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send_event(UserEvent::InputMode { id, pixels });
    }
    Ok(())
}

/// The next texture handle. Process-wide rather than per window so a handle can never
/// name a different texture in another window by accident; starts at 1 so 0 is never a
/// live texture (a `:sprite` naming it draws nothing).
pub fn next_texture_id() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// `(gui-size-px id)` — window `id`'s inner size in PHYSICAL pixels.
pub fn size_px(id: u64) -> Result<(u32, u32), String> {
    let w = lock(windows());
    let h = w.get(&id).ok_or("gui window not open")?;
    let size = *lock(&h.size_px);
    Ok(size)
}

/// `(gui-font-register …)` — register a font family (interned `name`) from raw
/// TTF bytes per style (regular, bold, italic, bold-italic), read from `paths`; the GUI
/// thread parses + shares it so `:family` can pick it. Starts the GUI thread if needed
/// (so a family can be registered up front).
pub fn register_family(name: u32, faces: [Vec<u8>; 4], paths: [String; 4]) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::RegisterFamily {
            name,
            faces: Box::new(faces),
            paths,
        })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-font-register name {:family-name "…"})` — register family `name` by an
/// installed font's family name. Resolved on the GUI thread, which scans the system
/// fonts the first time it is asked; `font_info` reports what it found. Starts the GUI
/// thread if needed, like `register_family`.
pub fn register_installed_family(name: u32, family_name: String) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::RegisterInstalledFamily { name, family_name })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-font-info [family])` — where family `family` (`None` = the global default)
/// got its faces, and the fallback fonts loaded so far. Answered on the GUI thread,
/// after every registration sent before it (the proxy is FIFO). Headless there is no
/// font engine, so this fails, as `cell_size` does.
pub fn font_info(family: Option<u32>) -> Result<super::FontInfo, String> {
    if headless() {
        return Err("gui is headless — no font engine to ask".into());
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::FontInfo {
            family,
            reply: reply_tx,
        })
        .map_err(|_| "gui thread is gone".to_string())?;
    reply_rx
        .recv()
        .map_err(|_| "gui thread did not reply".to_string())
}

/// `(gui-ligatures! on)` — shape punctuation runs as one unit (programming ligatures)
/// on every window + the default for ones opened later. No-op (silently) if the GUI
/// thread never started.
pub fn ligatures(on: bool) -> Result<(), String> {
    if headless() {
        return Ok(());
    }
    if let Ok(g) = gui() {
        let _ = lock(g).send_event(UserEvent::Ligatures { on });
    }
    Ok(())
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
    // Boxed: `GpuWindow` is large (device, queue, pipelines, glyph cache), which
    // would bloat every `Backend` — including the common `Cpu` one — to that
    // size. The box keeps the enum a couple of words; the GPU path is cold.
    #[cfg(feature = "gui-gpu")]
    Gpu(Box<crate::host::gui::gpu::GpuWindow>),
}

/// Whether a `gui-gpu` build draws on the GPU: yes unless `BROOD_GUI_GPU=0` asks for the
/// CPU painter (the A/B lever, and the escape hatch on a box whose GPU driver misbehaves).
/// It draws every op the CPU painter does, so the build flag alone selects it. A GPU
/// whose device cannot be created falls back to the CPU painter.
#[cfg(feature = "gui-gpu")]
fn gpu_enabled() -> bool {
    std::env::var("BROOD_GUI_GPU")
        .map(|v| v != "0")
        .unwrap_or(true)
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
    /// The inner size in physical pixels, shared with the Brood side for `gui-size-px`.
    size_px: Arc<Mutex<(u32, u32)>>,
    /// The process this window's input is delivered to (its mailbox).
    subscriber: u64,
    /// Mouse input in pixels rather than cells (`WindowSpec::pixel_input`).
    pixel_input: bool,
    /// The sounds uploaded for this window's `[:sound …]` ops, by handle.
    sounds: HashMap<u32, std::sync::Arc<crate::host::audio::SoundData>>,
    /// The track the frames on screen keep looping (`[:music …]`): its handle and the
    /// playing loop, which fades out when dropped — with the window, or by a frame
    /// without it.
    music: Option<(u32, crate::host::audio::Music)>,
    frame: Vec<Op>,
    /// `frame` with its edge backgrounds continued over the margin (`bleed`), built once
    /// per frame and window geometry, so a repaint (a caret blink, an expose) reuses it.
    bleed: bleed::Bleed,
    mods: ModifiersState,
    /// Which Option keys the OS reports held, `(left, right)` — only macOS reports them —
    /// and which of them is Meta (`WindowSpec::option_meta`, ADR-541): `chord_modifiers`.
    option_sides: (bool, bool),
    option_meta: OptionMeta,
    /// The active layout types a non-Latin script, as its last letter key said
    /// (`layout_is_non_latin`): its chords are read by US position (ADR-541).
    non_latin_layout: bool,
    cursor: (u16, u16),
    /// The pointer's last physical pixel position `(x, y)` — what a pixel-input
    /// window reports in place of the cell, and the position a press/release carries.
    cursor_px: (u16, u16),
    /// The pointer in FRACTIONAL window cells `(row, col)`, measured from the same grid origin
    /// and cell as `cursor` — what a mouse message carries as `{:at [row col]}` so a zoomed
    /// region can hit-test inside a window cell (`Mouse::at`).
    cursor_at: (f64, f64),
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
    /// The held key, every key down, and the gate on the OS repeat (`Keyboard`).
    keyboard: Keyboard,
    /// Cursor hot-zones from the last drawn frame (`Op::CursorZone`), flattened to
    /// grid-relative PIXEL rects + the shape to show while the pointer is inside —
    /// pixels because a zone inside a `CellRegion` is measured in that region's cells,
    /// not the window's. Hit-tested on `CursorMoved`. (ADR-080, ADR-363.)
    zones: Vec<Zone>,
    /// The shape currently applied to the window, so we only call `set_cursor`
    /// when the hit-test result changes (not on every pointer move).
    shape: Option<CursorShape>,
    /// When to ask next whether the app's loop is busy (ADR-491): armed by input, polled
    /// every [`BUSY_PROBE`] while the loop is inside an `update` or `view`, and dropped once
    /// it is idle again — so an idle window never wakes for it.
    busy_check: Option<Instant>,
    /// Whether the busy (wait) cursor is showing, because the loop has been in one turn
    /// past its quit key's busy-ms.
    busy_shown: bool,
    /// The loop this window asks has been in one turn past its busy-ms (`busy_cursor`).
    busy_local: bool,
    /// A thin client's daemon said its loop is busy (`%gui-busy`, ADR-491): the loop this
    /// window feeds runs on another node, where `busy_cursor` cannot ask.
    busy_remote: bool,
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
    /// When to repaint next for the particles in flight (`[:emitter …]`): they move between
    /// frames, so the window repaints on its own, about 60 times a second, while any live.
    particles_next: Option<Instant>,
    /// Time the previous momentum step was delivered, used for time-proportional decay
    /// (`0.97 ^ (elapsed_ms / 12)`). Keeps the physics correct when about_to_wait
    /// fires late so the coast speed is render-throughput independent.
    scroll_last_tick: Instant,
    /// `true` while a momentum scroll event is in-flight — i.e. delivered to Brood
    /// but the corresponding UserEvent::Draw has not yet arrived. A new step is never
    /// delivered while this is set, so the queue never grows beyond one pending event
    /// even when Brood's render pipeline is slower than the 12 ms tick rate.
    scroll_pending: bool,
    /// Whether an input method may compose in this window (`WindowSpec::ime`), where its
    /// composition stands (`Composition`), and the cursor rect last handed to it for the
    /// candidate window, so it is only told when the cursor moves.
    ime: bool,
    composition: Composition,
    ime_area: Option<(isize, isize, usize, usize)>,
    /// What a wheel notch does (`WindowSpec::wheel`). Under `WheelMode::Lines`, the lines
    /// of notches that arrived while the last scroll message was not yet drawn, summed
    /// into the next one, and when that last one went (a window that never draws is not
    /// held up past `WHEEL_HOLD`).
    wheel: WheelMode,
    wheel_backlog: f64,
    wheel_sent: Option<Instant>,
    /// Whether the window has been made visible. It is built hidden and shown once its
    /// first real frame (one the app drew) has been painted, or after `SHOW_TIMEOUT`
    /// with whatever it has — so opening never flashes an empty background, and a
    /// window whose app never draws still appears (ADR-440).
    shown: bool,
    /// Whether the app has drawn a frame yet. Until it has, a redraw paints nothing:
    /// the only thing to paint would be the bare background.
    drawn: bool,
    /// When the window was built, for `SHOW_TIMEOUT`.
    opened_at: Instant,
    /// When the earliest input not yet answered by a frame arrived (a key press, a
    /// button, a wheel step) — consumed by the next `UserEvent::Draw` (ADR-440).
    input_at: Option<Instant>,
    /// The input stamp the frame waiting to be painted answers, carried to the paint so
    /// it can report input -> present.
    draw_input: Option<Instant>,
    /// The last `LATENCY_SAMPLES` input-to-present samples, shared with `gui-latency`.
    latency: Arc<Mutex<VecDeque<LatencySample>>>,
    /// The frame's blinking caret and its clock (ADR-469), `None` when the frame on screen
    /// has no `[:cursor … blink]`: the window toggles it itself, between frames.
    caret_blink: Option<CaretBlink>,
}

/// How long a window waits, hidden, for its app's first frame before it is shown
/// anyway (ADR-440). Long enough that a Brood app's first draw normally lands inside
/// it; short enough that a slow one still gets a window promptly.
const SHOW_TIMEOUT: Duration = Duration::from_millis(500);

/// How many input-to-present samples a window keeps for `gui-latency`.
const LATENCY_SAMPLES: usize = 64;

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
    /// Whether punctuation runs are shaped together (programming ligatures).
    ligatures: bool,
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
            ligatures: false,
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
        .with_inner_size(LogicalSize::new(w, h))
        // Hidden until its first real frame is painted (`Win::shown`), so the desktop
        // never maps an empty window that a frame then replaces. Wayland ignores the
        // flag, but maps a surface only at its first commit — which the paint gating
        // in `RedrawRequested` makes the first real frame too.
        .with_visible(false);
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
    // Which Option key is Meta (ADR-541): winit then reads that side's keys without their
    // Option layer — left-Option+e is `e` for `M-e`, not a dead `´` — and the other side
    // composes as the layout says. The translation drops Alt for a composing side.
    #[cfg(target_os = "macos")]
    let attributes = {
        use winit::platform::macos::{OptionAsAlt, WindowAttributesExtMacOS};
        attributes.with_option_as_alt(match spec.option_meta {
            OptionMeta::Left => OptionAsAlt::OnlyLeft,
            OptionMeta::Right => OptionAsAlt::OnlyRight,
            OptionMeta::Both => OptionAsAlt::Both,
            OptionMeta::None => OptionAsAlt::None,
        })
    };
    let window = elwt
        .create_window(attributes)
        .map_err(|e| format!("window: {e}"))?;
    let window = Rc::new(window);
    // An input method composes in the window unless the app opted out (`{:ime false}`):
    // winit allows none by default, which left dead keys, compose and every CJK input
    // method unable to type here at all (ADR-441).
    window.set_ime_allowed(spec.ime);
    // A `gui-gpu` build draws on the GPU unless BROOD_GUI_GPU=0; a GPU that cannot be
    // brought up (no adapter, no surface) is reported once and the window falls back to
    // the CPU painter rather than failing to open.
    #[cfg(feature = "gui-gpu")]
    let backend = if gpu_enabled() {
        match crate::host::gui::gpu::GpuWindow::new(window.clone(), spec.vsync) {
            Ok(gpu) => Backend::Gpu(Box::new(gpu)),
            Err(e) => {
                eprintln!("brood gui: GPU render target unavailable ({e}); using the CPU painter");
                cpu_backend(&window)?
            }
        }
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
    renderer.set_ligatures(defaults.ligatures);
    let inner = window.inner_size();
    Ok(Win {
        window,
        backend,
        renderer,
        size: Arc::new(Mutex::new((80, 24))),
        size_px: Arc::new(Mutex::new((inner.width, inner.height))),
        subscriber,
        pixel_input: spec.pixel_input,
        sounds: HashMap::new(),
        music: None,
        frame: Vec::new(),
        bleed: bleed::Bleed::default(),
        mods: ModifiersState::empty(),
        option_sides: (false, false),
        option_meta: spec.option_meta,
        non_latin_layout: false,
        cursor: (0, 0),
        cursor_px: (0, 0),
        cursor_at: (0.0, 0.0),
        held: None,
        last_click: None,
        keyboard: Keyboard::new(spec.key_repeat),
        shown: false,
        drawn: false,
        opened_at: Instant::now(),
        input_at: None,
        caret_blink: None,
        draw_input: None,
        latency: Arc::default(),
        zones: Vec::new(),
        shape: None,
        busy_check: None,
        busy_shown: false,
        busy_local: false,
        busy_remote: false,
        scroll_velocity: 0.0,
        scroll_momentum_active: false,
        scroll_decay: TRACKPAD_DECAY,
        wheel_last: None,
        wheel_streak: 0,
        scroll_next_tick: Instant::now(),
        particles_next: None,
        scroll_last_tick: Instant::now(),
        scroll_pending: false,
        ime: spec.ime,
        composition: Composition::Idle,
        ime_area: None,
        wheel: spec.wheel,
        wheel_backlog: 0.0,
        wheel_sent: None,
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
                    size_px: win.size_px.clone(),
                    keys: win.keyboard.report(),
                    latency: win.latency.clone(),
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

/// The GUI thread holds this while it handles an event; `true` once the process is exiting.
/// `std::process::exit` — `nest run --for` running out, a game quitting, any CLI exit —
/// runs the C exit handlers on its own thread while this one may still be painting a
/// queued frame, and a GPU driver's handler tears its state down under it (NVIDIA's
/// `vkCreateBuffer` segfaulted in a `write_buffer`, every `nest run --for` on the GPU
/// target). `close_at_exit` shuts the gate before the driver's handler runs.
static EXIT_GATE: Mutex<bool> = Mutex::new(false);

/// How often a window with particles in flight repaints by itself (~60 Hz).
const PARTICLE_FRAME: Duration = Duration::from_millis(16);

/// The gate for one event: `None` once the process is exiting, when the event is dropped.
fn enter_gate() -> Option<std::sync::MutexGuard<'static, bool>> {
    let guard = lock(&EXIT_GATE);
    if *guard { None } else { Some(guard) }
}

/// Shut the gate when the process exits, waiting out the event in hand. C exit handlers
/// run last-registered first, so this is called once the GPU driver is loaded (a GPU
/// window's device made): it then runs before the driver's own teardown.
#[cfg(feature = "gui-gpu")]
pub(crate) fn close_at_exit() {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    // `atexit` is the C runtime's on Windows as on Unix.
    REGISTERED.call_once(|| {
        // SAFETY: `atexit` takes a plain `extern "C" fn()` that lives for the process.
        unsafe {
            libc::atexit(close_gate);
        }
    });
}

#[cfg(feature = "gui-gpu")]
extern "C" fn close_gate() {
    // Bounded: an exit called FROM the GUI thread, mid-event, holds the gate itself.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match EXIT_GATE.try_lock() {
            Ok(mut exiting) => {
                *exiting = true;
                return;
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                *poisoned.into_inner() = true;
                return;
            }
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(std::sync::TryLockError::WouldBlock) => return,
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
        let Some(_gate) = enter_gate() else {
            return;
        };
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
            // Measure a cell at some font size for a window (behind gui-cell-size).
            UserEvent::Metrics { id, px, reply } => {
                let answer = match self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    Some(w) => {
                        let m = match px {
                            Some(p) => w.renderer.metrics_at(p),
                            None => w.renderer.metrics(),
                        };
                        Ok((
                            m.cell_w.min(u16::MAX as usize) as u16,
                            m.cell_h.min(u16::MAX as usize) as u16,
                        ))
                    }
                    None => Err("gui window not open".to_string()),
                };
                let _ = reply.send(answer);
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
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get(wid))
                    && let Ok(ic) = Icon::from_rgba(rgba, iw, ih)
                {
                    win.window.set_window_icon(Some(ic));
                }
            }
            // A sprite texture lives on the GPU target; the CPU target has no sprite
            // arm, so for it the upload is a no-op (the `:sprite` op is skipped there too).
            UserEvent::Texture {
                id,
                tex,
                rgba,
                w: tw,
                h: th,
            } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    match &mut win.backend {
                        // The CPU painter samples it from memory (`paint::paint_sprite`).
                        Backend::Cpu { .. } => {
                            win.renderer
                                .textures
                                .insert(tex, render::CpuTexture { w: tw, h: th, rgba });
                        }
                        #[cfg(feature = "gui-gpu")]
                        Backend::Gpu(gpu) => gpu.upload_texture(tex, &rgba, tw, th),
                    }
                }
            }
            UserEvent::TextureFree { id, tex } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    match &mut win.backend {
                        Backend::Cpu { .. } => {
                            win.renderer.textures.remove(&tex);
                        }
                        #[cfg(feature = "gui-gpu")]
                        Backend::Gpu(gpu) => gpu.free_texture(tex),
                    }
                }
            }
            UserEvent::InputMode { id, pixels } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid))
                    && win.pixel_input != pixels
                {
                    win.pixel_input = pixels;
                    publish_size(win);
                }
            }
            UserEvent::Sound { id, snd, data } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    win.sounds.insert(snd, data);
                }
            }
            UserEvent::SoundFree { id, snd } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    win.sounds.remove(&snd);
                }
            }
            UserEvent::Draw { id, ops } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    // Brood finished rendering; the next momentum step may fire.
                    w.scroll_pending = false;
                    // The first frame to arrive after an input is taken as the one that
                    // answers it (ADR-440): the mailbox between hides which turn of the
                    // app's loop the input caused, so this is the honest approximation.
                    let input_at = w.input_at.take();
                    // A frame identical to the one on screen — a timer that fired and
                    // changed nothing, a model turn whose view is unchanged — costs no
                    // repaint at all. The op vocabulary is plain data, so equality is exact.
                    // A window still hidden paints it anyway: it is the app's first frame,
                    // and the window waits for one to show itself.
                    if ops == w.frame && w.shown {
                        return;
                    }
                    // Two frames before one paint keep the EARLIER input's stamp: the
                    // paint answers both.
                    w.draw_input = w.draw_input.or(input_at);
                    // Painting before this frame arrived would present the bare background.
                    w.drawn = true;
                    // Sounds are ops of the frame: each `[:sound …]` plays once, here, as
                    // the frame arrives — not on repaints, and (by the equality above) not
                    // for a frame that repeats the last one verbatim.
                    for op in &ops {
                        if let Op::Sound { id: snd, vol } = op
                            && let Some(data) = w.sounds.get(snd)
                        {
                            crate::host::audio::play(data, *vol);
                        }
                    }
                    reconcile_music(w, &ops);
                    w.renderer.emitters.frame_arrived(&ops);
                    // Refresh the cursor hot-zones from this frame, then store it. The walk
                    // goes INTO the regions (`ScrollRegion`, `CellRegion`), resolving each
                    // zone to pixels with the metrics its own region is painted at.
                    w.zones = cursor_zones(&ops, &w.renderer);
                    w.frame = ops;
                    w.bleed.invalidate();
                    // A caret that moved, changed or appeared starts its blink over, shown; the
                    // same caret in a new frame (a spinner turned, a line elsewhere changed)
                    // keeps blinking where it was (ADR-469).
                    w.caret_blink = match (blinking_caret(&w.frame), w.caret_blink.take()) {
                        (None, _) => None,
                        (Some(caret), Some(clock)) if clock.caret == *caret => Some(clock),
                        (Some(caret), _) => CaretBlink::of(caret),
                    };
                    update_ime_area(w);
                    w.window.request_redraw();
                }
            }
            UserEvent::Settle { id } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    // As a Draw would: the app is done with the last scroll step, so the
                    // next momentum step may fire — and the input it was answering changed
                    // nothing, so no later frame is its answer.
                    w.scroll_pending = false;
                    w.input_at = None;
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
            // A thin client's daemon says its loop is busy, or not (ADR-491).
            UserEvent::Busy { id, on } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    w.busy_remote = on;
                    show_busy(w);
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
                    publish_size(w);
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
            UserEvent::RegisterFamily { name, faces, paths } => {
                self.families.borrow_mut().register(name, *faces, paths);
                fonts_changed(&mut self.wins);
            }
            // By installed name: resolved against the system font database (scanned on
            // the first such call), the bundled mono standing in for a name not found.
            UserEvent::RegisterInstalledFamily { name, family_name } => {
                self.families
                    .borrow_mut()
                    .register_installed(name, &family_name);
                fonts_changed(&mut self.wins);
            }
            UserEvent::FontInfo { family, reply } => {
                let family = family
                    .or(self.defaults.family)
                    .unwrap_or_else(|| value::intern(DEFAULT_FAMILY));
                let info = self.families.borrow().info(family, self.defaults.ligatures);
                let _ = reply.send(info);
            }
            UserEvent::Ligatures { on } => {
                self.defaults.ligatures = on;
                for w in self.wins.values_mut() {
                    w.renderer.set_ligatures(on);
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
        let Some(_gate) = enter_gate() else {
            return;
        };
        let Some(w) = self.wins.get_mut(&window_id) else {
            return;
        };
        // Stamp the input this event is (ADR-440): a key press, a button press, a wheel step.
        // The earliest unanswered one is kept; the next frame to arrive answers it, and a
        // turn that settles with nothing to draw (`settle`) drops it. A press the arms below
        // drop (an auto-repeat) is stamped too — the frame the app's own repeat draws
        // answers it, which is what the user waits on. A bare modifier is not: `C-x` is
        // timed from the `x`, not from however long Ctrl was held before it.
        let is_input = match &event {
            WindowEvent::KeyboardInput {
                event: key_event,
                is_synthetic,
                ..
            } => {
                key_event.state == ElementState::Pressed
                    && !is_synthetic
                    && !is_modifier_key(key_event)
            }
            WindowEvent::MouseInput { state, .. } => *state == ElementState::Pressed,
            // a horizontal-only wheel event scrolls nothing here, so no frame answers it
            WindowEvent::MouseWheel { delta, .. } => match delta {
                MouseScrollDelta::LineDelta(_, y) => *y != 0.0,
                MouseScrollDelta::PixelDelta(position) => position.y != 0.0,
            },
            // an input method's commit is typing, whatever the keyboard did
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => !text.is_empty(),
            _ => false,
        };
        if is_input && w.busy_check.is_none() {
            // the loop may now be busy with this input: look in a moment (ADR-491)
            w.busy_check = Some(Instant::now() + BUSY_PROBE);
        }
        if is_input {
            // the caret shows from the moment the user does something, whatever its blink
            // phase, and the next blink is a whole interval away (ADR-469)
            wake_caret(w, true);
            if w.input_at.is_none() {
                w.input_at = Some(Instant::now());
            }
        }
        // A key that reached the window ends a composition that already ended: one that
        // was cancelled (Escape sends an empty preedit and no commit) must not make the
        // next one-character commit read as composed text.
        if let WindowEvent::KeyboardInput {
            event: key_event, ..
        } = &event
            && key_event.state == ElementState::Pressed
            && w.composition == Composition::Ended
        {
            w.composition = Composition::Idle;
        }
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
            WindowEvent::ModifiersChanged(m) => {
                use winit::keyboard::ModifiersKeyState;
                w.mods = m.state();
                w.option_sides = (
                    m.lalt_state() == ModifiersKeyState::Pressed,
                    m.ralt_state() == ModifiersKeyState::Pressed,
                );
            }
            WindowEvent::Resized(_) => {
                // The sub-cell remainder is placed at paint time (`grid_origin`:
                // on the right, and split above and below), so there's no
                // window-resize snap to do here — it works on every WM.
                // Wake the app loop so it re-renders at the new (cols, rows)
                // now, rather than after its (possibly long) poll timeout.
                publish_size(w);
                w.window.request_redraw();
            }
            // We deliberately ignore 0.30's `inner_size_writer` (which could
            // request a specific new inner size): the cell grid *reflows* to
            // whatever size the window is, so we just re-derive the scale and
            // recompute (cols, rows) from the current `inner_size()`.
            WindowEvent::ScaleFactorChanged { .. } => {
                w.renderer.set_scale(w.window.scale_factor());
                publish_size(w);
                w.window.request_redraw();
            }
            WindowEvent::KeyboardInput {
                event: ke,
                is_synthetic,
                ..
            } => match ke.state {
                // Every press goes through `Keyboard::press`, which tells a repeat from a fresh
                // press by TRANSITION, not winit's `ke.repeat` flag: on
                // GNOME/Wayland that flag is unreliable — held keys arrive as a
                // flood of `repeat == false` presses (ADR-086). A press for the
                // key already held (no release between) is the repeat. A genuine
                // re-press (double-tap) comes only after a release, so it still
                // registers. Synthetic presses (winit replaying held keys on focus
                // *gain*) are dropped — they'd be phantom keystrokes.
                //
                // A repeat is dropped under `{:key-repeat :app}` (ADR-086: the app
                // repeats from `gui-held-key`) and, under `:os` (ADR-441), delivered as
                // another press through `RepeatGate`, so at most one is ever undelivered
                // and a slow frame cannot build a backlog that plays on after key-up. A
                // keystroke that typed several characters (a compose sequence) is
                // `[:text …]`, not a key, and is held as itself: its repeats are found
                // the same way, and `gui-held-key` reports it as `[:text …]`.
                ElementState::Pressed if !is_synthetic => {
                    let press = KeyPress::of(&ke);
                    w.non_latin_layout = layout_is_non_latin(&press, w.non_latin_layout);
                    let stroke =
                        translate_keystroke(&press, w.chord_modifiers(), w.non_latin_layout);
                    match stroke.and_then(|stroke| {
                        w.keyboard.press(stroke, ke.physical_key, Instant::now())
                    }) {
                        Some(KeyMessage::Press(message)) => deliver(w.subscriber, message),
                        Some(KeyMessage::Repeat(message, credit)) => {
                            deliver_credited(w.subscriber, message, credit)
                        }
                        Some(KeyMessage::Restroke { up, press }) => {
                            deliver(w.subscriber, up);
                            deliver(w.subscriber, press);
                        }
                        None => {}
                    }
                }
                ElementState::Pressed => {} // synthetic press (focus-gain replay)
                // Key release → `[:key-up <what its press delivered>]` (`Keyboard::release`,
                // which matches it to its press by PHYSICAL key, so a shifted chord whose
                // Shift came up first still names its press). Synthetic releases count too
                // (winit emits them for a key let go while unfocused — exactly when we must
                // stop).
                ElementState::Released => {
                    let translated =
                        translate_key(&KeyPress::of(&ke), w.chord_modifiers(), w.non_latin_layout);
                    if let Some(message) = w.keyboard.release(ke.physical_key, translated) {
                        deliver(w.subscriber, message);
                    }
                }
            },
            // Losing focus (Alt-Tab away mid-hold) is the case a key-up can go
            // missing entirely — the release happens in another window. Deliver
            // `:blur` so the app can drop any held key and stop repeating; a
            // belt-and-suspenders backstop beside the synthetic releases above
            // (ADR-086). Focus *gain* is `:focus-in`, below.
            WindowEvent::Focused(false) => {
                // A window without focus holds its caret still and shown (ADR-469).
                wake_caret(w, false);
                // Drop the held key: we can't observe its release while unfocused,
                // so `gui-held-key` must not keep reporting it (the poll-based stop)
                // and the `:blur` is the event-based stop. Both, belt-and-braces.
                w.keyboard.blur();
                deliver(w.subscriber, Message::Keyword(value::intern("blur")));
                // A composition does not survive the window losing focus on every
                // platform's IME, and not every one says so: clear what the app shows.
                end_composition(w);
                // Same for a held mouse button: its release may land off-window /
                // unfocused and never reach us, so synthesize one now (see CursorLeft).
                if let Some(b) = w.held.take() {
                    let (col, row) = w.cursor;
                    deliver(
                        w.subscriber,
                        mouse_message(&release_of(b, col, row, w.pointer_at(), &w.mods)),
                    );
                }
            }
            // Focus GAIN, as `:focus-in` — the pair of `:blur` (ADR-441). What an app
            // needs to repaint focus-dependent chrome (a hollow cursor turned solid), to
            // re-check files changed while it was away, and to know the held-key state it
            // dropped on `:blur` starts clean. Not `:focus`: that is `ui-run`'s
            // raise-this-window control message, which the loop acts on itself.
            WindowEvent::Focused(true) => {
                wake_caret(w, true);
                deliver(w.subscriber, Message::Keyword(value::intern("focus-in")));
            }
            // The input method (ADR-441). A composition in progress is `[:preedit text
            // range]` — shown at point by the app, not inserted — and an empty one clears
            // it; a commit is `[:text …]`, or a plain key when the IME merely passed one
            // character through (`commit_message`). A disabled IME drops whatever
            // composition was showing, so the app is told to clear it.
            WindowEvent::Ime(ime) => match ime {
                winit::event::Ime::Enabled => {
                    // The candidate window must be told where the caret is again.
                    w.ime_area = None;
                    update_ime_area(w);
                }
                winit::event::Ime::Preedit(text, cursor) => {
                    if text.is_empty() {
                        if w.composition != Composition::Showing {
                            return;
                        }
                        w.composition = Composition::Ended;
                    } else {
                        w.composition = Composition::Showing;
                    }
                    deliver(w.subscriber, preedit_message(&text, cursor));
                }
                winit::event::Ime::Commit(text) => {
                    // winit clears the preedit just before the commit that ends it, so a
                    // composition that just ended counts as composed too.
                    let composed = w.composition != Composition::Idle;
                    w.composition = Composition::Idle;
                    if let Some(message) = commit_message(&text, composed) {
                        deliver(w.subscriber, message);
                    }
                }
                winit::event::Ime::Disabled => end_composition(w),
            },
            WindowEvent::CursorLeft { .. } => {
                // The pointer left the window. If a button was held, its release happens
                // outside and we never see it — so the NEXT re-entry's motion would emit
                // a phantom `:drag` and the app would think the button is still pressed.
                // Synthesize the release + clear `held` (mirrors the keyboard blur fix).
                if let Some(b) = w.held.take() {
                    let (col, row) = w.cursor;
                    deliver(
                        w.subscriber,
                        mouse_message(&release_of(b, col, row, w.pointer_at(), &w.mods)),
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
                let px = px_clamped(position);
                // A pixel-input window (a game) reports every pixel of motion — that IS
                // the grain it asked for, and winit already coalesces motion per frame —
                // where a cell-grid window reports only a cell crossing.
                let moved = if w.pixel_input {
                    px != w.cursor_px
                } else {
                    cell != w.cursor
                };
                w.cursor = cell;
                w.cursor_px = px;
                w.cursor_at = px_to_cell_frac(
                    position,
                    &w.renderer,
                    psz.width as usize,
                    psz.height as usize,
                );
                if moved {
                    let (col, row) = w.pointer();
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
                            at: w.pointer_at(),
                        }),
                    );
                }
                // Hover cursor: show a zone's shape (e.g. a resize cursor on a divider,
                // a pointing hand on a link) while the pointer is over it. Locally
                // handled — no event reaches the app, so no redraw flood. (ADR-080.)
                //
                // Tested per PIXEL, not per cell: a zone inside a zoomed `CellRegion` is
                // a fraction of a window cell tall, so two neighbouring zones can live in
                // one cell — hit-testing only when the *cell* changed would step straight
                // over them. The `set_cursor` below is still guarded by a change in shape,
                // which is the call that costs anything.
                let (grid_x, grid_y) = w
                    .renderer
                    .grid_origin(psz.width as usize, psz.height as usize);
                let want = shape_at(
                    &w.zones,
                    position.x as isize - grid_x as isize,
                    position.y as isize - grid_y as isize,
                );
                if want != w.shape {
                    w.shape = want;
                    // the busy cursor wins while the loop is stuck; its end restores this one
                    if !w.busy_shown {
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
                    let (col, row) = w.pointer();
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
                            at: w.pointer_at(),
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
                    let (col, row) = w.pointer();
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
                            at: w.pointer_at(),
                        }),
                    );
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                // Positive y scrolls up (away from the user). LineDelta is in discrete
                // line units; PixelDelta (trackpad) is in physical pixels — normalised
                // here to line units so Brood gets a consistent float in both cases.
                //
                // A wheel notch (LineDelta) scrolls its lines at once or glides, per the
                // window's `WheelMode`; a trackpad (PixelDelta) tracks the finger and coasts.
                // Momentum itself is driven by `about_to_wait` with `WaitUntil(12ms)` so
                // it fires once per event-loop cycle after the queue is drained — no
                // flooding, no background thread.
                let ch = w.renderer.cell_h.max(1);
                match delta {
                    MouseScrollDelta::LineDelta(_, y) => {
                        // `{:wheel :glide}`: a notch is an IMPULSE into the kinetic scroll a
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
                        // `{:wheel :lines}` (the default, ADR-441): the notch's whole
                        // distance at once, as a terminal and Emacs scroll — no glide, no
                        // streak. A notch the app has not drawn the last one of yet joins
                        // the backlog (`flush_wheel`), so a spun wheel is one message per
                        // frame carrying the sum, never a queue of them.
                        if w.wheel == WheelMode::Lines {
                            w.scroll_momentum_active = false;
                            w.scroll_velocity = 0.0;
                            if w.wheel_backlog * dy < 0.0 {
                                w.wheel_backlog = 0.0;
                            }
                            w.wheel_backlog += dy * WHEEL_LINES;
                            // what cannot go now, `about_to_wait` sends after the draw
                            flush_wheel(w, Instant::now());
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
                // Nothing drawn yet and still hidden: painting now would present the bare
                // background, the flash on open. `about_to_wait` shows the window after
                // `SHOW_TIMEOUT` if the app never draws (ADR-440).
                if !w.drawn && !w.shown {
                    return;
                }
                let _sg = crate::core::heap::stall_guard("gui-paint");
                let paint_start = Instant::now();
                w.renderer.caret_hidden = w
                    .caret_blink
                    .as_ref()
                    .is_some_and(|clock| clock.hidden(paint_start));
                // The GPU paint's phases, or `None` from the CPU painter; nothing at all
                // when the GPU target skipped the frame (an occluded window) — nothing
                // reached the compositor, so the window stays hidden and the input
                // unanswered for the frame that does.
                let inner = w.window.inner_size();
                let margins =
                    bleed::Margins::of(&w.renderer, inner.width as usize, inner.height as usize);
                let frame = w.bleed.frame(&w.frame, margins);
                let painted: Option<Option<[u64; 5]>> = match &mut w.backend {
                    Backend::Cpu { surface, .. } => {
                        paint(surface, &w.window, &mut w.renderer, frame);
                        Some(None)
                    }
                    #[cfg(feature = "gui-gpu")]
                    Backend::Gpu(gpu) => gpu
                        .paint(frame, &mut w.renderer)
                        .map(|times| Some(times.phases())),
                };
                let presented = Instant::now();
                // The app has drawn: the window may appear. Shown even when the GPU skipped
                // this frame — a surface reports itself occluded while its window is hidden
                // (Metal does), so waiting for a presented frame would wait out
                // `SHOW_TIMEOUT` — and asked to paint again once visible.
                if !w.shown {
                    w.shown = true;
                    w.window.set_visible(true);
                    if painted.is_none() {
                        w.window.request_redraw();
                    }
                }
                if let Some(split) = painted
                    && let Some(input_at) = w.draw_input.take()
                {
                    record_latency(w, input_at, paint_start, presented, split);
                }
                w.particles_next = w
                    .renderer
                    .emitters
                    .alive()
                    .then(|| Instant::now() + PARTICLE_FRAME);
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
        let Some(_gate) = enter_gate() else {
            return;
        };
        let now = Instant::now();
        let mut next_wakeup: Option<Instant> = None;
        for w in self.wins.values_mut() {
            // A window whose app has not drawn within `SHOW_TIMEOUT` is shown anyway,
            // painted with what it has: never an invisible window (ADR-440).
            if !w.shown {
                let due = w.opened_at + SHOW_TIMEOUT;
                if now >= due {
                    w.shown = true;
                    w.window.set_visible(true);
                    w.window.request_redraw();
                } else {
                    next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
                }
            }
            if let Some(due) = busy_cursor(w, now) {
                next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
            }
            if let Some(due) = w.particles_next {
                if now >= due {
                    // the paint sets the next one while particles remain
                    w.particles_next = None;
                    w.window.request_redraw();
                } else {
                    next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
                }
            }
            // The caret blinks here, not in the app's loop (ADR-469): a phase that differs
            // from the one painted asks for a paint — of the caret's row, on the CPU path —
            // and the window sleeps until the next toggle.
            if let Some(clock) = &w.caret_blink
                && w.shown
            {
                if clock.hidden(now) != w.renderer.caret_hidden {
                    w.window.request_redraw();
                }
                if let Some(due) = clock.next_toggle(now) {
                    next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
                }
            }
            // A `{:wheel :lines}` backlog goes out once the app has drawn the last
            // scroll (or `WHEEL_HOLD` has passed); until then, wake to check again.
            if !flush_wheel(w, now) {
                let due = w.wheel_sent.map_or(now, |sent| sent + WHEEL_HOLD);
                next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
            }
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

/// A family was (re-)registered: every window drops the glyphs baked from the old
/// faces, re-measures its cell (its own family may be the one that moved), tells its
/// app the grid it now has, and repaints.
fn fonts_changed(wins: &mut HashMap<WindowId, Win>) {
    for w in wins.values_mut() {
        w.renderer.fonts_changed();
        publish_size(w);
        w.window.request_redraw();
    }
}

/// Retune one window's cell font (family and/or px), then recompute its grid,
/// republish its size for `gui-size`, and request a repaint. Shared by the
/// global-default and per-window arms of `UserEvent::Font`.
fn apply_font(w: &mut Win, family: Option<u32>, px: Option<f32>) {
    w.renderer.set_font(family, px);
    // A new font changes the cell size, hence the (cols, rows) grid — exactly like a
    // window resize. Wake the app loop with the new grid so it RE-RENDERS its content at
    // the new size; without this the redraw below only repaints the stale old frame and
    // the buffer body goes blank until the next unrelated input/timer (the font-switch UI
    // break). The sub-cell remainder is re-placed at paint time (`grid_origin`).
    publish_size(w);
    w.window.request_redraw();
}

/// Tell the input method where the caret is, so its candidate window opens beside it
/// rather than in a screen corner (ADR-441): the frame's `Op::Cursor` cell
/// (`ime_cursor_area`), offset by the grid origin into window pixels. Only when it moved —
/// a frame per keystroke must not cost an IME round trip each.
fn update_ime_area(w: &mut Win) {
    if !w.ime {
        return;
    }
    let Some((x, y, width, height)) = ime_cursor_area(&w.frame, &w.renderer) else {
        return;
    };
    let inner = w.window.inner_size();
    let (origin_x, origin_y) = w
        .renderer
        .grid_origin(inner.width as usize, inner.height as usize);
    let area = (
        x.saturating_add(origin_x as isize),
        y.saturating_add(origin_y as isize),
        width,
        height,
    );
    if w.ime_area == Some(area) {
        return;
    }
    w.ime_area = Some(area);
    w.window.set_ime_cursor_area(
        PhysicalPosition::new(area.0.max(0) as i32, area.1.max(0) as i32),
        winit::dpi::PhysicalSize::new(width as u32, height as u32),
    );
}

/// Where an input method's composition stands. `Ended` is the empty preedit winit sends
/// just before the commit that ends a composition (or alone, for a cancelled one): the
/// commit after it is composed text, `[:text …]`, though nothing shows any more.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Composition {
    Idle,
    Showing,
    Ended,
}

/// Drop the composition — the IME was switched off, or the window lost focus — clearing
/// what the app shows if one was showing.
fn end_composition(w: &mut Win) {
    if std::mem::replace(&mut w.composition, Composition::Idle) == Composition::Showing {
        deliver(w.subscriber, preedit_message("", None));
    }
}

/// Record one input-to-present sample (ADR-440): from `input_at` (the GUI thread
/// received the input) to `presented` (the paint of the first frame that arrived after
/// it returned from its present), with the paint's own share and, on the GPU target,
/// its phases. Kept for `gui-latency`; printed under `BROOD_GUI_TRACE=1` (or a
/// `BROOD_STALL_MS` threshold the sample exceeds).
fn record_latency(
    w: &Win,
    input_at: Instant,
    paint_start: Instant,
    presented: Instant,
    split: Option<[u64; 5]>,
) {
    let sample = LatencySample {
        total_us: presented.duration_since(input_at).as_micros() as u64,
        paint_us: presented.duration_since(paint_start).as_micros() as u64,
        split,
    };
    if let Some(threshold_ms) = paint_stall_ms()
        && u128::from(sample.total_us) >= threshold_ms * 1000
    {
        let ms = |us: u64| us as f64 / 1000.0;
        let phases: String = sample
            .split
            .map(|split| {
                PAINT_PHASES
                    .iter()
                    .zip(split)
                    .map(|(name, us)| format!(" {name}={:.2}ms", ms(us)))
                    .collect::<String>()
            })
            .map(|named| format!(" |{named}"))
            .unwrap_or_default();
        eprintln!(
            "[gui-latency] input->present {:.2}ms (to paint {:.2}ms, paint {:.2}ms){phases}",
            ms(sample.total_us),
            ms(sample.total_us.saturating_sub(sample.paint_us)),
            ms(sample.paint_us),
        );
    }
    let mut samples = lock(&w.latency);
    if samples.len() == LATENCY_SAMPLES {
        samples.pop_front();
    }
    samples.push_back(sample);
}

/// Republish a window's size to the Brood side and wake its app loop with the
/// `[:resize …]` it expects: cells for a cell-grid window, PIXELS for a pixel-input
/// one (`WindowSpec::pixel_input`) — the unit every other message of that window uses.
/// Shared by the resize, scale-change and font arms so the three cannot disagree.
fn publish_size(w: &Win) {
    update_cells(&w.window, &w.renderer, &w.size);
    let inner = w.window.inner_size();
    *lock(&w.size_px) = (inner.width, inner.height);
    if w.pixel_input {
        deliver(w.subscriber, resize_message_px(inner.width, inner.height));
    } else {
        let (cols, rows) = *lock(&w.size);
        deliver(w.subscriber, resize_message(cols, rows));
    }
}

/// Recompute `(cols, rows)` from the window's physical size and the cell
/// metrics, and publish it for `gui-size`.
fn update_cells(window: &winit::window::Window, r: &Renderer, size: &Arc<Mutex<(u16, u16)>>) {
    let sz = window.inner_size();
    *lock(size) = bleed::grid_cells(r, sz.width as usize, sz.height as usize);
}

impl Win {
    /// The modifiers a key event is translated under (`chord_modifiers`): Alt is Meta
    /// unless the only Option keys held are ones that compose (ADR-541).
    fn chord_modifiers(&self) -> crate::host::gui::KeyModifiers {
        let (left, right) = self.option_sides;
        chord_modifiers(self.mods, left, right, self.option_meta)
    }

    /// The pointer position a mouse message carries, as `(col, row)` — the cell for a
    /// cell-grid window, the physical pixel `(x, y)` for a pixel-input one. One place,
    /// so press, release and motion cannot disagree about the unit.
    fn pointer(&self) -> (u16, u16) {
        if self.pixel_input {
            self.cursor_px
        } else {
            self.cursor
        }
    }

    /// The sub-cell position a mouse message carries beside [`Win::pointer`]: the fractional
    /// cell for a cell-grid window, None for a pixel-input one (its pointer is already exact).
    fn pointer_at(&self) -> Option<(f64, f64)> {
        if self.pixel_input {
            None
        } else {
            Some(self.cursor_at)
        }
    }
}

/// A window pixel position clamped to the `(x, y)` a mouse message can carry (u16 —
/// wider than any display). Negative coordinates (a pointer dragged past the window's
/// top-left while a button is held) clamp to the edge, like `px_to_cell`.
fn px_clamped(pos: PhysicalPosition<f64>) -> (u16, u16) {
    let clamp = |v: f64| v.max(0.0).min(u16::MAX as f64) as u16;
    (clamp(pos.x), clamp(pos.y))
}

/// A window pixel position to a (col, row) character cell, clamped to u16. The grid
/// origin (`grid_origin` — inset plus the remainder placement, the same the grid is
/// painted with) is subtracted first, so a click lands on the cell painted under it;
/// a click in the surrounding margin clamps to the edge cell.
/// [`px_to_cell`] without the floor: the pointer's `(row, col)` in fractional window cells,
/// from the same grid origin and cell, so `floor` of it is exactly the cell `px_to_cell`
/// gives. Clamped at 0 like it.
fn px_to_cell_frac(
    pos: PhysicalPosition<f64>,
    r: &Renderer,
    w_px: usize,
    h_px: usize,
) -> (f64, f64) {
    let (ox, oy) = r.grid_origin(w_px, h_px);
    let col = (pos.x - ox as f64).max(0.0) / r.cell_w.max(1) as f64;
    let row = (pos.y - oy as f64).max(0.0) / r.cell_h.max(1) as f64;
    (row, col)
}

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
