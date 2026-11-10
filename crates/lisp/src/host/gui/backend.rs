//! The windowed backend (feature `gui`): the winit event loop on its own thread (or the
//! process main thread where the platform demands it — ADR-324), the window registry
//! the `%gui-*` primitives address by id, and the messages a window delivers to its
//! subscribing process. Input translation, text rendering and frame painting are the
//! child modules.

mod bleed;
mod busy_cursor;
mod caret_blink;
mod degradation;
mod display_loss;
mod event_loop;
mod exit_gate;
mod fonts;
#[cfg(all(test, feature = "gui-gpu"))]
mod gpu_ink;
#[cfg(all(test, feature = "gui-gpu"))]
mod gpu_replay;
mod headless;
mod ink;
mod input;
mod keyboard;
mod main_thread;
mod offscreen;
mod paint;
mod primitives;
mod render;
mod rows;
mod wheel;
#[cfg(test)]
mod wheel_tests;
mod window_build;

use busy_cursor::*;
use caret_blink::*;
use degradation::*;
use display_loss::*;
use event_loop::*;
use exit_gate::*;
use fonts::*;
use headless::*;
use ink::*;
use input::*;
use keyboard::*;
use main_thread::*;
use paint::*;
pub(crate) use render::Renderer;
pub use render::TextAa;
use render::*;
use wheel::*;
use window_build::*;
// The GPU render target (`gpu.rs`) paints with the same text metric and hairline snap.
#[cfg(feature = "gui-gpu")]
pub(crate) use {
    ink::luminance, paint::holds_blinking_caret, paint::paint_stall_ms, paint::text_px_width,
    render::caret_cells, render::caret_glyph, render::cursor_rim, render::snap_hairline,
    render::text_units, render::unit_cells, rows::RowMap,
};

pub(super) use degradation::report_degraded;
pub use display_loss::lose_display;
#[cfg(feature = "gui-gpu")]
pub(crate) use exit_gate::close_at_exit;
pub use headless::headless;
pub use main_thread::host_main_thread;
pub use primitives::{
    bg, busy, capture, cell_size, close, drag_move, drag_resize, draw, fg, focus, font, font_info,
    fullscreen, grab, held_key, icon, input_mode, inset, key_repeat, key_repeats, latency,
    latency_histograms, ligatures, line_height, maximize, minimize, next_texture_id, open,
    register_family, register_installed_family, render_frame, settle, size, size_px, sound,
    sound_free, text_aa, text_contrast, texture, texture_free, title, window_ids,
};

use super::{
    CursorShape, InputKind, Key, KeyRepeat, Keystroke, LatencyHistogram, LatencyLog, LatencySample,
    Mouse, MouseAction, MouseButton, Op, OptionMeta, PAINT_PHASES, RepeatCounts, WheelMode,
    WindowSpec,
};

use crate::core::sync::lock;
use crate::core::value;

// Everything a window delivers is the user's input, so all of it rides the mailbox's
// PRIORITY lane (ADR-490): a UI loop takes it ahead of any background backlog.
use crate::process::{
    Message, deliver_priority as deliver, deliver_priority_credited as deliver_credited,
};

use std::cell::RefCell;

use std::collections::HashMap;

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

// The text contrast exponent γ a window starts with: the perceptual gamma light-on-dark text is
// weighted at (`TextInk`, ADR-611). 1.0 is the encoded lerp's weight, which reads thin for
// light text on a dark ground (its stems lose light where they straddle pixels); 2.2 is linear
// light's, which reads soft (ADR-358). 1.4 is between: fuller stems, rims that do not glow.
const TEXT_CONTRAST: f32 = 1.4;

// The rendering *mechanism's* fallback colours — used ONLY when Brood supplies
// none: `Op::Clear` / the inset-margin fill when no `gui-bg!` is set, and the text of a
// face with no `:fg` when no `gui-fg!` is set. Brood owns the actual palette as *policy* — every render op
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
    /// Set the content inset (logical px) — the margin on the cell grid's left,
    /// right and bottom — for every open window and ones opened later. Behind
    /// `gui-inset!`. The grid loses `2*inset` px of width and `inset` of height, so its
    /// cell count shrinks; the window re-renders at the new size.
    Inset { px: f32 },
    /// Set the window background — the fill for `Op::Clear`, the pre-clear, and the
    /// inset margin / snap remainder outside the cell grid — for every open window
    /// and ones opened later. Behind `gui-bg!`; `None` restores `DEFAULT_BG`. Pure
    /// repaint, no metric change.
    Background { rgb: Option<[u8; 3]> },
    /// Set the default text colour — what a text op whose face names no `:fg` is drawn
    /// in — for every open window and ones opened later. Behind `gui-fg!`; `None`
    /// restores `DEFAULT_FG`. Pure repaint, no metric change.
    Foreground { rgb: Option<[u8; 3]> },
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
    /// Paint window `id`'s current frame into memory, as its last paint drew it, and reply
    /// with the picture. Behind `gui/capture` (ADR-609).
    Capture {
        id: u64,
        reply: Sender<Result<super::FrameImage, String>>,
    },
}

/// A freshly opened window's wiring, handed back to the Brood side: its id and
/// the shared cell size the GUI thread keeps current. Input is *not* polled — the
/// GUI thread delivers it straight to the subscriber's mailbox (ADR-059).
struct OpenReply {
    id: u64,
    size: Arc<Mutex<(u16, u16)>>,
    size_px: Arc<Mutex<(u32, u32)>>,
    keys: Arc<Mutex<KeyReport>>,
    key_repeat: Arc<AtomicBool>,
    latency: Arc<Mutex<LatencyLog>>,
}

/// What the Brood side keeps per open window (keyed by the id `open` returns) —
/// just the shared cell size for `gui-size`. Input arrives as mailbox messages,
/// so there is no receiver to keep here (ADR-059).
struct WinHandle {
    /// The process its input is delivered to — told `[:hangup :display-lost]` when the
    /// display goes away under it (ADR-631).
    subscriber: u64,
    size: Arc<Mutex<(u16, u16)>>,
    /// The window's inner size in PHYSICAL pixels, republished on every resize, for
    /// `gui-size-px` — the frame a pixel-space app lays itself out in.
    size_px: Arc<Mutex<(u32, u32)>>,
    /// What the window's keyboard reports (`Keyboard::report`): the keystroke held down,
    /// for `gui-held-key` (the source of truth for a held key — immune to a missed key-up,
    /// ADR-086), and how its repeats went, for `gui-key-repeats`.
    keys: Arc<Mutex<KeyReport>>,
    /// Whether the last key press the app took from its mailbox was an OS repeat
    /// (`Keyboard::received_repeat`), for `gui-key-repeat?` (ADR-612).
    key_repeat: Arc<AtomicBool>,
    /// The last `LATENCY_SAMPLES` input-to-present samples, newest last, for
    /// `gui-latency` (ADR-440), and the session's distribution per kind of input, for
    /// `gui-latency-histogram` (ADR-612).
    latency: Arc<Mutex<LatencyLog>>,
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
    /// Whether the pointer is in the leftover strip of a frame that paints its next line
    /// there (ADR-615): `cursor`'s row is then the seam's, the strip's own (`Mouse::partial`).
    cursor_partial: bool,
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
    input_at: Option<(Instant, InputKind)>,
    /// The input stamp the frame waiting to be painted answers, carried to the paint so
    /// it can report input -> present.
    draw_input: Option<(Instant, InputKind)>,
    /// The last `LATENCY_SAMPLES` input-to-present samples and the session's distribution,
    /// shared with `gui-latency` and `gui-latency-histogram`.
    latency: Arc<Mutex<LatencyLog>>,
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
    /// Content inset (logical px) left, right and below the grid.
    inset: f32,
    /// Window background; `None` = `DEFAULT_BG`.
    bg: Option<[u8; 3]>,
    /// Default text colour; `None` = `DEFAULT_FG`.
    fg: Option<[u8; 3]>,
    /// Cell height as a multiple of the font px.
    line_height: f32,
    /// How monochrome text is anti-aliased.
    text_aa: TextAa,
    /// The text contrast exponent (`TEXT_CONTRAST`; 1.0 = no lift, the encoded lerp's weight).
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
            fg: None,
            line_height: LINE_HEIGHT,
            text_aa: TextAa::Auto,
            text_contrast: TEXT_CONTRAST,
            ligatures: false,
        }
    }
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
                    key_repeat: win.keyboard.received_repeat(),
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
    let (width_px, height_px) = (inner.width as usize, inner.height as usize);
    let (origin_x, _) = w.renderer.grid_origin(width_px, height_px);
    // the caret's uniform grid pixels as window pixels, through the row map (ADR-610)
    let area = (
        x.saturating_add(origin_x as isize),
        w.renderer.rows(width_px, height_px).window_y(y),
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
    input: InputKind,
    paint_start: Instant,
    presented: Instant,
    split: Option<[u64; 5]>,
) {
    let sample = LatencySample {
        total_us: presented.duration_since(input_at).as_micros() as u64,
        paint_us: presented.duration_since(paint_start).as_micros() as u64,
        split,
        input,
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
    lock(&w.latency).record(sample, LATENCY_SAMPLES);
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

    /// Whether the pointer is in a painted leftover strip (ADR-615, `Mouse::partial`): never
    /// for a pixel-input window, whose pointer is pixels.
    fn pointer_partial(&self) -> bool {
        !self.pixel_input && self.cursor_partial
    }
}

/// A window pixel position clamped to the `(x, y)` a mouse message can carry (u16 —
/// wider than any display). Negative coordinates (a pointer dragged past the window's
/// top-left while a button is held) clamp to the edge, like `px_to_cell`.
fn px_clamped(pos: PhysicalPosition<f64>) -> (u16, u16) {
    let clamp = |v: f64| v.max(0.0).min(u16::MAX as f64) as u16;
    (clamp(pos.x), clamp(pos.y))
}

/// A window pixel position to a (col, row) character cell, clamped to u16. The column
/// subtracts the grid's left edge; the row is the row map's inverse (`RowMap::row_at`, the
/// same map the grid is painted with, seam and all — ADR-610), so a click lands on the cell
/// painted under it; a click in the surrounding margin clamps to the edge cell, and one in
/// a seam's gap is the row above it, whose backgrounds paint the gap.
/// [`px_to_cell`] without the floor: the pointer's `(row, col)` in fractional window cells,
/// from the same grid origin and cell, so `floor` of it is exactly the cell `px_to_cell`
/// gives. Clamped at 0 like it.
fn px_to_cell_frac(
    pos: PhysicalPosition<f64>,
    r: &Renderer,
    w_px: usize,
    h_px: usize,
) -> (f64, f64) {
    let (ox, _) = r.grid_origin(w_px, h_px);
    let col = (pos.x - ox as f64).max(0.0) / r.cell_w.max(1) as f64;
    let row = r.rows(w_px, h_px).row_at(pos.y);
    (row, col)
}

fn px_to_cell(pos: PhysicalPosition<f64>, r: &Renderer, w_px: usize, h_px: usize) -> (u16, u16) {
    let (ox, _) = r.grid_origin(w_px, h_px);
    let col =
        ((pos.x - ox as f64).max(0.0) as usize / r.cell_w.max(1)).min(u16::MAX as usize) as u16;
    let row = r.rows(w_px, h_px).row_of(pos.y);
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
mod main_thread_hosting;
