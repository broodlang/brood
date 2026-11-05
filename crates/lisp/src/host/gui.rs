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
//! **Input never blocks a worker (ADR-059).** Rather than handing keys back through
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
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Face {
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// The underline's own colour (`:underline '#ff0000'` or `{:color …}`); `None` draws it in the
    /// text colour.
    pub underline_rgb: Option<[u8; 3]>,
    /// A wavy underline (`{:style :wave}`, Emacs's `:underline (:style wave)`) — what a spell
    /// checker or a diagnostic marks a word with, leaving the word its own colour.
    pub underline_wave: bool,
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
            underline_rgb: None,
            underline_wave: false,
            reverse: false,
            family: None,
            scale: 1,
        }
    }
}

impl Face {
    /// The face a block caret draws the text under it in (ADR-515): the cell's own face
    /// (its family, weight, slant and underline), coloured `ground`, the window
    /// background, with no background or reverse of its own, so it reads as a cut-out of
    /// the solid caret. Its scale is one: the caret is one cell tall, and a scaled glyph
    /// would spill out of it.
    // the painters are `gui`-only, its unit test is not
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub(crate) fn under_caret(self, ground: [u8; 3]) -> Face {
        Face {
            fg: Some(ground),
            bg: None,
            reverse: false,
            underline_rgb: None,
            scale: 1,
            ..self
        }
    }
}

/// The rectangles `(x, y, w, h)` (pixels) an underline paints under a run `w` pixels wide from
/// `left`, its rule's top at `y` and `thickness` tall: one rule, or for a wave a zigzag of
/// `thickness`-square steps rising and falling `2 × thickness` over a `4 × thickness` period.
/// The phase is the ABSOLUTE x, so the wave under one word is continuous across the cells
/// (and runs) it is painted in. Shared by the CPU and GPU painters.
// the painters are `gui`-only, its unit tests are not
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub(crate) fn underline_rects(
    left: isize,
    y: isize,
    w: usize,
    thickness: usize,
    wave: bool,
) -> Vec<(isize, isize, usize, usize)> {
    let thickness = thickness.max(1);
    if !wave {
        return vec![(left, y, w, thickness)];
    }
    let half = (2 * thickness) as isize;
    let period = 2 * half;
    (0..w as isize)
        .step_by(thickness)
        .map(|i| {
            let x = left + i;
            let phase = x.rem_euclid(period);
            let rise = if phase < half { phase } else { period - phase };
            // centred on the plain rule: from a thickness above it to a thickness below
            let step = (thickness as isize).min(w as isize - i) as usize;
            (x, y - thickness as isize + rise, step, thickness)
        })
        .collect()
}

/// What `gui-font-info` reports about one font family: where its faces came from, so a
/// user who asked for "JetBrains Mono" can see which file they got — or that they got
/// the bundled DejaVu because nothing by that name is installed (ADR-442). Plain data,
/// defined here rather than in the backend so the builtin compiles without the GUI.
#[derive(Clone, Debug, Default)]
pub struct FontInfo {
    /// The family name shaping uses (`"DejaVu Sans Mono"` for the bundled `:mono`).
    pub name: String,
    /// The installed name a `:family-name` registration asked for, when it was one.
    pub requested: Option<String>,
    /// `"bundled"`, `"files"` (registered from paths), `"system"` (found by name) or
    /// `"not-found"` (asked for by name, not installed: the bundled mono stands in).
    pub origin: &'static str,
    /// The file each style resolved to — regular, bold, italic, bold-italic. A bundled
    /// face reads `"<bundled DejaVuSansMono.ttf>"`.
    pub faces: [String; 4],
    /// The system fonts loaded so far to draw glyphs the primary font lacks, as
    /// `(family, file)`, in the order they were first needed.
    pub fallbacks: Vec<(String, String)>,
    /// Whether the system font database has been scanned yet (it is only on demand).
    pub system_scanned: bool,
    /// Whether runs of punctuation are shaped together (`gui-ligatures!`).
    pub ligatures: bool,
}

/// A frame as a window paints it, in pixels: what `gui/render-frame` and `gui/capture`
/// answer (ADR-609). `rgba` is `width * height * 4` bytes, row-major, opaque. The rest
/// is the geometry the picture was painted with, so a test can find a row or a cell in
/// it without re-deriving the layout rule: the cell in physical px, the grid in cells,
/// where the grid's top-left sits, the inset, the seam the frame declared and the pixel
/// top of every row (ADR-610). Plain data, defined here so the builtins compile without
/// the GUI.
#[derive(Clone, Debug, Default)]
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub cell: (u32, u32),
    pub grid: (u16, u16),
    pub origin: (u32, u32),
    pub inset: u32,
    /// The frame's seam row (`Op::Seam`), when it declared one inside the grid.
    pub seam: Option<u16>,
    /// The pixel row each grid row starts at, then where the last one ends: `grid.1 + 1`
    /// entries, read off the renderer's row map (`RowMap::y`), so row `k` covers
    /// `[row_tops[k], row_tops[k] + cell.1)` and a seam's leftover is the step between
    /// `row_tops[seam - 1] + cell.1` and `row_tops[seam]`.
    pub row_tops: Vec<u32>,
}

/// Which font `gui/render-frame` paints with: a family keyword the offscreen painter
/// knows (the bundled `:mono`, or one registered while headless painting), or an
/// installed family by its name, resolved as `gui-font-register`'s `:family-name` is.
#[derive(Clone, Debug)]
pub enum FontChoice {
    Registered(u32),
    Installed(String),
}

/// What `gui/render-frame` paints a frame AS: the window it stands in for. Every
/// `None` is the default a window opens with (840×560 at scale 1, the bundled mono at
/// 15 px, line height 1.4, no inset, the default background and text colour).
#[derive(Clone, Debug, Default)]
pub struct RenderSpec {
    /// Physical pixels.
    pub size: Option<(u32, u32)>,
    pub font: Option<FontChoice>,
    pub px: Option<f32>,
    pub line_height: Option<f32>,
    pub inset: Option<f32>,
    pub bg: Option<[u8; 3]>,
    /// The default text colour (`gui-fg!`): what a text op with no `:fg` is drawn in.
    pub fg: Option<[u8; 3]>,
    pub scale: Option<f64>,
    pub text_aa: Option<TextAa>,
    pub ligatures: Option<bool>,
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
    /// True when the window delivers its mouse input in PIXELS rather than cells (the
    /// `{:input :pixels}` open option): a `[:mouse action button y x mods …]` message
    /// carries the pointer's physical pixel position in the row/col slots, `:move` and
    /// `:drag` fire on every pixel of motion (not once per cell crossed), and `[:resize
    /// w h]` reports the window's pixel size. The shape of every message is unchanged, so
    /// `ui-run` and the key dispatch do not care; only the numbers' unit does. What a
    /// game drawing with the pixel-space ops (`:sprite`, `:px-rect`, `:line`) needs — a
    /// cell is the wrong grain for a sprite under the pointer.
    pub pixel_input: bool,
    /// The `:vsync` open option, as asked: `Some(true)` presents in step with the
    /// monitor refresh (FIFO); `Some(false)` asks for the work-bound rate, tearing
    /// allowed (IMMEDIATE where the platform has it); `None`, the default, is the
    /// low-latency choice that never tears (MAILBOX where available, else FIFO).
    /// Only the GPU render target honours it: the CPU path presents synchronously
    /// whatever this says (ADR-440).
    pub vsync: Option<bool>,
    /// Who repeats a held key (the `{:key-repeat …}` open option, ADR-441).
    pub key_repeat: KeyRepeat,
    /// True to let an input method compose text in this window (the `{:ime …}` open
    /// option, on by default): its preedit arrives as `[:preedit text range]`, its
    /// commits as `[:text text]`, and the candidate window is placed at the frame's
    /// cursor. A game that reads raw keys can turn it off.
    pub ime: bool,
    /// What a mouse-wheel notch does (the `{:wheel …}` open option, ADR-441).
    pub wheel: WheelMode,
    /// Which Option key is Meta on macOS (the `{:option-meta …}` open option, ADR-541).
    pub option_meta: OptionMeta,
}

/// Who repeats a held key in a GUI window (ADR-441).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KeyRepeat {
    /// The app does (`{:key-repeat :app}`, the default): the OS auto-repeat is dropped and
    /// a held key is one press and one `[:key-up …]`, so the app paces its own repeat
    /// from `gui-held-key` (ADR-086).
    #[default]
    App,
    /// The OS does (`{:key-repeat :os}`): each auto-repeat is delivered as a press of the
    /// held key, at the user's system delay and rate — but never more than ONE undelivered
    /// repeat at a time, so a slow frame cannot queue a burst that plays on after release.
    Os,
}

/// What one mouse-wheel notch does in a GUI window (ADR-441). A trackpad's pixel deltas
/// and their momentum are the same under both.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WheelMode {
    /// The notch's whole distance (three lines) in one scroll message, now — what a
    /// terminal and Emacs do (`{:wheel :lines}`, the default). Notches that arrive while
    /// the app has not yet drawn the last one are summed into the next message.
    #[default]
    Lines,
    /// The notch is an impulse into a short kinetic glide (`{:wheel :glide}`): the same
    /// three lines over about half a second of fractional steps.
    Glide,
}

/// Which Option key is Meta on macOS (the `{:option-meta …}` open option, ADR-541) — Emacs's
/// `ns-alternate-modifier` / `ns-right-alternate-modifier`. An Option key that is not Meta
/// COMPOSES, as everywhere else on a Mac: right-Option+2 types `@` on a German layout, and
/// the `@` arrives as typed text. Only macOS reports which side is held, so elsewhere Alt is
/// Meta and AltGr is AltGr (never Alt to the OS), whatever this says.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OptionMeta {
    /// Left Option is Meta, right Option composes (`:left`, the default).
    #[default]
    Left,
    /// Right Option is Meta, left Option composes (`:right`).
    Right,
    /// Both Option keys are Meta (`:both`) — nothing composes with Option.
    Both,
    /// Neither is Meta (`:none`) — both compose, as in every other Mac app.
    None,
}

impl OptionMeta {
    /// Whether Alt, held on the sides given, is Meta. `left` / `right` say which Option keys
    /// the OS reports down; when it reports neither (every platform but macOS), Alt is Meta.
    pub fn alt_is_meta(self, left: bool, right: bool) -> bool {
        if !left && !right {
            return true;
        }
        let left_meta = matches!(self, OptionMeta::Left | OptionMeta::Both);
        let right_meta = matches!(self, OptionMeta::Right | OptionMeta::Both);
        (left && left_meta) || (right && right_meta)
    }
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
            pixel_input: false,
            vsync: None,
            key_repeat: KeyRepeat::App,
            ime: true,
            wheel: WheelMode::Lines,
            option_meta: OptionMeta::Left,
        }
    }
}

/// One input-to-present measurement of a window (ADR-440, `gui-latency`): from the GUI
/// thread receiving an input (a key press, a mouse button, a wheel step) to the paint
/// of the first frame that ARRIVED after it returning from its present. Which frame
/// answers which input cannot be known across the app's mailbox, so "the next frame
/// drawn" is the approximation; a present returning is when the compositor has the
/// frame, not when the panel lights it (add up to one refresh for that).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatencySample {
    /// input -> present, microseconds
    pub total_us: u64,
    /// the paint's share of it (op walk to present), microseconds
    pub paint_us: u64,
    /// the GPU paint's phases — encode, upload, acquire, submit, present — in
    /// microseconds; `None` on the CPU painter
    pub split: Option<[u64; 5]>,
    /// what kind of input it answers (ADR-612)
    pub input: InputKind,
}

/// How a window's held keys have repeated since it opened (`gui-key-repeats`, ADR-441):
/// the OS repeats delivered to the app and those dropped because one was still waiting
/// (the app fell behind the repeat rate), and, from the last key that repeated, the
/// desktop's delay before its first repeat and the interval between its last two — so an
/// app can compare how long it takes to answer one with how often they come.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RepeatCounts {
    pub delivered: u64,
    pub dropped: u64,
    /// press -> first repeat, microseconds
    pub delay_us: Option<u64>,
    /// between the last two repeats, microseconds
    pub interval_us: Option<u64>,
}

/// The names of `LatencySample::split`'s phases, in order.
pub const PAINT_PHASES: [&str; 5] = ["encode", "upload", "acquire", "submit", "present"];

/// Which kind of input a latency sample answers (ADR-612): a key (a press, an OS repeat, an
/// input method's commit) or the pointer (a button, a wheel step). Kept apart because "how
/// fast does a key show" is the number typing feels, and a wheel step answered by a glide is
/// a different question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Key,
    Pointer,
}

/// A window's input-to-present times as a DISTRIBUTION (ADR-612): every sample since the
/// window opened, counted into log-linear buckets — exact below 16 µs, then 16 buckets per
/// power of two, so a bucket is at most 1/16 (6.25%) wide — with the count and the largest
/// sample. The ring of recent samples (`LatencyLog::samples`) answers "what were the last
/// few"; this answers p50 / p99 / p99.9 over a whole session, where the tail that typing
/// feels lives (one key in a thousand at ten keys a second is one every hundred seconds).
/// Fixed size, so recording costs an index and an add on the GUI thread whatever the count.
#[derive(Clone, Debug, PartialEq)]
pub struct LatencyHistogram {
    counts: Vec<u64>,
    pub count: u64,
    pub max_us: u64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        LatencyHistogram {
            counts: vec![0; LatencyHistogram::BUCKETS],
            count: 0,
            max_us: 0,
        }
    }
}

impl LatencyHistogram {
    /// Sub-buckets per power of two (4 bits): a bucket's width is 1/16 of its lower bound.
    const SUB: u64 = 16;
    /// Samples at or past 2^30 µs (about 18 minutes) share the last bucket.
    const CEILING_US: u64 = (1 << 30) - 1;
    /// 16 exact buckets, then 16 for each power of two from 2^4 up to 2^29.
    pub const BUCKETS: usize = 16 + 26 * 16;

    /// The bucket `us` falls in.
    pub fn bucket_of(us: u64) -> usize {
        let us = us.min(Self::CEILING_US);
        if us < Self::SUB {
            return us as usize;
        }
        let octave = 63 - u64::from(us.leading_zeros());
        let shift = octave - 4;
        let sub = (us >> shift) & (Self::SUB - 1);
        (Self::SUB + shift * Self::SUB + sub) as usize
    }

    /// The largest value bucket `index` holds, in microseconds: what a percentile that
    /// lands in it reads as, so a percentile is never under the samples it stands for.
    pub fn bucket_upper_us(index: usize) -> u64 {
        let index = index as u64;
        if index < Self::SUB {
            return index;
        }
        let shift = (index - Self::SUB) / Self::SUB;
        let sub = (index - Self::SUB) % Self::SUB;
        ((Self::SUB + sub) << shift) + (1 << shift) - 1
    }

    /// Count one sample of `us` microseconds.
    pub fn record(&mut self, us: u64) {
        self.counts[Self::bucket_of(us)] += 1;
        self.count += 1;
        self.max_us = self.max_us.max(us);
    }

    /// The buckets holding any sample, ascending, as `(upper bound µs, count)`.
    pub fn buckets(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(index, count)| (Self::bucket_upper_us(index), *count))
    }
}

/// What a window keeps of its input-to-present times: the last `samples` (newest last,
/// `gui-latency`, ADR-440) and the session's distribution per kind of input
/// (`gui-latency-histogram`, ADR-612).
#[derive(Clone, Debug, Default)]
pub struct LatencyLog {
    pub samples: std::collections::VecDeque<LatencySample>,
    pub keys: LatencyHistogram,
    pub pointer: LatencyHistogram,
}

impl LatencyLog {
    /// Keep `sample`, dropping the oldest past `ring` samples, and count it in its kind's
    /// distribution.
    pub fn record(&mut self, sample: LatencySample, ring: usize) {
        if self.samples.len() == ring {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        match sample.input {
            InputKind::Key => self.keys.record(sample.total_us),
            InputKind::Pointer => self.pointer.record(sample.total_us),
        }
    }
}

#[cfg(test)]
mod latency_histogram_tests {
    use super::*;

    #[test]
    fn every_value_lands_in_a_bucket_whose_upper_bound_is_within_a_sixteenth() {
        let mut previous = 0;
        for us in (0..5000).chain([65_535, 1_000_000, 123_456_789]) {
            let index = LatencyHistogram::bucket_of(us);
            let upper = LatencyHistogram::bucket_upper_us(index);
            assert!(upper >= us, "{us} µs reads as {upper}");
            assert!(
                (upper - us) * 16 <= us.max(1),
                "{us} µs reads as {upper}: too wide"
            );
            assert!(
                index >= previous || us > 5000,
                "buckets ascend with the value"
            );
            previous = index;
        }
    }

    #[test]
    fn the_last_bucket_holds_everything_past_the_ceiling() {
        let last = LatencyHistogram::BUCKETS - 1;
        assert_eq!(LatencyHistogram::bucket_of(u64::MAX), last);
        assert_eq!(LatencyHistogram::bucket_of(1 << 30), last);
        assert_eq!(LatencyHistogram::bucket_upper_us(last), (1 << 30) - 1);
    }

    #[test]
    fn a_log_counts_each_kind_apart_and_keeps_a_bounded_ring() {
        let mut log = LatencyLog::default();
        let sample = |total_us, input| LatencySample {
            total_us,
            paint_us: 0,
            split: None,
            input,
        };
        for us in [4_000, 5_000, 6_000] {
            log.record(sample(us, InputKind::Key), 2);
        }
        log.record(sample(90_000, InputKind::Pointer), 2);
        assert_eq!(log.samples.len(), 2);
        assert_eq!((log.keys.count, log.keys.max_us), (3, 6_000));
        assert_eq!((log.pointer.count, log.pointer.max_us), (1, 90_000));
        let buckets: Vec<_> = log.keys.buckets().collect();
        assert_eq!(buckets.len(), 3);
        assert!(buckets.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(buckets.iter().map(|(_, count)| count).sum::<u64>(), 3);
    }
}

/// The pointer shape a cursor zone requests — frontend-neutral (mapped to a winit
/// `CursorIcon` only inside the GUI backend). `ColResize` is the ↔ used for a
/// side-by-side (`:col`) split's divider; `RowResize` the ↕ for a stacked (`:row`)
/// one. (ADR-080.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    ColResize,
    RowResize,
    /// A hand / link pointer — the `:pointer` shape a clickable text region (a
    /// results-buffer row, a mode-line segment) requests to read as a link.
    Pointer,
}

/// Where a pixel-space text op's `x` lands on its run: its left edge (the default), its
/// centre, or its right edge — so a HUD centres a caption without knowing the cell
/// metrics the frontend will shape it with.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// How the text cursor is drawn at its cell. `Block` (the default) overlays the
/// whole cell — the terminal-style caret; `Bar` is a thin vertical line on the
/// cell's left edge (a modern GUI insertion caret); `Underline` a thin rule along
/// the cell bottom; `Hollow` an outline box around the cell — the cursor of a window
/// that does not have focus, as Emacs and kitty draw it. A `[:cursor row col]` op with
/// no style is `Block`, so existing callers are unchanged. The terminal frontend maps
/// these to crossterm's steady cursor styles (`Hollow` to a block: a terminal has no
/// outline cursor).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
    Hollow,
}

/// How a caret BLINKS when the window animates it itself (ADR-469): visible for
/// `interval_ms`, hidden for `interval_ms`, and so on — `blinks` whole cycles, then it
/// stays visible until the next input (Emacs's `blink-cursor-blinks`); `0` blinks
/// forever. The window resets the phase to visible on every key, button and wheel step
/// it sees and on every frame that moves the caret, so a typist never sees it vanish.
/// The app draws the caret ONCE and the window toggles it: the blink costs the app's
/// loop nothing, and a toggle repaints the caret's row, not the window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CursorBlink {
    pub interval_ms: u32,
    pub blinks: u32,
}

impl CursorBlink {
    /// Whether a caret blinking since `elapsed` ago is in its hidden half now. The
    /// first half-period is visible (the delay before the first blink), and after
    /// `blinks` cycles it is visible for good.
    pub fn hidden_after(&self, elapsed: std::time::Duration) -> bool {
        let interval = u128::from(self.interval_ms.max(1));
        let half_periods = elapsed.as_millis() / interval;
        if self.blinks > 0 && half_periods >= 2 * u128::from(self.blinks) {
            return false;
        }
        half_periods % 2 == 1
    }

    /// When the caret blinking since `since` next changes visibility, after `now` — or
    /// `None` once its `blinks` have run out and it stays visible.
    pub fn next_toggle(
        &self,
        since: std::time::Instant,
        now: std::time::Instant,
    ) -> Option<std::time::Instant> {
        let interval = u128::from(self.interval_ms.max(1));
        let half_periods = now.saturating_duration_since(since).as_millis() / interval;
        if self.blinks > 0 && half_periods >= 2 * u128::from(self.blinks) {
            return None;
        }
        let next = (half_periods + 1) * interval;
        Some(since + std::time::Duration::from_millis(u64::try_from(next).unwrap_or(u64::MAX)))
    }
}

/// The ops of one memoised view fragment (`editor/ui`'s `ui-memo`, ADR-470), parsed once
/// and SHARED: a fragment whose deps did not change arrives at the window as the very
/// allocation it arrived as last frame. Equality tries the pointer first, so comparing
/// an unchanged fragment with its previous self — the frame diff, the strip diff — is
/// O(1) instead of a walk of every op and string in it, and cloning a frame clones a
/// pointer per fragment.
#[derive(Clone)]
pub struct FragmentOps(pub std::sync::Arc<[Op]>);

impl PartialEq for FragmentOps {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0) || self.0[..] == other.0[..]
    }
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
        /// The caret's colour (the op's optional 5th element); `None` is the
        /// painters' default — a near-white rim over a 50% white wash for a block,
        /// `CURSOR_FG` for the thin shapes. A block with a colour washes the cell in
        /// it at 50% and rims it solid; every other shape is drawn solid in it.
        color: Option<[u8; 3]>,
        /// Blink the caret in the window, with no frame from the app per toggle
        /// (`[:cursor row col style colour blink]`, ADR-469); `None` is a steady caret.
        blink: Option<CursorBlink>,
        /// The text under the caret and the face it is painted in
        /// (`[:cursor row col style colour blink under face]`, ADR-515). With it a BLOCK
        /// caret is drawn as Emacs draws its box: the cell filled solid in the caret's
        /// colour and this text drawn over it in its own face, coloured the window
        /// background ([`Face::under_caret`]), so the character stays readable through the
        /// caret, bold or italic as it is. Without it a block is the half-strength wash it
        /// always was. Other shapes ignore it.
        under: Option<(String, Face)>,
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
    /// Draw `ops` with the cell metrics of font size `px`, inside the rect `[x y w h]`
    /// given in the PARENT's cells. Inner ops are positioned in the region's own cell
    /// space (row 0, col 0 is the region's top-left), so a caller lays a region out in its
    /// own units and does not have to know the outer size.
    ///
    /// This is what lets one frame carry two text sizes — an editor zooming ONE buffer
    /// while the rest of the window stays put. The alternative the grid already had is the
    /// `:scale` face attr, but that is a whole-number multiple of the cell (2×, 3×), so the
    /// smallest step it can express is a doubling. A region carries a px, so a step is a
    /// pixel.
    ///
    /// Scoped and self-restoring like `ScrollRegion`: ops after the region are painted with
    /// the window's metrics again, and regions nest (an inner one overrides). GUI-only — the
    /// terminal SKIPS it (as it does `FRect` and `VSpans`): a terminal cell is whatever the
    /// terminal says it is, and flattening region-local ops would paint them in the wrong
    /// place.
    CellRegion {
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        px: f32,
        ops: Vec<Op>,
    },
    /// A textured quad in PIXEL space — the sprite mechanism. Texture `tex` (a handle
    /// `gui-texture` uploaded under) is drawn with its top-left at `(x, y)` physical
    /// pixels from the window's corner, `w`×`h` pixels large, sampling the texture-space
    /// rect `uv` (`[u0 v0 du dv]`, 0..1; a negative extent mirrors), multiplied by `tint`
    /// (straight rgba; opaque white leaves the texel as is) and turned `rot` radians about
    /// its centre. Deliberately the bare quad: which part of a sprite sheet a frame is,
    /// a flip, an animation are Brood policy over this (`gui/sprite` and the engine
    /// above it) — the renderer only needs a rect, a UV rect, a tint and an angle.
    /// Pixel space, not cells, because a sprite lives at a sub-cell position and moves
    /// by pixels. Consecutive sprites from one texture are one draw, so the Brood side
    /// is O(sprites) and the per-pixel work is native, the `VSpans` rule. Instanced on the
    /// GPU target, sampled per pixel in software by the CPU painter (`paint::paint_sprite`);
    /// the terminal skips it.
    Sprite {
        tex: u32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        tint: [u8; 4],
        rot: f32,
    },
    /// A solid quad in PIXEL space: `w`×`h` pixels at `(x, y)`, filled with `color`
    /// (straight rgba, so `[r g b 128]` is a half-transparent overlay) and turned `rot`
    /// radians about its centre. The pixel-space sibling of `Rect`, and the one solid
    /// primitive a game needs: a line is a thin quad at an angle (`gui/line`, Brood), a
    /// health bar two of them. Drawn by both window painters (the CPU one per pixel).
    Quad {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [u8; 4],
        rot: f32,
    },
    /// Text in PIXEL space: the run `s` shaped in `face` (the same face a `Text` op
    /// takes — colours, bold, italic, underline, `:scale`) with its top at `y` and its
    /// left, centre or right edge at `x` per `align`, physical pixels from the window's
    /// corner. What a game's HUD and title screens need, since a cell row is the wrong
    /// place for a caption over a sprite. Drawn by both window painters; the terminal
    /// skips it (like `Quad`).
    TextPx {
        x: f32,
        y: f32,
        s: String,
        face: Face,
        align: Align,
    },
    /// Play sound `id` (a handle `gui-sound` uploaded under) once, at peak amplitude
    /// `vol` (0..1). A sound is an op of the frame that carries it, so it rides the display
    /// seam like a texture does: the window that draws the frame plays it — the app's own
    /// window, a thin client's over the wire, or a browser tab — and a headless test sees
    /// it in the frame. Fired when the frame is DRAWN (once per `gui-draw`), not on
    /// every repaint. Draws nothing; the terminal ignores it.
    Sound {
        id: u32,
        vol: f32,
    },
    /// Keep sound `id` LOOPING at peak amplitude `vol` (0..1) for as long as the frames
    /// drawn carry this op — a game's music. Where `Sound` is an event, `Music` is state:
    /// a frame with the same handle keeps the track going (a new `vol` fades to it), a
    /// different handle switches tracks, and a frame without one fades it out. So a
    /// thin client or a late joiner hears what the game is playing from the next frame
    /// it draws. The first `Music` op of a frame wins. Draws nothing; the terminal
    /// ignores it.
    Music {
        id: u32,
        vol: f32,
    },
    /// Turn LIGHTING on for this frame: what is drawn before the frame's `Overlay` (all
    /// of it, without one) is multiplied by a light map that starts at `color` — the light
    /// where no `Light` reaches — and adds every `Light` on top. Without an `Ambient` op a
    /// frame is unlit, however many `Light`s it carries.
    Ambient {
        color: [u8; 3],
    },
    /// A point light at `(x, y)` pixels: `color` × `intensity` at its centre, falling off
    /// smoothly to nothing at `radius`. Lights add. Where in the frame the op sits does
    /// not matter; it takes effect only under an `Ambient`.
    Light {
        x: f32,
        y: f32,
        radius: f32,
        color: [u8; 3],
        intensity: f32,
    },
    /// The ops after this one are the UNLIT layer — a HUD over a lit scene: drawn after
    /// the light map is applied, before post-processing. A second one changes nothing.
    Overlay,
    /// Bloom post-processing: what is brighter than `threshold` (0..1 luma) is blurred
    /// `radius` pixels wide and added back at `intensity`. Applied to the finished frame.
    Bloom {
        threshold: f32,
        intensity: f32,
        radius: f32,
    },
    /// A CRT pass over the finished frame: `scanlines` darkens every other row by that
    /// fraction, `curvature` bulges the picture as a tube's glass does (0 = flat), and
    /// `vignette` darkens toward the corners by that fraction. After `Bloom`.
    Crt {
        scanlines: f32,
        curvature: f32,
        vignette: f32,
    },
    /// Keep particle emitter `id` alive at `(x, y)` pixels, emitting as `spec` says, for as
    /// long as the frames drawn carry it: state, like `Music`. The particles live in the
    /// window (simulated on the GPU by a compute shader, or natively by the CPU painter),
    /// so a frame carries one op per emitter, never one per particle. When a frame stops
    /// carrying it the emitter stops emitting and its particles live out their lives.
    /// Drawn where the op sits in the frame's order.
    Emitter {
        id: u32,
        x: f32,
        y: f32,
        spec: EmitterSpec,
    },
    /// Emit `count` particles at once from emitter `id` (an event, like `Sound`: once,
    /// when the frame is drawn). Names an emitter of the same frame.
    Burst {
        id: u32,
        count: u32,
    },
    /// A memoised view fragment's ops (`[:ui/memo key deps ops]`, ADR-470), painted
    /// exactly as if they were spliced in its place — every painter descends into it with
    /// the context it is in (scroll shift, cell region). It exists for what it SHARES: an
    /// unchanged fragment is the same allocation frame after frame (`FragmentOps`), so the
    /// parse, the frame diff and the strip diff skip it. A fragment never carries a
    /// frame-global op (`Sound`, `Music`, the effects, the emitters): `%gui-draw` splices
    /// one that would, so the scanners that read those stay at the top level and in
    /// regions.
    Fragment(FragmentOps),
    /// How the frame's rows sit in the window (`[:seam row]` / `[:seam row {:top-band b}]`,
    /// ADR-610): state of the frame, read at the top level only (the first one wins), and
    /// painting nothing itself.
    ///
    /// `row` is the SEAM: rows above it are anchored to the top of the window, rows from it
    /// down to the bottom, so the last row ends exactly at the bottom inset, and the
    /// leftover height (the window less the margins, mod one row) lies between row
    /// `row - 1` and row `row`, painted by the backgrounds of the row above it. An editor
    /// names its bottom mode line: the echo area stays on the window's bottom edge and the
    /// leftover becomes the text body's partial line, as in Emacs. `None` (or a row past
    /// the grid) leaves the leftover under the last row.
    ///
    /// `top_band` says row 0 is a full-width band (a header line, a tab bar): no margin
    /// above it. False gives the top the inset as the sides have it, and the grid loses
    /// those rows (the window re-publishes its size when it changes). True, the default,
    /// is the layout of a frame without the op.
    ///
    /// The rows stay whole cells and the app stays in cell units: only the renderer's
    /// row-to-pixel map (`backend::rows::RowMap`) and its inverse in the hit-test change.
    Seam {
        row: Option<u16>,
        top_band: bool,
    },
}

/// How an emitter emits: what an `[:emitter id x y opts]` op's map says, with defaults
/// for the keys it leaves out. Velocities and positions are in pixels, times in seconds,
/// angles in radians (0 points right; y points down, so a positive angle turns clockwise).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EmitterSpec {
    /// Particles a second, emitted continuously.
    pub rate: f32,
    /// A particle's life, uniformly between the two.
    pub life: [f32; 2],
    /// Its launch speed, uniformly between the two.
    pub speed: [f32; 2],
    /// The direction of launch, and the full width of the cone around it.
    pub angle: f32,
    pub spread: f32,
    /// Acceleration, and the fraction of its velocity a particle loses each second.
    pub gravity: [f32; 2],
    pub drag: f32,
    /// Its size at birth and at death (linear between).
    pub size: [f32; 2],
    /// Its colour at birth and at death (straight rgba, linear between).
    pub color: [u8; 4],
    pub color_end: [u8; 4],
    /// Added onto what is under it (fire, sparks) rather than blended over it.
    pub additive: bool,
    /// A square rather than a soft disc.
    pub square: bool,
    /// The most particles alive at once; a new one replaces the oldest beyond it.
    pub max: u32,
}

impl Default for EmitterSpec {
    fn default() -> EmitterSpec {
        EmitterSpec {
            rate: 0.0,
            life: [1.0, 1.0],
            speed: [50.0, 100.0],
            angle: -std::f32::consts::FRAC_PI_2,
            spread: std::f32::consts::TAU,
            gravity: [0.0, 0.0],
            drag: 0.0,
            size: [4.0, 4.0],
            color: [255, 255, 255, 255],
            color_end: [255, 255, 255, 0],
            additive: false,
            square: false,
            max: 1024,
        }
    }
}

/// The most particles one emitter may keep: 65 536 × 32 bytes is 2 MB of buffer.
pub const MAX_PARTICLES: u32 = 65_536;

/// A keystroke, in a backend-neutral shape the Brood side turns into the same
/// values `term-poll` yields: `Char` → a 1-char string, `Named` → a keyword (`:ctrl-c`,
/// `:alt-f`, `:up`, …, spelled by [`named_key`]). `PartialEq` so the event loop can tell an
/// auto-repeat (a press for the key *already* held) from a fresh press without
/// trusting winit's `ke.repeat` flag — unreliable on Wayland (ADR-086).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Named(&'static str),
}

/// What one press delivered, and so what is held while its key is down: a [`Key`], or
/// the text a keystroke typed when it was more than one character (a compose sequence, a
/// layout key that types two), delivered as `[:text "…"]`. `gui-held-key` reports it and
/// its release names it, so a held compose sequence is held like any key.
#[derive(Clone, PartialEq, Eq)]
pub enum Keystroke {
    Key(Key),
    Text(String),
}

/// A character `c` under chord `modifiers`, Shift already folded into `c` — ONE rule for
/// both frontends. Control chords are case-INsensitive (Emacs `C-a` = `C-A`); Meta and
/// Super are case-SENSITIVE (`M-O` open-line-above ≠ `M-o`): a shifted letter stays
/// upper-case (by Unicode case, so `M-Ä` is not `M-ä`), while an unshifted chord
/// lower-cases, so Caps Lock or a stray Shift cannot change the binding. A chord spells
/// through [`named_key`] (`:ctrl-x`, `:alt-X`, `:ctrl-meta-x`, `:ctrl-super-x`).
pub fn character_chord(c: char, modifiers: KeyModifiers) -> Key {
    // a letter whose other case is not one char (`ß` → "SS") keeps its own
    fn single(c: char, mut cased: impl Iterator<Item = char>) -> char {
        match (cased.next(), cased.next()) {
            (Some(only), None) => only,
            _ => c,
        }
    }
    if !(modifiers.ctrl || modifiers.alt || modifiers.super_key) {
        return Key::Char(c);
    }
    let glyph = if modifiers.ctrl && !modifiers.super_key {
        c.to_ascii_lowercase()
    } else if modifiers.shift && !modifiers.ctrl {
        single(c, c.to_uppercase())
    } else {
        single(c, c.to_lowercase())
    };
    Key::Named(named_key(
        &glyph.to_string(),
        KeyModifiers {
            shift: false,
            ..modifiers
        },
    ))
}

/// The modifiers a key name spells, frontend-neutral: each frontend reads its own event's
/// modifier state into one of these and [`named_key`] spells it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Super / Windows / Command key — Emacs's `s-`.
    pub super_key: bool,
}

/// The name of a key under modifiers — ONE rule for both frontends, so a chord reads the
/// same whether it arrived from winit or from crossterm:
/// `[ctrl-meta-|ctrl-|alt-][shift-][super-]<name>` — `:ctrl-left`, `:alt-shift-up`,
/// `:ctrl-meta-delete`, `:super-f5`, `:ctrl-shift-super-left`. The order is Emacs's own
/// (`C-M-S-s-`), so a label is the prefixes translated one by one. Character chords
/// already spell their modifiers this way (`:ctrl-x`, `:ctrl-meta-f`); a character chord
/// that has Super (`:super-x`, `:ctrl-super-x`) goes through here too, with its Shift
/// already folded into the glyph (`:super->`, not `:shift-super-.`). Tab keeps its own
/// spelling (Shift+Tab is `:back-tab`) and Escape carries none.
///
/// Returns a `&'static str` — the vocabulary is closed (the named keys and the printable
/// characters × sixteen modifier sets), so each spelling is built once and kept for the
/// process, which lets [`Key`] stay `Copy`.
pub fn named_key(base: &str, modifiers: KeyModifiers) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    let name = format!(
        "{}{}{}{}",
        match (modifiers.ctrl, modifiers.alt) {
            (true, true) => "ctrl-meta-",
            (true, false) => "ctrl-",
            (false, true) => "alt-",
            (false, false) => "",
        },
        if modifiers.shift { "shift-" } else { "" },
        if modifiers.super_key { "super-" } else { "" },
        base
    );
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = crate::core::sync::lock(NAMES.get_or_init(|| Mutex::new(HashSet::new())));
    if let Some(s) = names.get(name.as_str()) {
        return s;
    }
    let leaked: &'static str = Box::leak(name.into_boxed_str());
    names.insert(leaked);
    leaked
}

/// The name of function key F`number` — `"f1"` … `"f24"` — or `None` past F24. Both
/// frontends name the function keys through here, so `<f5>` binds the same in each.
pub fn function_key(number: u8) -> Option<&'static str> {
    const NAMES: [&str; 24] = [
        "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13", "f14",
        "f15", "f16", "f17", "f18", "f19", "f20", "f21", "f22", "f23", "f24",
    ];
    NAMES.get(usize::from(number).checked_sub(1)?).copied()
}

#[cfg(test)]
mod underline_rect_tests {
    use super::underline_rects;

    #[test]
    fn a_plain_underline_is_one_rule() {
        assert_eq!(underline_rects(10, 20, 30, 2, false), vec![(10, 20, 30, 2)]);
    }

    #[test]
    fn a_wave_covers_the_run_and_stays_a_thickness_either_side_of_the_rule() {
        let rects = underline_rects(3, 20, 25, 2, true);
        // contiguous columns from `left` to the end of the run, none past it
        let mut x = 3;
        for (rx, _, w, _) in &rects {
            assert_eq!(*rx, x);
            x += *w as isize;
        }
        assert_eq!(x, 3 + 25);
        // it rises and falls, within [y - t, y + t]
        assert!(rects.iter().all(|(_, y, _, _)| *y >= 18 && *y <= 22));
        let heights: std::collections::BTreeSet<isize> =
            rects.iter().map(|(_, y, _, _)| *y).collect();
        assert!(heights.len() > 1);
    }
}

#[cfg(test)]
mod under_caret_tests {
    use super::Face;

    #[test]
    fn the_text_under_a_block_caret_keeps_its_face_in_the_background_colour() {
        let cell = Face {
            fg: Some([200, 100, 50]),
            bg: Some([10, 10, 10]),
            bold: true,
            italic: true,
            reverse: true,
            underline: true,
            underline_rgb: Some([255, 0, 0]),
            family: Some(2),
            scale: 3,
            ..Face::default()
        };
        let under = cell.under_caret([30, 30, 46]);
        assert_eq!(under.fg, Some([30, 30, 46]));
        assert_eq!(under.bg, None);
        assert!(under.bold && under.italic && under.underline);
        assert!(!under.reverse);
        assert_eq!(under.underline_rgb, None);
        assert_eq!(under.family, Some(2));
        assert_eq!(under.scale, 1);
    }
}

#[cfg(test)]
mod named_key_tests {
    use super::{KeyModifiers, function_key, named_key};
    fn mods(ctrl: bool, alt: bool, shift: bool, super_key: bool) -> KeyModifiers {
        KeyModifiers {
            ctrl,
            alt,
            shift,
            super_key,
        }
    }
    #[test]
    fn spells_modifiers_the_way_character_chords_do() {
        assert_eq!(named_key("left", mods(false, false, false, false)), "left");
        assert_eq!(
            named_key("left", mods(false, false, true, false)),
            "shift-left"
        );
        assert_eq!(
            named_key("left", mods(true, false, false, false)),
            "ctrl-left"
        );
        assert_eq!(named_key("up", mods(false, true, false, false)), "alt-up");
        assert_eq!(
            named_key("up", mods(true, true, false, false)),
            "ctrl-meta-up"
        );
        assert_eq!(
            named_key("right", mods(true, false, true, false)),
            "ctrl-shift-right"
        );
        assert_eq!(
            named_key("delete", mods(false, true, true, false)),
            "alt-shift-delete"
        );
    }
    #[test]
    fn super_comes_last_in_emacs_order() {
        assert_eq!(named_key("x", mods(false, false, false, true)), "super-x");
        assert_eq!(
            named_key("f5", mods(true, false, false, true)),
            "ctrl-super-f5"
        );
        assert_eq!(
            named_key("left", mods(true, true, true, true)),
            "ctrl-meta-shift-super-left"
        );
    }
    #[test]
    fn the_same_chord_is_the_same_static_str() {
        let a = named_key("home", mods(true, false, true, false));
        let b = named_key("home", mods(true, false, true, false));
        assert!(std::ptr::eq(a, b));
    }
    #[test]
    fn function_keys_run_f1_to_f24() {
        assert_eq!(function_key(1), Some("f1"));
        assert_eq!(function_key(24), Some("f24"));
        assert_eq!(function_key(0), None);
        assert_eq!(function_key(25), None);
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
    /// The pointer's position in FRACTIONAL window cells, `(row, col)` — where in the cell
    /// it is, which the whole-cell `row`/`col` throw away. A region painted at another cell
    /// size (a zoomed `CellRegion`, ADR-363) needs it to hit-test: one window cell can hold
    /// parts of two of its cells. Appended as a trailing `{:at [row col]}` (see
    /// `mouse_message`); None where the frontend has no sub-cell position (a terminal) or
    /// the window reports pixels already.
    pub at: Option<(f64, f64)>,
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

#[cfg(feature = "gui")]
pub(crate) mod effects; // lighting, post-processing and particles, shared by both painters

#[cfg(feature = "gui-gpu")]
pub(crate) mod gpu; // the wgpu render target behind `BROOD_GUI_GPU=1`

#[cfg(not(feature = "gui"))]
pub use disabled::{
    TextAa, bg, busy, capture, cell_size, close, drag_move, drag_resize, draw, fg, focus, font,
    font_info, fullscreen, grab, headless, held_key, host_main_thread, icon, input_mode, inset,
    key_repeat, key_repeats, latency, latency_histograms, ligatures, line_height, maximize,
    minimize, next_texture_id, open, register_family, register_installed_family, render_frame,
    settle, size, size_px, sound, sound_free, text_aa, text_contrast, texture, texture_free, title,
    window_ids,
};

#[cfg(feature = "gui")]
pub use backend::{
    TextAa, bg, busy, capture, cell_size, close, drag_move, drag_resize, draw, fg, focus, font,
    font_info, fullscreen, grab, headless, held_key, host_main_thread, icon, input_mode, inset,
    key_repeat, key_repeats, latency, latency_histograms, ligatures, line_height, maximize,
    minimize, next_texture_id, open, register_family, register_installed_family, render_frame,
    settle, size, size_px, sound, sound_free, text_aa, text_contrast, texture, texture_free, title,
    window_ids,
};
