//! The windowed frontend: these primitives translate Brood `Value`s to and from the plain
//! `gui::Op` / `gui::Key` / `gui::Face` the backend speaks.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use crate::builtins::args::{arg, expect_bigint, expect_int, expect_string};
use crate::builtins::bytes::collect_bytes;
use crate::builtins::display::*;
use crate::builtins::numeric::num_to_f64;

mod fragments;

// ---- the GUI frontend (ADR-046, feature "gui") ------------------------------
//
// `gui-*` mirror `term-*`: a second frontend that paints the *same* render-op
// protocol (a frame is the same Brood data) to a native window and reads keys
// back in the same encoding. The window/loop machinery lives in `crate::host::gui`
// (behind the `gui` feature); these primitives just translate Brood `Value`s ⇄
// the plain `gui::Op`/`gui::Key`/`gui::Face` the backend speaks. A composite
// "broadcast" display in std/tool/observer.blsp drives term + gui (+ remote later)
// from one frame — so the frontends can't drift. Without the `gui` feature the
// backend functions return an error naming how to build it in (see `gui::NOT_COMPILED`).

/// Resolve a face map (`{:fg :red :bg :blue :bold true :reverse true}`) into the
/// plain `gui::Face` the backend renders. A non-map face is the default face.
pub(in crate::builtins) fn gui_face(heap: &Heap, face: Value) -> crate::host::gui::Face {
    let mut f = crate::host::gui::Face::default();
    let Some(read) = read_face(heap, face) else {
        return f;
    };
    let Value::Map(id) = face else { return f };
    let k = &*FACE_KEYS;
    f.fg = read.fg.and_then(|v| face_rgb(heap, v));
    f.bg = read.bg.and_then(|v| face_rgb(heap, v));
    f.bold = read.bold;
    f.italic = read.italic;
    f.reverse = read.reverse;
    if let Some(underline) = read.underline {
        f.underline = true;
        f.underline_rgb = underline.colour.and_then(|v| face_rgb(heap, v));
        f.underline_wave = underline.wave;
    }
    // `:family` is a keyword naming a registered font family; carry its interned
    // id so the renderer can pick the matching font set (`:mono` / unknown → default).
    f.family = match heap.map_get(id, k.family) {
        Some(Value::Keyword(s)) => Some(s),
        _ => None,
    };
    // `:scale n` (GUI only, ADR-079): draw the op's text n× larger, in an n×n cell
    // block. Clamp to 1..=GUI_MAX_SCALE — a non-positive value falls back to the
    // default 1, and the cap bounds the per-op framebuffer work + glyph cache.
    if let Some(Value::Int(n)) = heap.map_get(id, k.scale) {
        f.scale = n.clamp(1, GUI_MAX_SCALE as i64) as u16;
    }
    f
}

/// Upper bound on a face's `:scale` (a `:scale 3` glyph already covers 9 cells; the
/// cap keeps a stray huge value from blowing up framebuffer work + the glyph cache).
const GUI_MAX_SCALE: u16 = 16;

/// Read a window-id argument (the integer `gui-open` returned) for the windowed
/// primitives. Negative ids clamp to 0 (no such window → a clean "not open" error).
pub(in crate::builtins) fn gui_window_id(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<u64, LispError> {
    Ok(expect_int(heap, who, v)?.max(0) as u64)
}

/// `(gui-open)` / `(gui-open title)` — open a new native window and return its integer
/// id, optionally with a title-bar string (else a default `brood observer #id`). Its
/// key/mouse input is delivered to the **calling process's mailbox** (ADR-059), so the
/// observer parks in `(receive)` rather than pinning a worker in a blocking poll.
/// Starts the GUI thread on the first call; each call is an independent window.
pub(in crate::builtins) fn gui_open(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let spec = window_spec(args, heap)?;
    let id =
        crate::host::gui::open(crate::process::self_pid(), spec).map_err(LispError::runtime)?;
    Ok(Value::int(id as i64))
}

/// The `WindowSpec` `gui-open`'s arguments ask for: its title, size and opts map, read and
/// checked here, before any window exists.
fn window_spec(args: &[Value], heap: &Heap) -> Result<crate::host::gui::WindowSpec, LispError> {
    let title = match arg(args, 0) {
        Value::Nil => None,
        v => Some(expect_string(heap, "%gui-open", v)?),
    };
    let size = match arg(args, 1) {
        Value::Nil => None,
        w => Some((
            expect_int(heap, "%gui-open", w)? as f64,
            expect_int(heap, "%gui-open", arg(args, 2))? as f64,
        )),
    };
    // 4th arg: an opts map of build-time window attributes.
    //   :decorations false — a borderless window, for an app that draws its own chrome
    //     and would otherwise sit under a redundant OS title bar.
    //   :app-id "name"     — the desktop application id (Wayland `app_id` / X11
    //     `WM_CLASS`), matched against the installed `name.desktop` entry; without it
    //     the desktop can't identify the window and paints its generic fallback icon.
    // Absent (every pre-existing caller) means decorated, with no app id — as before.
    let opts = match arg(args, 3) {
        Value::Map(id) => Some(id),
        _ => None,
    };
    let decorations = match opts.and_then(|id| heap.map_get(id, value::kw("decorations"))) {
        Some(v) => crate::eval::truthy(v),
        None => true,
    };
    // An EMPTY app id is rejected rather than passed on: the desktop would take it as
    // an id that matches no entry, so the window would open with the generic fallback
    // icon and nothing would say why. `nil` (or an absent key) is the honest way to
    // say "no id" and stays allowed.
    let app_id = match opts.and_then(|id| heap.map_get(id, value::kw("app-id"))) {
        Some(Value::Nil) | None => None,
        Some(v) => {
            let name = expect_string(heap, "%gui-open", v)?;
            if name.is_empty() {
                return Err(LispError::runtime(
                    "%gui-open: :app-id must not be empty — it names the .desktop entry \
                     the desktop matches this window to (pass nil for no app id)",
                ));
            }
            Some(name)
        }
    };
    //   :input :pixels     — mouse messages carry the pointer in physical pixels and
    //     `[:resize w h]` the pixel size; `:move`/`:drag` fire per pixel of motion.
    //   :vsync true        — present in step with the monitor (FIFO; the GPU target only).
    //   :vsync false       — the work-bound rate, tearing allowed (IMMEDIATE).
    //   absent             — low latency without tearing (MAILBOX, else FIFO).
    let pixel_input = matches!(
        opts.and_then(|id| heap.map_get(id, value::kw("input"))),
        Some(Value::Keyword(s)) if s == value::intern("pixels")
    );
    let vsync = opts
        .and_then(|id| heap.map_get(id, value::kw("vsync")))
        .filter(|v| !matches!(v, Value::Nil))
        .map(crate::eval::truthy);
    //   :key-repeat :os    — the OS auto-repeat reaches the app as presses of the held key
    //     (at most one undelivered at a time); `:app` (the default) drops it (ADR-086).
    //   :ime false         — no input method in this window (on by default).
    //   :wheel :glide      — a wheel notch glides; `:lines` (the default) scrolls its
    //     three lines at once (ADR-441).
    let keyword_option = |key: &str, allowed: &[&str]| -> Result<Option<String>, LispError> {
        match opts.and_then(|id| heap.map_get(id, value::kw(key))) {
            None | Some(Value::Nil) => Ok(None),
            Some(Value::Keyword(name)) if allowed.contains(&value::symbol_name_ref(name)) => {
                Ok(Some(value::symbol_name(name)))
            }
            Some(_) => Err(LispError::runtime(format!(
                "%gui-open: :{key} is one of {}",
                allowed
                    .iter()
                    .map(|name| format!(":{name}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ))),
        }
    };
    let key_repeat = match keyword_option("key-repeat", &["app", "os"])?.as_deref() {
        Some("os") => crate::host::gui::KeyRepeat::Os,
        _ => crate::host::gui::KeyRepeat::App,
    };
    let wheel = match keyword_option("wheel", &["lines", "glide"])?.as_deref() {
        Some("glide") => crate::host::gui::WheelMode::Glide,
        _ => crate::host::gui::WheelMode::Lines,
    };
    let ime = match opts.and_then(|id| heap.map_get(id, value::kw("ime"))) {
        Some(v) => crate::eval::truthy(v),
        None => true,
    };
    //   :option-meta :left  — which Option key is Meta on macOS; the other composes
    //     (`:right`, `:both`, `:none`; ADR-541). Elsewhere Alt is Meta and AltGr AltGr.
    let option_meta =
        match keyword_option("option-meta", &["left", "right", "both", "none"])?.as_deref() {
            Some("right") => crate::host::gui::OptionMeta::Right,
            Some("both") => crate::host::gui::OptionMeta::Both,
            Some("none") => crate::host::gui::OptionMeta::None,
            _ => crate::host::gui::OptionMeta::Left,
        };
    //   :present-events true — `[:presented t-ns]` after each successful present, the
    //     clock a game paces its frames by; off unless asked.
    let present_events = opts
        .and_then(|id| heap.map_get(id, value::kw("present-events")))
        .is_some_and(crate::eval::truthy);
    Ok(crate::host::gui::WindowSpec {
        title,
        size,
        decorations,
        app_id,
        pixel_input,
        vsync,
        key_repeat,
        ime,
        wheel,
        option_meta,
        present_events,
    })
}

/// `(%gui-size-px id)` — window `id`'s inner size as `[w h]` in PHYSICAL pixels.
pub(in crate::builtins) fn gui_size_px(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-size-px", arg(args, 0))?;
    let (w, h) = crate::host::gui::size_px(id).map_err(LispError::runtime)?;
    Ok(heap.alloc_vector2(Value::int(w as i64), Value::int(h as i64)))
}

/// `(%gui-refresh-hz id)` — the refresh rate of window `id`'s monitor in hertz, or nil
/// when the platform does not say (and headless).
pub(in crate::builtins) fn gui_refresh_hz(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-refresh-hz", arg(args, 0))?;
    let hertz = crate::host::gui::refresh_hz(id).map_err(LispError::runtime)?;
    Ok(hertz.map_or(Value::Nil, Value::float))
}

/// `(%gui-settle id)` — window `id`'s app finished a turn with nothing new to draw (ADR-444).
pub(in crate::builtins) fn gui_settle(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-settle", arg(args, 0))?;
    crate::host::gui::settle(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(%gui-latency id)` — window `id`'s recent input-to-present samples, oldest first, as
/// maps of milliseconds (and `:input`, `:key` or `:pointer`, ADR-612): `:total` (input to present) and `:paint` (the paint's share),
/// plus the GPU paint's `:encode` `:upload` `:acquire` `:submit` `:present` phases when
/// the GPU target drew it (ADR-440). GC-safe: a builtin never fires GC mid-execution,
/// so the maps stay live until the vector holds them.
pub(in crate::builtins) fn gui_latency(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-latency", arg(args, 0))?;
    let samples = crate::host::gui::latency(id).map_err(LispError::runtime)?;
    let ms = |us: u64| Value::float(us as f64 / 1000.0);
    let mut maps = Vec::with_capacity(samples.len());
    for sample in samples {
        let mut pairs = vec![
            (value::kw("total"), ms(sample.total_us)),
            (value::kw("paint"), ms(sample.paint_us)),
            (
                value::kw("input"),
                value::kw(match sample.input {
                    crate::host::gui::InputKind::Key => "key",
                    crate::host::gui::InputKind::Pointer => "pointer",
                }),
            ),
        ];
        if let Some(split) = sample.split {
            for (name, us) in crate::host::gui::PAINT_PHASES.into_iter().zip(split) {
                pairs.push((value::kw(name), ms(us)));
            }
        }
        maps.push(heap.map_from_pairs(pairs));
    }
    Ok(heap.alloc_vector(maps))
}

/// `(%gui-latency-histogram id)` — window `id`'s input-to-present distribution since it
/// opened, `{:key h :pointer h}`, each `h` a `{:count n :max-us n :buckets {upper-us n}}` —
/// a bucket's key its largest value in microseconds, so two histograms add bucket by bucket
/// (ADR-612). Percentiles are std's (`gui/latency-percentiles`), over this data. GC-safe as
/// `gui_latency` is.
pub(in crate::builtins) fn gui_latency_histogram(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let id = gui_window_id(heap, "%gui-latency-histogram", arg(args, 0))?;
    let (keys, pointer) = crate::host::gui::latency_histograms(id).map_err(LispError::runtime)?;
    let count = |n: u64| Value::int(n as i64);
    let mut histogram = |h: &crate::host::gui::LatencyHistogram| {
        let buckets: Vec<_> = h
            .buckets()
            .map(|(upper_us, n)| (count(upper_us), count(n)))
            .collect();
        let buckets = heap.map_from_pairs(buckets);
        heap.map_from_pairs(vec![
            (value::kw("count"), count(h.count)),
            (value::kw("max-us"), count(h.max_us)),
            (value::kw("buckets"), buckets),
        ])
    };
    let keys = histogram(&keys);
    let pointer = histogram(&pointer);
    Ok(heap.map_from_pairs(vec![
        (value::kw("key"), keys),
        (value::kw("pointer"), pointer),
    ]))
}

/// `(%gui-key-repeat? id)` — whether the last key press window `id`'s app took from its
/// mailbox was an OS auto-repeat (ADR-612); false for a window not open.
pub(in crate::builtins) fn gui_key_repeat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-key-repeat?", arg(args, 0))?;
    Ok(Value::boolean(crate::host::gui::key_repeat(id)))
}
/// `(gui-key-repeats id)` — how window `id`'s held keys have repeated since it opened:
/// `{:delivered n :dropped n :delay ms :interval ms}`, `:delay` and `:interval` the
/// desktop's as the last key that repeated showed them (nil before any has).
pub(in crate::builtins) fn gui_key_repeats(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let id = gui_window_id(heap, "%gui-key-repeats", arg(args, 0))?;
    let repeats = crate::host::gui::key_repeats(id).map_err(LispError::runtime)?;
    let ms = |us: Option<u64>| us.map_or(Value::nil(), |us| Value::float(us as f64 / 1000.0));
    let count = |n: u64| Value::int(n as i64);
    Ok(heap.map_from_pairs(vec![
        (value::kw("delivered"), count(repeats.delivered)),
        (value::kw("dropped"), count(repeats.dropped)),
        (value::kw("delay"), ms(repeats.delay_us)),
        (value::kw("interval"), ms(repeats.interval_us)),
    ]))
}

/// `(%gui-texture id tex rgba w h filter)` — upload `rgba` (`w*h*4` bytes, row-major
/// straight alpha, a bytes value or a vector of ints) as texture `tex` of window `id`,
/// sampled per `filter` (`:nearest` or `:smooth`, ADR-783). The handle
/// is the CALLER's (Brood allocates it — `gui/texture` — so a sprite sheet's size can be
/// remembered beside it without a round trip here), which is why this takes it rather
/// than returns it. The dimensions are validated against the byte count before any
/// narrowing cast, as `gui-icon!` does: a mismatch is a clean error, never a texture
/// that lies about its size.
pub(in crate::builtins) fn gui_texture(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-texture", arg(args, 0))?;
    let tex_i = expect_int(heap, "%gui-texture", arg(args, 1))?;
    let rgba = collect_bytes("%gui-texture", arg(args, 2), heap)?;
    let w_i = expect_int(heap, "%gui-texture", arg(args, 3))?;
    let h_i = expect_int(heap, "%gui-texture", arg(args, 4))?;
    let filter = texture_filter(heap, arg(args, 5))?;
    let Ok(tex) = u32::try_from(tex_i) else {
        return Err(LispError::runtime(format!(
            "%gui-texture: texture handle must be a non-negative int, got {tex_i}"
        )));
    };
    let (w, h) = match (u32::try_from(w_i), u32::try_from(h_i)) {
        (Ok(w), Ok(h))
            if w > 0
                && h > 0
                && (w as u64)
                    .checked_mul(h as u64)
                    .and_then(|n| n.checked_mul(4))
                    == Some(rgba.len() as u64) =>
        {
            (w, h)
        }
        _ => {
            return Err(LispError::runtime(format!(
                "%gui-texture: {w_i}x{h_i} needs {} bytes of rgba, got {}",
                (w_i.max(0) as u64)
                    .saturating_mul(h_i.max(0) as u64)
                    .saturating_mul(4),
                rgba.len()
            )));
        }
    };
    crate::host::gui::texture(id, tex, rgba, w, h, filter).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// A texture's sampling filter from its keyword: `:nearest`, `:smooth` (ADR-783) or `:coverage`
/// (smooth, its mips keeping their coverage). Anything else is a clean error naming them, never
/// a silent nearest.
fn texture_filter(heap: &Heap, given: Value) -> Result<crate::host::gui::TextureFilter, LispError> {
    use crate::host::gui::TextureFilter;
    match given {
        Value::Keyword(name) if value::symbol_name_ref(name) == "nearest" => {
            Ok(TextureFilter::Nearest)
        }
        Value::Keyword(name) if value::symbol_name_ref(name) == "smooth" => {
            Ok(TextureFilter::Smooth)
        }
        Value::Keyword(name) if value::symbol_name_ref(name) == "coverage" => {
            Ok(TextureFilter::Coverage)
        }
        other => Err(LispError::wrong_type(
            heap,
            "%gui-texture",
            "a filter, :nearest, :smooth or :coverage",
            other,
        )),
    }
}

/// `(%gui-sound id snd rate channels pcm)` — keep `pcm` (interleaved PCM16 little-endian
/// bytes: a bytes value or a vector of 0-255 ints) at `rate` Hz with `channels` channels as
/// sound `snd` of window `id`, for its `[:sound …]` ops. The handle is the caller's
/// (`gui/sound` allocates it in Brood beside the sound's length), as `%gui-texture`'s is.
pub(in crate::builtins) fn gui_sound(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-sound", arg(args, 0))?;
    let snd_i = expect_int(heap, "%gui-sound", arg(args, 1))?;
    let rate_i = expect_int(heap, "%gui-sound", arg(args, 2))?;
    let channels_i = expect_int(heap, "%gui-sound", arg(args, 3))?;
    let pcm = collect_bytes("%gui-sound", arg(args, 4), heap)?;
    let Ok(snd) = u32::try_from(snd_i) else {
        return Err(LispError::runtime(format!(
            "%gui-sound: sound handle must be a non-negative int, got {snd_i}"
        )));
    };
    if !(1..=384_000).contains(&rate_i) || !(1..=2).contains(&channels_i) {
        return Err(LispError::runtime(format!(
            "%gui-sound: rate must be 1..384000 Hz and channels 1 or 2, got {rate_i} Hz × {channels_i}"
        )));
    }
    if pcm.len() % 2 != 0 {
        return Err(LispError::runtime(format!(
            "%gui-sound: PCM16 needs an even byte count, got {}",
            pcm.len()
        )));
    }
    crate::host::gui::sound(id, snd, rate_i as u32, channels_i as u16, pcm)
        .map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(%gui-sound-free id snd)` — forget sound `snd` of window `id`.
pub(in crate::builtins) fn gui_sound_free(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-sound-free", arg(args, 0))?;
    let snd_i = expect_int(heap, "%gui-sound-free", arg(args, 1))?;
    let Ok(snd) = u32::try_from(snd_i) else {
        return Ok(Value::nil());
    };
    crate::host::gui::sound_free(id, snd).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(%gui-input! id mode)` — deliver window `id`'s mouse input in `:pixels` or `:cells`
/// from now on (the `{:input …}` open option, switchable); the window then reports its
/// size in that unit. Anything but `:pixels` means cells.
pub(in crate::builtins) fn gui_input(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-input!", arg(args, 0))?;
    let pixels = matches!(arg(args, 1), Value::Keyword(s) if s == value::intern("pixels"));
    crate::host::gui::input_mode(id, pixels).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(%gui-texture-free id tex)` — release texture `tex` of window `id`.
pub(in crate::builtins) fn gui_texture_free(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let id = gui_window_id(heap, "%gui-texture-free", arg(args, 0))?;
    let tex_i = expect_int(heap, "%gui-texture-free", arg(args, 1))?;
    let Ok(tex) = u32::try_from(tex_i) else {
        return Ok(Value::nil());
    };
    crate::host::gui::texture_free(id, tex).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(%gui-compiled?)` — was this runtime built with the GUI backend? The predicate an
/// app asks BEFORE a `gui/*` call it means to skip headlessly, instead of calling and
/// catching — a catch around a `gui/*` call swallows an unbound symbol just as readily
/// as the missing backend, hiding a rename.
pub(in crate::builtins) fn gui_compiled_p(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(cfg!(feature = "gui")))
}

/// `(gui-close id)` — close window `id` (the teardown for `gui-open`; idempotent).
pub(in crate::builtins) fn gui_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-close", arg(args, 0))?;
    fragments::forget(id);
    crate::host::gui::close(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-title! id text)` — set window `id`'s OS title-bar text at runtime.
pub(in crate::builtins) fn gui_title(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-title!", arg(args, 0))?;
    let title = expect_string(heap, "%gui-title!", arg(args, 1))?;
    crate::host::gui::title(id, title).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-icon! id rgba w h)` — set window `id`'s taskbar/title-bar icon from raw RGBA
/// pixels (a vector of `w*h*4` byte ints, row-major).
pub(in crate::builtins) fn gui_icon(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-icon!", arg(args, 0))?;
    let w_i = expect_int(heap, "%gui-icon!", arg(args, 2))?;
    let h_i = expect_int(heap, "%gui-icon!", arg(args, 3))?;
    let rgba: Vec<u8> = match arg(args, 1) {
        Value::Vector(vid) => heap
            .vector(vid)
            .iter()
            .map(|v| match v {
                Value::Int(i) => *i as u8,
                _ => 0,
            })
            .collect(),
        _ => {
            return Err(LispError::runtime(
                "%gui-icon!: rgba must be a vector of bytes".to_string(),
            ));
        }
    };
    // Validate BEFORE the narrowing cast, and against the buffer we actually have:
    // `expect_int(..)? as u32` truncates silently (4294967296 arrives as 0), and the
    // backend reads `w * h * 4` bytes out of `rgba`, so a dimension that doesn't match
    // the pixel count is a malformed icon at best. A caller mistake must be a clean
    // catchable error, never a cast that lies about the size.
    // `checked_mul`, not `*`: `w * h * 4` at u64 still overflows for two dimensions near
    // `u32::MAX` (their product fits, the ×4 does not), and an overflow in the *check* is a
    // debug-assertions panic on whatever thread this runs on — the exact class of
    // uncatchable failure this validation was added to prevent. An overflow is `None`,
    // which can never equal `Some(len)`, so it rejects.
    let (w, h) = match (u32::try_from(w_i), u32::try_from(h_i)) {
        (Ok(w), Ok(h))
            if (w as u64)
                .checked_mul(h as u64)
                .and_then(|px| px.checked_mul(4))
                == Some(rgba.len() as u64) =>
        {
            (w, h)
        }
        _ => {
            return Err(LispError::runtime(format!(
                "%gui-icon!: w*h*4 must equal the rgba byte count (w={w_i}, h={h_i}, {} bytes)",
                rgba.len()
            )));
        }
    };
    crate::host::gui::icon(id, rgba, w, h).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-focus id)` — raise window `id` and give it OS keyboard focus (un-minimising
/// it). Lets an app surface an already-open singleton window instead of opening a
/// duplicate. Errors only if `id` isn't a live window.
pub(in crate::builtins) fn gui_focus(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-focus", arg(args, 0))?;
    crate::host::gui::focus(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-grab-cursor id on)` — confine the pointer to window `id` while `on` is
/// truthy, release it otherwise.
pub(in crate::builtins) fn gui_grab_cursor(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let id = gui_window_id(heap, "%gui-grab-cursor", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::host::gui::grab(id, on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(%gui-busy id on)` — show window `id`'s busy cursor for a loop on another node (ADR-491).
pub(in crate::builtins) fn gui_busy(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-busy", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::host::gui::busy(id, on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-fullscreen! id on)` — make window `id` borderless-fullscreen (`on` truthy)
/// or restore it to a normal window.
pub(in crate::builtins) fn gui_fullscreen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-fullscreen!", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::host::gui::fullscreen(id, on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-minimize! id)` — iconify window `id`. The counterpart of `gui-maximize!`
/// for an app that draws its own window controls, which a borderless window
/// (`gui-open` with `{:decorations false}`) must.
pub(in crate::builtins) fn gui_minimize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-minimize!", arg(args, 0))?;
    crate::host::gui::minimize(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-drag-move id)` — hand window `id` to the window manager for an interactive
/// move, for the rest of the currently-held press.
///
/// This is what a borderless window needs to stay movable: with no OS title bar
/// there is nothing to grab, so the app nominates a region of its own chrome (a
/// browser's tab strip) and calls this when a press lands there.
pub(in crate::builtins) fn gui_drag_move(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-drag-move", arg(args, 0))?;
    crate::host::gui::drag_move(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-drag-resize id dir)` — hand window `id` to the window manager for an
/// interactive resize from `dir`: `:north` `:south` `:east` `:west` `:north-east`
/// `:north-west` `:south-east` `:south-west`. The window-frame counterpart of
/// `gui-drag-move`, for a borderless window that draws its own edges.
pub(in crate::builtins) fn gui_drag_resize(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let id = gui_window_id(heap, "%gui-drag-resize", arg(args, 0))?;
    let dir = match arg(args, 1) {
        Value::Keyword(sym) => value::symbol_name(sym),
        v => expect_string(heap, "%gui-drag-resize", v)?,
    };
    crate::host::gui::drag_resize(id, &dir).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-maximize! id on)` — maximise window `id` (`on` truthy) or restore it,
/// keeping the title bar / decorations (unlike fullscreen).
pub(in crate::builtins) fn gui_maximize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-maximize!", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::host::gui::maximize(id, on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-size id)` — window `id`'s size as `[cols rows]` (character cells), same
/// shape as `term-size`.
pub(in crate::builtins) fn gui_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-size", arg(args, 0))?;
    let (cols, rows) = crate::host::gui::size(id).map_err(LispError::runtime)?;
    Ok(heap.alloc_vector2(Value::int(cols as i64), Value::int(rows as i64)))
}

pub(in crate::builtins) fn gui_cell_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-cell-size", arg(args, 0))?;
    let px = match arg(args, 1) {
        Value::Nil => None,
        v => Some(num_to_f64(heap, "%gui-cell-size", v)? as f32),
    };
    let (cell_w, cell_h) = crate::host::gui::cell_size(id, px).map_err(LispError::runtime)?;
    Ok(heap.alloc_vector2(Value::int(cell_w as i64), Value::int(cell_h as i64)))
}

/// `(gui-held-key id)` — the keystroke window `id` currently sees as physically held (the
/// same value its press delivered: a key, or `[:text "…"]` for a keystroke that typed
/// several characters), or nil when none. Tracked from press/release
/// transitions, not winit's unreliable `ke.repeat`, so it's the source of truth a
/// consumer-paced key repeat polls to stop the instant the key is up — making a
/// missed key-up unable to cause runaway repeat (ADR-086).
pub(in crate::builtins) fn gui_held_key(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-held-key", arg(args, 0))?;
    match crate::host::gui::held_key(id).map_err(LispError::runtime)? {
        Some(crate::host::gui::Keystroke::Key(k)) => Ok(gui_key_to_value(heap, k)),
        Some(crate::host::gui::Keystroke::Text(text)) => {
            let text = heap.alloc_string(&text);
            Ok(heap.alloc_vector2(value::kw("text"), text))
        }
        None => Ok(Value::nil()),
    }
}

/// `(gui-draw id frame)` — paint a frame (the same op vector `term-draw` takes) to
/// window `id`. Parses the ops into plain `gui::Op`s (it has heap access) and ships
/// them to the GUI thread. Unknown ops are skipped (forward-compatible).
/// Interned tag symbols shared between `gui_draw` and `parse_gui_ops`.
struct GuiOpTags {
    clear_t: value::Symbol,
    memo_t: value::Symbol,
    text_t: value::Symbol,
    cursor_t: value::Symbol,
    cursor_zone_t: value::Symbol,
    col_resize_t: value::Symbol,
    row_resize_t: value::Symbol,
    pointer_t: value::Symbol,
    vspans_t: value::Symbol,
    cells_t: value::Symbol,
    cells_rgb_t: value::Symbol,
    scroll_region_t: value::Symbol,
    cell_region_t: value::Symbol,
    rect_t: value::Symbol,
    image_t: value::Symbol,
    frect_t: value::Symbol,
    quad_t: value::Symbol,
    sprite_t: value::Symbol,
    text_px_t: value::Symbol,
    sound_t: value::Symbol,
    music_t: value::Symbol,
    ambience_t: value::Symbol,
    ambient_t: value::Symbol,
    light_t: value::Symbol,
    overlay_t: value::Symbol,
    bloom_t: value::Symbol,
    crt_t: value::Symbol,
    emitter_t: value::Symbol,
    scene_t: value::Symbol,
    burst_t: value::Symbol,
    seam_t: value::Symbol,
    above_seam_t: value::Symbol,
    translate_t: value::Symbol,
    /// The first op the parse refused rather than skipped, and why: `%gui-draw` raises it
    /// instead of drawing the frame.
    refusal: std::cell::RefCell<Option<String>>,
}
impl GuiOpTags {
    fn new() -> Self {
        Self {
            clear_t: value::intern("clear"),
            memo_t: value::intern("ui/memo"),
            text_t: value::intern("text"),
            cursor_t: value::intern("cursor"),
            cursor_zone_t: value::intern("cursor-zone"),
            col_resize_t: value::intern("col-resize"),
            row_resize_t: value::intern("row-resize"),
            pointer_t: value::intern("pointer"),
            vspans_t: value::intern("vspans"),
            cells_t: value::intern("cells"),
            cells_rgb_t: value::intern("cells-rgb"),
            scroll_region_t: value::intern("scroll-region"),
            cell_region_t: value::intern("cell-region"),
            rect_t: value::intern("rect"),
            image_t: value::intern("image"),
            frect_t: value::intern("frect"),
            quad_t: value::intern("quad"),
            sprite_t: value::intern("sprite"),
            text_px_t: value::intern("text-px"),
            sound_t: value::intern("sound"),
            music_t: value::intern("music"),
            ambience_t: value::intern("ambience"),
            ambient_t: value::intern("ambient"),
            light_t: value::intern("light"),
            overlay_t: value::intern("overlay"),
            bloom_t: value::intern("bloom"),
            crt_t: value::intern("crt"),
            emitter_t: value::intern("emitter"),
            scene_t: value::intern("scene"),
            burst_t: value::intern("burst"),
            seam_t: value::intern("seam"),
            above_seam_t: value::intern("above-seam"),
            translate_t: value::intern("translate"),
            refusal: std::cell::RefCell::new(None),
        }
    }

    /// Keep `why` as the frame's refusal, unless an op before refused it already.
    fn refuse(&self, why: String) {
        self.refusal.borrow_mut().get_or_insert(why);
    }

    /// Why the parse refused the frame, if it did, as the error `who` raises.
    fn refused(&self, who: &str) -> Result<(), LispError> {
        match self.refusal.borrow_mut().take() {
            Some(why) => Err(LispError::runtime(format!("{who}: {why}"))),
            None => Ok(()),
        }
    }
}

/// `text` drawn from column `col`, clipped at the window's left edge as the terminal's
/// `put_text` clips it: the clusters left of column 0 dropped and a wide one the edge cuts
/// drawn as a blank, so the text that is left starts at column 0 — `None` when none is. At a
/// column of 0 or more, the text as it is. The terminal and the window agree on where a
/// `[:text row -2 …]` lands, where clamping the column painted the whole string from 0.
fn clip_text_left(text: String, col: i64) -> Option<String> {
    use unicode_segmentation::UnicodeSegmentation;
    if col >= 0 {
        return Some(text);
    }
    let mut at = col;
    for (start, cluster) in text.grapheme_indices(true) {
        if at >= 0 {
            return Some(text[start..].to_string());
        }
        let end = at + crate::host::text_width::cluster_cells(cluster) as i64;
        if end > 0 {
            // a wide cluster cut by the edge: its visible cell is a blank
            let rest = &text[start + cluster.len()..];
            return Some(format!("{}{rest}", " ".repeat(end as usize)));
        }
        at = end;
    }
    None
}

/// Parse a sequence of already-tagged op tuples into `gui::Op`s. Called recursively
/// for the inner ops of a `ScrollRegion`. Malformed or unknown ops are skipped.
fn parse_gui_ops(
    heap: &Heap,
    parsed: Vec<(value::Symbol, Vec<Value>)>,
    tags: &GuiOpTags,
    pass: &mut fragments::FragmentPass,
) -> Vec<crate::host::gui::Op> {
    // A ratio or decimal reads as its float — `/` is exact in Brood (ADR-196), so a
    // frame built with `(/ w 2)` carries a ratio, which must not silently read as 0.
    let num = |v: Value| -> f32 {
        match v {
            Value::Int(n) => n as f32,
            Value::Float(f) => f as f32,
            Value::Ratio(_) | Value::Decimal(_) | Value::BigInt(_) => {
                num_to_f64(heap, "%gui-draw", v)
                    .map(|f| f as f32)
                    .unwrap_or(0.0)
            }
            _ => 0.0,
        }
    };
    let mut ops = Vec::with_capacity(parsed.len());
    for (tag, parts) in parsed {
        if tag == tags.clear_t {
            ops.push(crate::host::gui::Op::Clear);
        } else if tag == tags.memo_t {
            // `[:ui/memo key deps ops reused?]` — a memoised view fragment, drawn with its
            // marker by a display that takes them (ADR-470): last frame's parse when the
            // loop reused it, else its ops parsed here.
            fragments::parse_marker(heap, &parts, pass, &mut ops, |heap, items, pass| {
                parse_gui_ops(heap, tagged_ops(heap, items), tags, pass)
            });
        } else if tag == tags.cursor_t {
            let Ok(row_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(col_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            // off the window's top or left edge: no caret, as the terminal skips it
            if row_i < 0 || col_i < 0 {
                continue;
            }
            let style = cursor_style_from(parts.get(3).copied().unwrap_or(Value::nil()));
            // `[:cursor row col style color]`: the colour a face colour takes (a
            // keyword, `[r g b]` or `"#rrggbb"`); absent, nil or unreadable is the
            // painters' default. The style slot may be nil to colour a block.
            let color = parts.get(4).and_then(|v| face_rgb(heap, *v));
            // `[:cursor row col style colour blink]`: a blink the window animates itself
            // (ADR-469), `{:interval ms :blinks n}`; absent or unreadable is steady.
            let blink = parts.get(5).and_then(|v| cursor_blink_from(heap, *v));
            // `[:cursor row col style colour blink under face]`: the text under the caret and
            // the face it is painted in, which a block draws over its solid fill (ADR-515);
            // absent, nil or not a string is none, and an absent face is the default one.
            let under_face = gui_face(heap, parts.get(7).copied().unwrap_or(Value::nil()));
            let under = parts
                .get(6)
                .and_then(|v| expect_string(heap, "%gui-draw", *v).ok())
                .filter(|s| !s.is_empty())
                .map(|s| (s, under_face));
            ops.push(crate::host::gui::Op::Cursor {
                row: clamp_u16(row_i),
                col: clamp_u16(col_i),
                style,
                color,
                blink,
                under,
            });
        } else if tag == tags.rect_t {
            let Ok(row_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(col_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let Ok(w_i) = expect_int(heap, "%gui-draw", arg(&parts, 3)) else {
                continue;
            };
            let Ok(h_i) = expect_int(heap, "%gui-draw", arg(&parts, 4)) else {
                continue;
            };
            // the part left of or above the window is cut off, as the terminal's `fill` clips
            // it — a clamped origin moved the whole panel down or right by what was off-screen
            let (row_i, h_i) = (row_i.max(0), h_i.saturating_add(row_i.min(0)));
            let (col_i, w_i) = (col_i.max(0), w_i.saturating_add(col_i.min(0)));
            if w_i <= 0 || h_i <= 0 {
                continue;
            }
            let face = gui_face(heap, parts.get(5).copied().unwrap_or(Value::nil()));
            // Optional 7th element: a corner radius in cell units. Absent (every
            // existing frame) means 0.0 — a square panel, byte-for-byte the old
            // behaviour.
            let radius = match parts.get(6).copied() {
                Some(v @ (Value::Int(_) | Value::Float(_))) => num(v),
                _ => 0.0,
            };
            ops.push(crate::host::gui::Op::Rect {
                row: clamp_u16(row_i),
                col: clamp_u16(col_i),
                w: clamp_u16(w_i),
                h: clamp_u16(h_i),
                face,
                radius,
            });
        } else if tag == tags.image_t {
            // `[:image row col w h source opts]` (ADR-686): decoded here, through the content
            // cache, so the GUI thread only resamples and paints; one that does not read is skipped.
            if let Some(image) = read_image_op(heap, &parts) {
                ops.push(crate::host::gui::Op::Image {
                    row: clamp_u16(image.row),
                    col: clamp_u16(image.col),
                    w: clamp_u16(image.w),
                    h: clamp_u16(image.h),
                    picture: crate::host::picture::PictureRef(image.picture),
                    fit: image.fit,
                    anchor: image.anchor,
                });
            }
        } else if tag == tags.frect_t {
            let x = num(arg(&parts, 1));
            let y = num(arg(&parts, 2));
            let w = num(arg(&parts, 3));
            let h = num(arg(&parts, 4));
            let face = gui_face(heap, parts.get(5).copied().unwrap_or(Value::nil()));
            let opacity = match parts.get(6).copied() {
                Some(v @ (Value::Int(_) | Value::Float(_))) => num(v),
                _ => 1.0,
            };
            let radius = num(parts.get(7).copied().unwrap_or(Value::nil()));
            ops.push(crate::host::gui::Op::FRect {
                x,
                y,
                w,
                h,
                face,
                opacity,
                radius,
            });
        } else if tag == tags.text_t {
            let Ok(row_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(col_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let Ok(s) = expect_string(heap, "%gui-draw", arg(&parts, 3)) else {
                continue;
            };
            // a row above the window paints nothing, and a column left of it clips the
            // text at the edge, as the terminal's `put_text` does (never clamped to 0, which
            // painted the op over row 0 or from column 0)
            if row_i < 0 {
                continue;
            }
            let Some(s) = clip_text_left(s, col_i) else {
                continue;
            };
            let face = gui_face(heap, parts.get(4).copied().unwrap_or(Value::nil()));
            ops.push(crate::host::gui::Op::Text {
                row: clamp_u16(row_i),
                col: clamp_u16(col_i),
                s,
                face,
            });
        } else if tag == tags.cursor_zone_t {
            let Ok(x_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(y_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let Ok(w_i) = expect_int(heap, "%gui-draw", arg(&parts, 3)) else {
                continue;
            };
            let Ok(h_i) = expect_int(heap, "%gui-draw", arg(&parts, 4)) else {
                continue;
            };
            let shape = match parts.get(5) {
                Some(Value::Keyword(s)) if *s == tags.col_resize_t => {
                    Some(crate::host::gui::CursorShape::ColResize)
                }
                Some(Value::Keyword(s)) if *s == tags.row_resize_t => {
                    Some(crate::host::gui::CursorShape::RowResize)
                }
                Some(Value::Keyword(s)) if *s == tags.pointer_t => {
                    Some(crate::host::gui::CursorShape::Pointer)
                }
                _ => None,
            };
            if let Some(shape) = shape {
                ops.push(crate::host::gui::Op::CursorZone {
                    x: clamp_u16(x_i),
                    y: clamp_u16(y_i),
                    w: clamp_u16(w_i),
                    h: clamp_u16(h_i),
                    shape,
                });
            }
        } else if tag == tags.vspans_t {
            let Ok(row0_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(col0_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let col_vals: Vec<Value> = match arg(&parts, 3) {
                Value::Vector(id) => heap.vector(id).to_vec(),
                _ => Vec::new(),
            };
            let mut cols = Vec::with_capacity(col_vals.len());
            for cv in col_vals {
                let seg_vals: Vec<Value> = match cv {
                    Value::Vector(id) => heap.vector(id).to_vec(),
                    _ => Vec::new(),
                };
                let mut segs = Vec::with_capacity(seg_vals.len());
                for sv in seg_vals {
                    let s: Vec<Value> = match sv {
                        Value::Vector(id) => heap.vector(id).to_vec(),
                        _ => continue,
                    };
                    if s.len() >= 2 {
                        let Ok(h) = expect_int(heap, "%gui-draw", s[0]) else {
                            continue;
                        };
                        segs.push((clamp_u16(h), span_color(heap, s[1])));
                    }
                }
                cols.push(segs);
            }
            ops.push(crate::host::gui::Op::VSpans {
                row0: clamp_u16(row0_i),
                col0: clamp_u16(col0_i),
                cols,
            });
        } else if tag == tags.cells_t {
            let Ok(row0_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(col0_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let Ok(w_i) = expect_int(heap, "%gui-draw", arg(&parts, 3)) else {
                continue;
            };
            let Ok(asp_i) = expect_int(heap, "%gui-draw", arg(&parts, 4)) else {
                continue;
            };
            let bytes = match arg(&parts, 5) {
                Value::Str(id) => match heap.local_shared_blob(id) {
                    Some(blob) => blob.as_bytes().to_vec(),
                    None => heap.string(id).as_bytes().to_vec(),
                },
                v => match expect_bigint(heap, "%gui-draw", v) {
                    Ok(b) => b.magnitude().to_bytes_le(),
                    Err(_) => continue,
                },
            };
            let color = span_color(heap, arg(&parts, 6));
            ops.push(crate::host::gui::Op::Cells {
                row0: clamp_u16(row0_i),
                col0: clamp_u16(col0_i),
                // Clamp BEFORE the narrowing cast: `w_i.max(1) as u32` truncates
                // afterwards, so 4294967296 came through the guard as 0.
                w: clamp_extent(w_i).max(1) as u32,
                aspect: clamp_u16(asp_i).max(1),
                bytes,
                color,
            });
        } else if tag == tags.cells_rgb_t {
            let Ok(row0_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(col0_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let Ok(w_i) = expect_int(heap, "%gui-draw", arg(&parts, 3)) else {
                continue;
            };
            let Ok(asp_i) = expect_int(heap, "%gui-draw", arg(&parts, 4)) else {
                continue;
            };
            let bytes = match arg(&parts, 5) {
                Value::Str(id) => match heap.local_shared_blob(id) {
                    Some(blob) => blob.as_bytes().to_vec(),
                    None => heap.string(id).as_bytes().to_vec(),
                },
                v => match expect_bigint(heap, "%gui-draw", v) {
                    Ok(b) => b.magnitude().to_bytes_le(),
                    Err(_) => continue,
                },
            };
            let mut colors = std::collections::HashMap::new();
            if let Value::Map(mid) = arg(&parts, 6) {
                for (k, v) in heap.map_entries(mid) {
                    if let (Value::Int(idx), Some(p)) = (k, heap.as_bigint(v)) {
                        let bits: u64 = (&p).try_into().unwrap_or(0);
                        colors.insert(
                            idx as u64,
                            [
                                (bits & 4095) as u8,
                                ((bits >> 12) & 4095) as u8,
                                ((bits >> 24) & 4095) as u8,
                            ],
                        );
                    }
                }
            }
            let default = span_color(heap, arg(&parts, 7)).unwrap_or([229, 229, 229]);
            ops.push(crate::host::gui::Op::CellsRgb {
                row0: clamp_u16(row0_i),
                col0: clamp_u16(col0_i),
                // Clamp BEFORE the narrowing cast: `w_i.max(1) as u32` truncates
                // afterwards, so 4294967296 came through the guard as 0.
                w: clamp_extent(w_i).max(1) as u32,
                aspect: clamp_u16(asp_i).max(1),
                bytes,
                colors,
                default,
            });
        } else if tag == tags.translate_t {
            // [:translate drow dcol inner-ops] — the inner ops moved drow rows down and dcol
            // columns right, resolved here into ops at absolute cells (ADR-728): nothing past
            // the parse knows the block was there.
            // a malformed block is skipped, as every op here is: no error is built to be dropped
            let (Some(drow), Some(dcol)) = (arg(&parts, 1).as_int(), arg(&parts, 2).as_int())
            else {
                continue;
            };
            let (Ok(drow), Ok(dcol)) = (i32::try_from(drow), i32::try_from(dcol)) else {
                continue;
            };
            // the ops a list or a vector: `ui-memo-at` hands its one marker as a list, as
            // `ui-memo` answers it
            if let Some(items) = heap.list_or_vector_items(arg(&parts, 3)) {
                let inner_ops = parse_gui_ops(heap, tagged_ops(heap, items), tags, pass);
                pass.push_translated(inner_ops, drow, dcol, &mut ops);
            }
        } else if tag == tags.scroll_region_t {
            // [:scroll-region dy-frac inner-ops] — shift inner ops upward by dy_frac ×
            // cell_h pixels. Self-resetting block; ops outside are unaffected. ADR-114.
            let dy_frac = num(arg(&parts, 1));
            let inner_val = arg(&parts, 2);
            if let Ok(inner_parsed) = frame_ops(heap, inner_val, "%gui-draw", "scroll-region ops") {
                let inner_ops = parse_gui_ops(heap, inner_parsed, tags, pass);
                ops.push(crate::host::gui::Op::ScrollRegion {
                    dy_frac,
                    ops: inner_ops,
                });
            }
        } else if tag == tags.cell_region_t {
            // [:cell-region x y w h px inner-ops] — paint the inner ops with the cell
            // metrics of `px`, inside the parent-cell rect [x y w h], positioned in the
            // region's OWN cell space. Self-restoring like scroll-region; this is what lets
            // one frame carry two text sizes (a per-buffer zoom).
            let Ok(x_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(y_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let Ok(w_i) = expect_int(heap, "%gui-draw", arg(&parts, 3)) else {
                continue;
            };
            let Ok(h_i) = expect_int(heap, "%gui-draw", arg(&parts, 4)) else {
                continue;
            };
            let px = num(arg(&parts, 5));
            if !(px.is_finite() && px >= 1.0) {
                continue;
            }
            if let Ok(inner_parsed) =
                frame_ops(heap, arg(&parts, 6), "%gui-draw", "cell-region ops")
            {
                let inner_ops = parse_gui_ops(heap, inner_parsed, tags, pass);
                ops.push(crate::host::gui::Op::CellRegion {
                    x: clamp_u16(x_i),
                    y: clamp_u16(y_i),
                    w: clamp_extent(w_i),
                    h: clamp_extent(h_i),
                    px,
                    ops: inner_ops,
                });
            }
        } else if tag == tags.quad_t {
            // `[:quad x y w h color]` / `[:quad x y w h color rot]` — pixel space.
            let x = num(arg(&parts, 1));
            let y = num(arg(&parts, 2));
            let w = num(arg(&parts, 3));
            let h = num(arg(&parts, 4));
            let Some(color) = rgba_of(heap, arg(&parts, 5)) else {
                continue;
            };
            let rot = num(parts.get(6).copied().unwrap_or(Value::nil()));
            ops.push(crate::host::gui::Op::Quad {
                x,
                y,
                w,
                h,
                color,
                rot,
            });
        } else if tag == tags.text_px_t {
            // `[:text-px x y s face]` — pixel space; the face may carry `:align`.
            let x = num(arg(&parts, 1));
            let y = num(arg(&parts, 2));
            let Ok(s) = expect_string(heap, "%gui-draw", arg(&parts, 3)) else {
                continue;
            };
            let face_v = parts.get(4).copied().unwrap_or(Value::nil());
            let face = gui_face(heap, face_v);
            let align = match face_v {
                Value::Map(id) => match heap.map_get(id, value::kw("align")) {
                    Some(Value::Keyword(k)) if k == value::intern("center") => {
                        crate::host::gui::Align::Center
                    }
                    Some(Value::Keyword(k)) if k == value::intern("right") => {
                        crate::host::gui::Align::Right
                    }
                    _ => crate::host::gui::Align::Left,
                },
                _ => crate::host::gui::Align::Left,
            };
            ops.push(crate::host::gui::Op::TextPx {
                x,
                y,
                s,
                face,
                align,
            });
        } else if tag == tags.sound_t {
            // `[:sound id]` / `[:sound id vol]` / `[:sound id vol pan]` /
            // `[:sound id vol pan key]` — a handle that is not a non-negative int is not a
            // sound; an absent or non-number volume is the default (0), an absent pan the
            // centre, and a key that is not an int no key (the sound plays every frame
            // that carries it, as before keys).
            let Value::Int(snd) = arg(&parts, 1) else {
                continue;
            };
            let Ok(id) = u32::try_from(snd) else {
                continue;
            };
            let number_at = |index: usize| match parts.get(index).copied() {
                Some(v @ (Value::Int(_) | Value::Float(_))) => num(v),
                _ => 0.0,
            };
            let key = match parts.get(4).copied() {
                Some(Value::Int(key)) => Some(key),
                _ => None,
            };
            ops.push(crate::host::gui::Op::Sound {
                id,
                vol: number_at(2),
                pan: number_at(3),
                key,
            });
        } else if tag == tags.music_t || tag == tags.ambience_t {
            // `[:music id]` / `[:music id vol]`, and `[:ambience …]` the same — read
            // exactly as `[:sound …]` is.
            let Value::Int(snd) = arg(&parts, 1) else {
                continue;
            };
            let Ok(id) = u32::try_from(snd) else {
                continue;
            };
            let vol = match parts.get(2).copied() {
                Some(v @ (Value::Int(_) | Value::Float(_))) => num(v),
                _ => 0.0,
            };
            ops.push(if tag == tags.music_t {
                crate::host::gui::Op::Music { id, vol }
            } else {
                crate::host::gui::Op::Ambience { id, vol }
            });
        } else if tag == tags.ambient_t {
            // `[:ambient color]` — a colour lighting cannot read is no lighting.
            let Some(color) = face_rgb(heap, arg(&parts, 1)) else {
                continue;
            };
            ops.push(crate::host::gui::Op::Ambient { color });
        } else if tag == tags.light_t {
            // `[:light x y radius color]` / `… intensity` (default 1).
            let Some(color) = face_rgb(heap, arg(&parts, 4)) else {
                continue;
            };
            let intensity = match parts.get(5).copied() {
                Some(v @ (Value::Int(_) | Value::Float(_) | Value::Ratio(_))) => num(v),
                _ => 1.0,
            };
            ops.push(crate::host::gui::Op::Light {
                x: num(arg(&parts, 1)),
                y: num(arg(&parts, 2)),
                radius: num(arg(&parts, 3)),
                color,
                intensity,
            });
        } else if tag == tags.seam_t {
            // `[:seam row]` / `[:seam row {:top-band b}]` (ADR-610): row a non-negative int or
            // nil (no seam); `:top-band` true unless the map says false or nil.
            let row = match arg(&parts, 1) {
                Value::Int(n) => u16::try_from(n.max(0)).ok(),
                _ => None,
            };
            let top_band = match parts.get(2).copied() {
                Some(Value::Map(opts)) => heap
                    .map_get(opts, value::kw("top-band"))
                    .is_none_or(crate::eval::truthy),
                _ => true,
            };
            ops.push(crate::host::gui::Op::Seam { row, top_band });
        } else if tag == tags.above_seam_t {
            // `[:above-seam ops]` (ADR-615): ops on the rows above the seam carried on past it,
            // cut off at the seam — what lands on its row shows in the leftover strip.
            if let Ok(inner_parsed) = frame_ops(heap, arg(&parts, 1), "%gui-draw", "above-seam ops")
            {
                let inner_ops = parse_gui_ops(heap, inner_parsed, tags, pass);
                ops.push(crate::host::gui::Op::AboveSeam { ops: inner_ops });
            }
        } else if tag == tags.overlay_t {
            ops.push(crate::host::gui::Op::Overlay);
        } else if tag == tags.bloom_t {
            // `[:bloom threshold intensity radius]`; a missing one takes its default.
            let or = |k: usize, default: f32| match parts.get(k).copied() {
                Some(v @ (Value::Int(_) | Value::Float(_) | Value::Ratio(_))) => num(v),
                _ => default,
            };
            ops.push(crate::host::gui::Op::Bloom {
                threshold: or(1, 0.7),
                intensity: or(2, 0.8),
                radius: or(3, 8.0),
            });
        } else if tag == tags.crt_t {
            // `[:crt scanlines curvature vignette]`; a missing one takes its default.
            let or = |k: usize, default: f32| match parts.get(k).copied() {
                Some(v @ (Value::Int(_) | Value::Float(_) | Value::Ratio(_))) => num(v),
                _ => default,
            };
            ops.push(crate::host::gui::Op::Crt {
                scanlines: or(1, 0.25),
                curvature: or(2, 0.15),
                vignette: or(3, 0.3),
            });
        } else if tag == tags.emitter_t {
            // `[:emitter id x y opts]` — a handle that is not a non-negative int is not an
            // emitter; every key of `opts` is optional.
            let Value::Int(id) = arg(&parts, 1) else {
                continue;
            };
            let Ok(id) = u32::try_from(id) else {
                continue;
            };
            let spec = emitter_spec(heap, arg(&parts, 4), &num);
            ops.push(crate::host::gui::Op::Emitter {
                id,
                x: num(arg(&parts, 2)),
                y: num(arg(&parts, 3)),
                spec,
            });
        } else if tag == tags.scene_t {
            // `[:scene x y w h spec]` — meshes in 3D over a pixel rect (ADR-786); a spec without
            // a 16-number `:camera` is not a scene.
            // A malformed light is refused, not skipped (ADR-788): the frame is not drawn.
            match scene_spec(
                heap,
                [
                    num(arg(&parts, 1)),
                    num(arg(&parts, 2)),
                    num(arg(&parts, 3)),
                    num(arg(&parts, 4)),
                ],
                arg(&parts, 5),
                &num,
            ) {
                Ok(Some(scene)) => {
                    ops.push(crate::host::gui::Op::Scene(std::sync::Arc::new(scene)))
                }
                Ok(None) => {}
                Err(why) => tags.refuse(format!("a [:scene …] op: {why}")),
            }
        } else if tag == tags.burst_t {
            // `[:burst id count]`.
            let (Value::Int(id), Value::Int(count)) = (arg(&parts, 1), arg(&parts, 2)) else {
                continue;
            };
            let (Ok(id), Ok(count)) = (u32::try_from(id), u32::try_from(count)) else {
                continue;
            };
            ops.push(crate::host::gui::Op::Burst {
                id,
                count: count.min(crate::host::gui::MAX_PARTICLES),
            });
        } else if tag == tags.sprite_t {
            // `[:sprite tex x y w h]` with optional `uv` (`[u0 v0 du dv]`, default the
            // whole texture), `tint` (default opaque white) and `rot` (default 0) —
            // pixel space. A texture handle that is not a non-negative int is not a
            // sprite; a `uv` of the wrong shape means the whole texture.
            let Value::Int(tex) = arg(&parts, 1) else {
                continue;
            };
            let Ok(tex) = u32::try_from(tex) else {
                continue;
            };
            let x = num(arg(&parts, 2));
            let y = num(arg(&parts, 3));
            let w = num(arg(&parts, 4));
            let h = num(arg(&parts, 5));
            let uv = match parts.get(6).copied() {
                Some(Value::Vector(vid)) if heap.vector(vid).len() == 4 => {
                    let xs = heap.vector(vid);
                    [num(xs[0]), num(xs[1]), num(xs[2]), num(xs[3])]
                }
                _ => [0.0, 0.0, 1.0, 1.0],
            };
            let tint = parts
                .get(7)
                .copied()
                .and_then(|v| rgba_of(heap, v))
                .unwrap_or([255, 255, 255, 255]);
            let rot = num(parts.get(8).copied().unwrap_or(Value::nil()));
            ops.push(crate::host::gui::Op::Sprite {
                tex,
                x,
                y,
                w,
                h,
                uv,
                tint,
                rot,
            });
        }
    }
    ops
}

/// A pixel-space op's colour: a face colour (`:red`, `[r g b]`, `"#rrggbb"`, via the
/// shared `face_rgb`) read as opaque, or an explicit `[r g b a]` vector (each channel
/// clamped to 0..255) for a translucent quad. `None` for anything else — the op is
/// skipped, the frame's forward-compatibility rule.
pub(in crate::builtins) fn rgba_of(heap: &Heap, v: Value) -> Option<[u8; 4]> {
    if let Value::Vector(id) = v {
        let xs = heap.vector(id);
        if xs.len() == 4 {
            let chan = |k: usize| match xs[k] {
                Value::Int(n) => n.clamp(0, 255) as u8,
                Value::Float(f) => f.clamp(0.0, 255.0) as u8,
                _ => 0,
            };
            return Some([chan(0), chan(1), chan(2), chan(3)]);
        }
    }
    face_rgb(heap, v).map(|[r, g, b]| [r, g, b, 255])
}

/// An `[:emitter …]` op's options map read onto `EmitterSpec`'s defaults: `:rate`, `:life`
/// `[min max]`, `:speed [min max]`, `:angle`, `:spread`, `:gravity [gx gy]`, `:drag`,
/// `:size [birth death]`, `:color` / `:color-end` (a face colour or `[r g b a]`),
/// `:blend :add`, `:shape :square`, `:max`. A key of the wrong shape keeps its default;
/// every number is clamped to something drawable.
fn emitter_spec(
    heap: &Heap,
    opts: Value,
    num: &dyn Fn(Value) -> f32,
) -> crate::host::gui::EmitterSpec {
    use crate::host::gui::{EmitterSpec, MAX_PARTICLES};
    let mut spec = EmitterSpec::default();
    let Value::Map(map) = opts else {
        return spec;
    };
    let get = |key: &str| heap.map_get(map, value::kw(key));
    let number = |key: &str| match get(key) {
        Some(v @ (Value::Int(_) | Value::Float(_) | Value::Ratio(_))) => Some(num(v)),
        _ => None,
    };
    let pair = |key: &str| match get(key) {
        Some(Value::Vector(id)) if heap.vector(id).len() == 2 => {
            let xs = heap.vector(id);
            Some([num(xs[0]), num(xs[1])])
        }
        Some(v @ (Value::Int(_) | Value::Float(_) | Value::Ratio(_))) => Some([num(v), num(v)]),
        _ => None,
    };
    let finite = |x: f32, lo: f32, hi: f32| if x.is_finite() { x.clamp(lo, hi) } else { lo };
    if let Some(rate) = number("rate") {
        spec.rate = finite(rate, 0.0, 1_000_000.0);
    }
    if let Some([a, b]) = pair("life") {
        spec.life = [finite(a, 0.001, 600.0), finite(b, 0.001, 600.0)];
    }
    if let Some([a, b]) = pair("speed") {
        spec.speed = [finite(a, -1e6, 1e6), finite(b, -1e6, 1e6)];
    }
    if let Some(angle) = number("angle") {
        spec.angle = finite(angle, -1e3, 1e3);
    }
    if let Some(spread) = number("spread") {
        spec.spread = finite(spread, 0.0, std::f32::consts::TAU);
    }
    if let Some([gx, gy]) = pair("gravity") {
        spec.gravity = [finite(gx, -1e6, 1e6), finite(gy, -1e6, 1e6)];
    }
    if let Some(drag) = number("drag") {
        spec.drag = finite(drag, 0.0, 1e3);
    }
    if let Some([a, b]) = pair("size") {
        spec.size = [finite(a, 0.0, 4096.0), finite(b, 0.0, 4096.0)];
    }
    if let Some(color) = get("color").and_then(|v| rgba_of(heap, v)) {
        spec.color = color;
        // without a :color-end, fade the same colour out
        spec.color_end = [color[0], color[1], color[2], 0];
    }
    if let Some(color) = get("color-end").and_then(|v| rgba_of(heap, v)) {
        spec.color_end = color;
    }
    if let Some(Value::Keyword(k)) = get("blend") {
        spec.additive = k == value::intern("add");
    }
    if let Some(Value::Keyword(k)) = get("shape") {
        spec.square = k == value::intern("square");
    }
    if let Some(Value::Int(max)) = get("max") {
        spec.max = max.clamp(1, MAX_PARTICLES as i64) as u32;
    }
    spec
}

/// A `[:scene x y w h spec]` op's spec read into a `mesh::Scene` (ADR-786): `:camera` (16
/// numbers, a column-major view-projection with clip depth 0 … 1 — required), `:eye [x y z]`,
/// `:lights` (up to `MAX_LIGHTS`, each directional, point or spot — [`light_spec`], ADR-788),
/// `:sky` / `:ground [r g b]` (the ambient from above and below) or `:ambient-sh` (nine
/// `[r g b]` spherical-harmonic coefficients, ADR-793), `:exposure`, `:background
/// [r g b a]` (0 … 1, transparent by default), `:ambient-occlusion {:radius m :strength s}`
/// (or `true`: folds and corners darken the ambient light — off by default), `:palettes`
/// (joint matrices, 16 numbers each: a vector of numbers or f32 bytes per skeleton) and `:draws [{:mesh h :model [16] :palette i
/// :morphs [[target weight] …] :material {…}} …]`. Colours and light are linear. A key of the
/// wrong shape keeps its default and a draw without a mesh handle is dropped — except the
/// lights, `:ambient-sh`, and a material's `:cast-shadow`, `:lobes`, `:scatter` and `:detail`
/// keys (ADR-797) and its `:eye` (ADR-796), where a mistake would light the scene wrongly
/// without a word: those are
/// refused (`Err`, which `%gui-draw` raises).
/// No 16-number `:camera` is no scene (`Ok(None)`).
fn scene_spec(
    heap: &Heap,
    rect: [f32; 4],
    spec: Value,
    num: &dyn Fn(Value) -> f32,
) -> Result<Option<crate::host::gui::mesh::Scene>, String> {
    use crate::host::gui::mesh::{
        AmbientOcclusion, DEFAULT_OCCLUSION_RADIUS, Draw, MAX_DETAIL_SCALE, MAX_DETAIL_STRENGTH,
        MAX_EMITTERS, MAX_SCATTER, Material, SPECULAR_TO_F0, Scene, Surface,
    };
    let Value::Map(map) = spec else {
        return Ok(None);
    };
    let is_number = |candidate: Value| {
        matches!(
            candidate,
            Value::Int(_) | Value::Float(_) | Value::Ratio(_) | Value::Decimal(_)
        )
    };
    let finite = |number: f32| if number.is_finite() { number } else { 0.0 };
    let numbers = |given: Option<Value>, count: usize| -> Option<Vec<f32>> {
        match given {
            Some(Value::Vector(id))
                if heap.vector(id).len() == count
                    && heap.vector(id).iter().all(|item| is_number(*item)) =>
            {
                Some(
                    heap.vector(id)
                        .iter()
                        .map(|item| finite(num(*item)))
                        .collect(),
                )
            }
            _ => None,
        }
    };
    let three = |given: Option<Value>, default: [f32; 3]| {
        numbers(given, 3).map_or(default, |values| [values[0], values[1], values[2]])
    };
    let matrix = |given: Option<Value>| {
        numbers(given, 16).map(|values| {
            let mut out = [0.0f32; 16];
            out.copy_from_slice(&values);
            out
        })
    };
    let get = |within, key: &str| heap.map_get(within, value::kw(key));
    let Some(view_proj) = matrix(get(map, "camera")) else {
        return Ok(None);
    };
    let lights = lights_spec(heap, get(map, "lights"), num)?;
    let palettes = match get(map, "palettes") {
        Some(Value::Vector(id)) => heap
            .vector(id)
            .iter()
            .map(|palette| {
                let values: Vec<f32> = match palette {
                    Value::Vector(numbers_id) => heap
                        .vector(*numbers_id)
                        .iter()
                        .map(|item| num(*item))
                        .collect(),
                    Value::Bytes(bytes_id) => heap
                        .bytes(*bytes_id)
                        .as_bytes()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|word| f32::from_le_bytes(*word))
                        .collect(),
                    _ => Vec::new(),
                };
                // whole joints only, every number finite
                let whole = values.len() / 16 * 16;
                let values: Vec<f32> = values[..whole]
                    .iter()
                    .map(|number| finite(*number))
                    .collect();
                std::sync::Arc::from(values)
            })
            .collect(),
        _ => Vec::new(),
    };
    let material_of = |given: Option<Value>| -> Result<Material, String> {
        let mut material = Material::default();
        let Some(Value::Map(material_map)) = given else {
            return Ok(material);
        };
        let number = |key: &str| match get(material_map, key) {
            Some(item) if is_number(item) => Some(num(item)).filter(|value| value.is_finite()),
            _ => None,
        };
        let handle = |key: &str| match get(material_map, key) {
            Some(Value::Int(handle)) => u32::try_from(handle).ok(),
            _ => None,
        };
        if let Some(Value::Keyword(surface)) = get(material_map, "surface") {
            material.surface = match value::symbol_name_ref(surface) {
                "skin" => Surface::Skin,
                "cutout" => Surface::Cutout,
                "blend" => Surface::Blend,
                "hair" => Surface::Hair,
                "strands" => Surface::Strands,
                "eye" => Surface::Eye,
                _ => Surface::Standard,
            };
        }
        material.base = handle("base");
        material.normal = handle("normal");
        material.occlusion = handle("occlusion");
        // the detail tile and its settings (ADR-797), each refused when malformed: a tile
        // dropped or read at the wrong size is skin gone smooth again without a word
        material.detail = match get(material_map, "detail") {
            None | Some(Value::Nil) => None,
            Some(Value::Int(given)) if u32::try_from(given).is_ok() => u32::try_from(given).ok(),
            Some(given) => {
                return Err(format!(
                    "a material's :detail is a texture handle (a whole number, 0 or more), not {}",
                    crate::syntax::printer::print(heap, given)
                ));
            }
        };
        let repeats = |given: f32| given > 0.0 && given <= MAX_DETAIL_SCALE;
        material.detail_scale = match get(material_map, "detail-scale") {
            None | Some(Value::Nil) => material.detail_scale,
            Some(given) if is_number(given) && repeats(num(given)) => [num(given); 2],
            Some(given) => match numbers(Some(given), 2) {
                Some(scale) if repeats(scale[0]) && repeats(scale[1]) => [scale[0], scale[1]],
                _ => {
                    return Err(format!(
                        "a material's :detail-scale is how many times its :detail tile repeats across the UV square, a number or [u v] above 0 and at most {MAX_DETAIL_SCALE}, not {}",
                        crate::syntax::printer::print(heap, given)
                    ));
                }
            },
        };
        let within = |key: &str, most: f32, what: &str| -> Result<Option<f32>, String> {
            match get(material_map, key) {
                None | Some(Value::Nil) => Ok(None),
                Some(given) if is_number(given) && (0.0..=most).contains(&num(given)) => {
                    Ok(Some(num(given)))
                }
                Some(given) => Err(format!(
                    "a material's :{key} is {what}, a number from 0 to {most}, not {}",
                    crate::syntax::printer::print(heap, given)
                )),
            }
        };
        if let Some(strength) = within(
            "detail-strength",
            MAX_DETAIL_STRENGTH,
            "what its :detail normal's slope is multiplied by (1: as baked)",
        )? {
            material.detail_strength = strength;
        }
        if let Some(cavity) = within(
            "detail-cavity",
            1.0,
            "how much its :detail tile's cavity damps the specular (1: all of it)",
        )? {
            material.detail_cavity = cavity;
        }
        if let Some(color) = numbers(get(material_map, "color"), 4) {
            material.color = [color[0], color[1], color[2], color[3]];
        } else if let Some(color) = numbers(get(material_map, "color"), 3) {
            material.color = [color[0], color[1], color[2], 1.0];
        }
        if let Some(roughness) = number("roughness") {
            material.roughness = roughness.clamp(0.0, 1.0);
        }
        if let Some(specular) = number("specular") {
            material.specular = specular.clamp(0.0, 1.0 / SPECULAR_TO_F0);
        }
        // how far light travels under a `:skin` surface (ADR-794): a multiple of human skin's
        // on a mesh in metres. Its old meaning, the wrapped light's colour, is refused by name,
        // so a material written for the wrapped diffuse says what to change instead of drawing
        // differently in silence.
        material.scatter = match get(material_map, "scatter") {
            None | Some(Value::Nil) => None,
            Some(given) if is_number(given) && (0.0..=MAX_SCATTER).contains(&num(given)) => {
                Some(num(given))
            }
            Some(given) => {
                return Err(format!(
                    "a material's :scatter is how far light travels under a :skin surface, a number from 0 (none) to {MAX_SCATTER} (1: human skin on a mesh in metres), not {}; the scattered light's colour is skin's own now, not a colour to give",
                    crate::syntax::printer::print(heap, given)
                ));
            }
        };
        // a clear layer over the surface (ADR-795), refused when malformed: a wet lip drawn dry
        // says nothing. (`:rim` is gone with the same decision — the surroundings reflected at
        // a grazing angle are the rim — and is now a key a material does not have: ignored.)
        material.coat = match get(material_map, "coat") {
            None | Some(Value::Nil) => None,
            Some(given) => match numbers(Some(given), 2) {
                Some(coat) if (0.0..=1.0).contains(&coat[0]) && (0.0..=1.0).contains(&coat[1]) => {
                    Some([coat[0], coat[1]])
                }
                _ => {
                    return Err(format!(
                        "a material's :coat is [strength roughness], a clear layer over the surface, each 0 … 1, not {}",
                        crate::syntax::printer::print(heap, given)
                    ));
                }
            },
        };
        if let Some(cutoff) = number("cutoff") {
            material.cutoff = cutoff.clamp(0.0, 1.0);
        }
        // a `:strands` surface's width, metres: one number, or [root tip]
        if let Some(width) = number("width") {
            material.strand_width = [width.max(0.0); 2];
        } else if let Some(widths) = numbers(get(material_map, "width"), 2) {
            material.strand_width = [widths[0].max(0.0), widths[1].max(0.0)];
        }
        // a hair surface's two highlights' shifts along its normal, [first second], and its
        // second highlight's colour, linear [r g b] (ADR-799; as before when not given)
        if let Some(shift) = numbers(get(material_map, "shift"), 2) {
            material.hair_shift = [shift[0].clamp(-1.0, 1.0), shift[1].clamp(-1.0, 1.0)];
        }
        if let Some(sheen) = numbers(get(material_map, "sheen"), 3) {
            material.sheen = Some([sheen[0].max(0.0), sheen[1].max(0.0), sheen[2].max(0.0)]);
        }
        material.lobes = match get(material_map, "lobes") {
            None | Some(Value::Nil) => None,
            Some(given) => match numbers(Some(given), 3) {
                Some(lobes)
                    if lobes[0] > 0.0 && lobes[1] > 0.0 && (0.0..=1.0).contains(&lobes[2]) =>
                {
                    Some([lobes[0], lobes[1], lobes[2]])
                }
                _ => {
                    return Err(format!(
                        "a material's :lobes is [rough-a rough-b mix], the roughness multipliers above 0 and the mix 0 … 1, not {}",
                        crate::syntax::printer::print(heap, given)
                    ));
                }
            },
        };
        material.double_sided =
            matches!(get(material_map, "double-sided"), Some(Value::Bool(true)));
        // its skin pressed by the scene's `:contacts` (ADR-805)
        material.deform = matches!(get(material_map, "deform"), Some(Value::Bool(true)));
        material.cast_shadow = match get(material_map, "cast-shadow") {
            None | Some(Value::Bool(true)) => true,
            Some(Value::Bool(false)) => false,
            Some(other) => {
                return Err(format!(
                    "a material's :cast-shadow is true or false, not {}",
                    crate::syntax::printer::print(heap, other)
                ));
            }
        };
        // an eye's optics (ADR-796): read and checked on any surface, used by `:eye` alone,
        // which is nothing without them
        material.eye = eye_spec(heap, get(material_map, "eye"), num)?;
        if material.surface == Surface::Eye && material.eye.is_none() {
            return Err(
                "an :eye surface needs its :eye: {:eyes [{:centre [x y z] :axis [x y z] :iris-uv [u v]} …] :iris-uv-radius r}, where its eyes are in the mesh at rest and their irises in its texture"
                    .to_string(),
            );
        }
        Ok(material)
    };
    let draws = match get(map, "draws") {
        Some(Value::Vector(id)) => heap
            .vector(id)
            .iter()
            .filter_map(|draw| {
                let Value::Map(draw_map) = draw else {
                    return None;
                };
                let Some(Value::Int(mesh)) = get(*draw_map, "mesh") else {
                    return None;
                };
                let mesh = u32::try_from(mesh).ok()?;
                let material = match material_of(get(*draw_map, "material")) {
                    Ok(material) => material,
                    Err(why) => return Some(Err(why)),
                };
                let palette = match get(*draw_map, "palette") {
                    Some(Value::Int(index)) => usize::try_from(index).ok(),
                    _ => None,
                };
                let mut morphs: Vec<(u32, f32)> = match get(*draw_map, "morphs") {
                    Some(Value::Vector(morphs_id)) => heap
                        .vector(morphs_id)
                        .iter()
                        .filter_map(|pair| match pair {
                            Value::Vector(pair_id) if heap.vector(*pair_id).len() == 2 => {
                                let items = heap.vector(*pair_id);
                                match items[0] {
                                    Value::Int(target) => u32::try_from(target)
                                        .ok()
                                        .map(|target| (target, num(items[1]))),
                                    _ => None,
                                }
                            }
                            _ => None,
                        })
                        .filter(|(_, weight)| weight.is_finite() && *weight != 0.0)
                        .collect(),
                    _ => Vec::new(),
                };
                morphs.sort_by(|left, right| right.1.abs().total_cmp(&left.1.abs()));
                Some(Ok(Draw {
                    mesh,
                    model: matrix(get(*draw_map, "model")).unwrap_or([
                        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                        1.0,
                    ]),
                    palette,
                    morphs,
                    material,
                }))
            })
            .collect::<Result<Vec<Draw>, String>>()?,
        _ => Vec::new(),
    };
    let exposure = match get(map, "exposure") {
        Some(item) if is_number(item) && num(item).is_finite() => num(item).max(0.0),
        _ => 1.0,
    };
    let background = numbers(get(map, "background"), 4)
        .map_or([0.0; 4], |color| [color[0], color[1], color[2], color[3]]);
    let ambient_sh = match get(map, "ambient-sh") {
        None | Some(Value::Nil) => None,
        Some(given) => {
            let triples = match given {
                Value::Vector(id) if heap.vector(id).len() == 9 => heap
                    .vector(id)
                    .iter()
                    .map(|triple| numbers(Some(*triple), 3).map(|rgb| [rgb[0], rgb[1], rgb[2]]))
                    .collect::<Option<Vec<[f32; 3]>>>(),
                _ => None,
            };
            let Some(triples) = triples else {
                return Err(format!(
                    ":ambient-sh is 9 [r g b] spherical-harmonic coefficients (L00 L1-1 L10 L11 \
                     L2-2 L2-1 L20 L21 L22), not {}",
                    crate::syntax::printer::print(heap, given)
                ));
            };
            Some(std::array::from_fn(|index| triples[index]))
        }
    };
    // `:ambient-occlusion {:radius metres :strength 0 … 1}` (or `true` for the defaults): the
    // ambient light darkened in folds and corners (`mesh::ambient_occlusion`); off without it
    let ambient_occlusion = match get(map, "ambient-occlusion") {
        Some(Value::Bool(true)) => Some(AmbientOcclusion {
            radius: DEFAULT_OCCLUSION_RADIUS,
            strength: 1.0,
        }),
        Some(Value::Map(settings)) => {
            let number = |key: &str, default: f32| match get(settings, key) {
                Some(item) if is_number(item) && num(item).is_finite() => num(item),
                _ => default,
            };
            Some(AmbientOcclusion {
                radius: number("radius", DEFAULT_OCCLUSION_RADIUS).max(0.0),
                strength: number("strength", 1.0).clamp(0.0, 1.0),
            })
        }
        _ => None,
    };
    // the shapes seen only in reflections (ADR-795); with the shaped lights, at most
    // `MAX_EMITTERS`
    let emitters = reflected_emitters_spec(heap, get(map, "emitters"), num)?;
    let shaped = lights.iter().filter(|light| light.shape.is_some()).count();
    if shaped + emitters.len() > MAX_EMITTERS {
        return Err(format!(
            "{shaped} lights with a :shape and {} :emitters are more than the {MAX_EMITTERS} shapes a scene's reflections show",
            emitters.len()
        ));
    }
    // `:contacts [{:plane [nx ny nz d]} {:sphere [x y z r]} {:capsule [ax ay az bx by bz r]} …]`:
    // what presses the skin of its `:deform` draws this frame (ADR-805), in world space
    let mut contacts = Vec::new();
    // and how far each drags the skin along its surface this frame (`:drag [x y z]`, metres)
    let mut contact_drags = Vec::new();
    if let Some(given) = get(map, "contacts") {
        let Value::Vector(list) = given else {
            return Err(":contacts is a vector of contact maps".into());
        };
        let list = heap.vector(list).to_vec();
        if list.len() > crate::host::gui::mesh::MAX_CONTACTS {
            return Err(format!(
                "{} :contacts are more than the {} a scene presses its skin with",
                list.len(),
                crate::host::gui::mesh::MAX_CONTACTS
            ));
        }
        let shape = "a contact is {:plane [nx ny nz d]}, {:sphere [x y z r]} or {:capsule [ax ay az bx by bz r]}";
        for item in list {
            let Value::Map(contact) = item else {
                return Err(shape.into());
            };
            contact_drags.push(
                numbers(get(contact, "drag"), 3)
                    .map_or([0.0; 3], |drag| [drag[0], drag[1], drag[2]]),
            );
            use crate::host::gui::mesh::Contact;
            let contact = if let Some(plane) = numbers(get(contact, "plane"), 4) {
                let length =
                    (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
                if length <= 1e-9 {
                    return Err("a :plane contact's normal has no length".into());
                }
                Contact::Plane {
                    normal: [plane[0] / length, plane[1] / length, plane[2] / length],
                    offset: plane[3] / length,
                }
            } else if let Some(sphere) = numbers(get(contact, "sphere"), 4) {
                Contact::Sphere {
                    centre: [sphere[0], sphere[1], sphere[2]],
                    radius: sphere[3].max(0.0),
                }
            } else if let Some(capsule) = numbers(get(contact, "capsule"), 7) {
                Contact::Capsule {
                    start: [capsule[0], capsule[1], capsule[2]],
                    end: [capsule[3], capsule[4], capsule[5]],
                    radius: capsule[6].max(0.0),
                }
            } else {
                return Err(shape.into());
            };
            contacts.push(contact);
        }
    }
    Ok(Some(Scene {
        x: rect[0],
        y: rect[1],
        w: rect[2],
        h: rect[3],
        view_proj,
        eye: three(get(map, "eye"), [0.0, 0.0, 0.0]),
        lights,
        sky: three(get(map, "sky"), [0.3, 0.32, 0.36]),
        ground: three(get(map, "ground"), [0.1, 0.08, 0.06]),
        ambient_sh,
        exposure,
        ambient_occlusion,
        emitters,
        background,
        palettes,
        draws,
        contacts,
        contact_drags,
    }))
}

/// A `:shape` read: `:disc`, `:rect` or `:sphere` (ADR-795), or why it is none of them.
fn emitter_shape(
    heap: &Heap,
    given: Value,
) -> Result<crate::host::gui::mesh::EmitterShape, String> {
    use crate::host::gui::mesh::EmitterShape;
    let name = match given {
        Value::Keyword(symbol) => value::symbol_name_ref(symbol),
        _ => "",
    };
    match name {
        "disc" => Ok(EmitterShape::Disc),
        "rect" => Ok(EmitterShape::Rect),
        "sphere" => Ok(EmitterShape::Sphere),
        _ => Err(format!(
            ":shape is :disc, :rect or :sphere, not {}",
            crate::syntax::printer::print(heap, given)
        )),
    }
}

/// A scene spec's `:emitters` (absent or nil: none) read into `mesh::Emitter`s (ADR-795): shapes
/// seen only in reflections, each `{:shape :rect :position [x y z] :direction [x y z] :up
/// [x y z] :extent [w h] :radiance [r g b]}`, or with `:shape :disc` and `:size d` in place of
/// `:up` and `:extent`, or `{:shape :sphere :position … :size d :radiance …}`. Metres;
/// `:direction` is the way its light travels (the way it faces), `:up` (world up by default)
/// where a rectangle's height runs, `:radiance` linear, 0 or more, on the ambient's scale. A
/// malformed one, a key its shape does not take, or more than `MAX_EMITTERS` is refused with
/// why.
fn reflected_emitters_spec(
    heap: &Heap,
    given: Option<Value>,
    num: &dyn Fn(Value) -> f32,
) -> Result<Vec<crate::host::gui::mesh::Emitter>, String> {
    use crate::host::gui::mesh::{Emitter, EmitterShape, MAX_EMITTERS};
    let shown = |value: Value| crate::syntax::printer::print(heap, value);
    let id = match given {
        None | Some(Value::Nil) => return Ok(Vec::new()),
        Some(Value::Vector(id)) => id,
        Some(other) => {
            return Err(format!(
                ":emitters is a vector of emitter maps, not {}",
                shown(other)
            ));
        }
    };
    let entries = heap.vector(id);
    if entries.len() > MAX_EMITTERS {
        return Err(format!(
            "{} :emitters is more than the {MAX_EMITTERS} shapes a scene's reflections show",
            entries.len()
        ));
    }
    let numbers = |key: &str, value: Value, count: usize| -> Result<Vec<f32>, String> {
        let parsed = match value {
            Value::Vector(id) if heap.vector(id).len() == count => heap
                .vector(id)
                .iter()
                .map(|item| match item {
                    Value::Int(_) | Value::Float(_) | Value::Ratio(_) | Value::Decimal(_) => {
                        Some(num(*item)).filter(|number| number.is_finite())
                    }
                    _ => None,
                })
                .collect::<Option<Vec<f32>>>(),
            _ => None,
        };
        parsed.ok_or_else(|| format!(":{key} is {count} numbers, not {}", shown(value)))
    };
    let one = |entry: Value| -> Result<Emitter, String> {
        let Value::Map(map) = entry else {
            return Err(format!("an emitter is a map, not {}", shown(entry)));
        };
        let get = |key: &str| heap.map_get(map, value::kw(key));
        let shape = match get("shape") {
            Some(given) => emitter_shape(heap, given)?,
            None => return Err("an emitter needs a :shape, :disc, :rect or :sphere".to_string()),
        };
        let (name, takes): (&str, &[&str]) = match shape {
            EmitterShape::Rect => (
                "a :rect emitter",
                &["shape", "position", "direction", "up", "extent", "radiance"],
            ),
            EmitterShape::Disc => (
                "a :disc emitter",
                &["shape", "position", "direction", "size", "radiance"],
            ),
            EmitterShape::Sphere => (
                "a :sphere emitter",
                &["shape", "position", "size", "radiance"],
            ),
        };
        for (key, _) in heap.map_entries(map) {
            let key_name = match key {
                Value::Keyword(symbol) => value::symbol_name_ref(symbol),
                _ => "",
            };
            if !takes.contains(&key_name) {
                return Err(format!(
                    "{name} has no {}; it takes {}",
                    shown(key),
                    takes
                        .iter()
                        .map(|key| format!(":{key}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
        }
        let needed = |key: &str| get(key).ok_or_else(|| format!("{name} needs a :{key}"));
        let triple = |key: &str, value: Value| {
            numbers(key, value, 3).map(|values| [values[0], values[1], values[2]])
        };
        let heading = |key: &str, value: Value| {
            let direction = triple(key, value)?;
            if direction.iter().all(|axis| *axis == 0.0) {
                return Err(format!(":{key} must not be [0 0 0]"));
            }
            Ok(direction)
        };
        let position = triple("position", needed("position")?)?;
        let radiance = triple("radiance", needed("radiance")?)?;
        if radiance.iter().any(|channel| *channel < 0.0) {
            return Err(format!(
                ":radiance is 0 or more, not {}",
                shown(needed("radiance")?)
            ));
        }
        let direction = match shape {
            EmitterShape::Sphere => [0.0, -1.0, 0.0],
            EmitterShape::Disc | EmitterShape::Rect => heading("direction", needed("direction")?)?,
        };
        let up = match get("up") {
            Some(given) => heading("up", given)?,
            None => [0.0, 1.0, 0.0],
        };
        let extent = if shape == EmitterShape::Rect {
            let given = needed("extent")?;
            let extent = numbers("extent", given, 2)?;
            if extent.iter().any(|side| *side <= 0.0) {
                return Err(format!(
                    ":extent is [width height], metres above 0, not {}",
                    shown(given)
                ));
            }
            [extent[0], extent[1]]
        } else {
            let given = needed("size")?;
            match given {
                Value::Int(_) | Value::Float(_) | Value::Ratio(_) | Value::Decimal(_)
                    if num(given).is_finite() && num(given) > 0.0 =>
                {
                    [num(given), num(given)]
                }
                _ => {
                    return Err(format!(
                        ":size is its diameter, metres above 0, not {}",
                        shown(given)
                    ));
                }
            }
        };
        Ok(Emitter {
            shape,
            position,
            direction,
            up,
            extent,
            radiance,
        })
    };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| one(*entry).map_err(|why| format!("emitter {index}: {why}")))
        .collect()
}

/// A material's `:eye` (absent or nil: none) read into a `mesh::Eye` (ADR-796), or why it is
/// not one: `{:eyes [{:centre [x y z] :axis [x y z] :iris-uv [u v]} …] :iris-uv-radius r}` —
/// one or two eyes in the mesh's space at rest — with `:up [x y z]`, `:radius`,
/// `:iris-radius`, `:cornea-radius`, `:iris-depth`, `:ior`, `:pupil` and `:concavity` as it likes. A
/// mistake here is an eye drawn flat or looking the wrong way without a word, so each is
/// refused with what it should be.
fn eye_spec(
    heap: &Heap,
    given: Option<Value>,
    num: &dyn Fn(Value) -> f32,
) -> Result<Option<crate::host::gui::mesh::Eye>, String> {
    use crate::host::gui::mesh::{
        DEFAULT_CORNEA_RADIUS, DEFAULT_EYE_IOR, DEFAULT_EYE_RADIUS, DEFAULT_IRIS_CONCAVITY,
        DEFAULT_IRIS_DEPTH, DEFAULT_IRIS_RADIUS, DEFAULT_PUPIL, Eye, EyeAt, MAX_EYES,
        MAX_IRIS_CONCAVITY,
    };
    let map = match given {
        None | Some(Value::Nil) => return Ok(None),
        Some(Value::Map(map)) => map,
        Some(other) => {
            return Err(format!(
                "a material's :eye is a map of its eyes' optics ({{:eyes [{{:centre :axis :iris-uv}} …] :iris-uv-radius r …}}), not {}",
                crate::syntax::printer::print(heap, other)
            ));
        }
    };
    let print = |value: Value| crate::syntax::printer::print(heap, value);
    let is_number = |candidate: Value| {
        matches!(
            candidate,
            Value::Int(_) | Value::Float(_) | Value::Ratio(_) | Value::Decimal(_)
        )
    };
    // `count` finite numbers, or none
    let numbers = |given: Value, count: usize| -> Option<Vec<f32>> {
        match given {
            Value::Vector(id) if heap.vector(id).len() == count => heap
                .vector(id)
                .iter()
                .map(|item| Some(num(*item)).filter(|value| is_number(*item) && value.is_finite()))
                .collect(),
            _ => None,
        }
    };
    let direction = |within, key: &str, whose: &str| -> Result<Option<[f32; 3]>, String> {
        match heap.map_get(within, value::kw(key)) {
            None | Some(Value::Nil) => Ok(None),
            Some(given) => match numbers(given, 3) {
                Some(values) => {
                    let length = values.iter().map(|value| value * value).sum::<f32>().sqrt();
                    if length > 1e-6 {
                        Ok(Some([
                            values[0] / length,
                            values[1] / length,
                            values[2] / length,
                        ]))
                    } else {
                        Err(format!(
                            "{whose} :{key} is a direction [x y z], not {}: it has no length",
                            print(given)
                        ))
                    }
                }
                None => Err(format!(
                    "{whose} :{key} is a direction [x y z], not {}",
                    print(given)
                )),
            },
        }
    };
    // a number above `least` and at most `most`, or the default when the key is absent
    let sized =
        |key: &str, least: f32, most: f32, default: f32, what: &str| -> Result<f32, String> {
            match heap.map_get(map, value::kw(key)) {
                None | Some(Value::Nil) => Ok(default),
                Some(given) if is_number(given) && num(given) > least && num(given) <= most => {
                    Ok(num(given))
                }
                Some(given) => Err(format!(
                    "an :eye's :{key} is {what}, a number above {least} and at most {most}, not {}",
                    print(given)
                )),
            }
        };
    let entries = match heap.map_get(map, value::kw("eyes")) {
        Some(Value::Vector(id)) if (1..=MAX_EYES).contains(&heap.vector(id).len()) => {
            heap.vector(id)
        }
        other => {
            return Err(format!(
                "an :eye's :eyes is a vector of one or two eyes, each {{:centre [x y z] :axis [x y z] :iris-uv [u v]}}, not {}",
                other.map_or_else(|| "nothing".to_string(), print)
            ));
        }
    };
    let eyes = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let whose = format!("eye {index}'s");
            let Value::Map(entry_map) = *entry else {
                return Err(format!(
                    "eye {index} is a map {{:centre [x y z] :axis [x y z] :iris-uv [u v]}}, not {}",
                    print(*entry)
                ));
            };
            let place = |key: &str, count: usize, what: &str| -> Result<Vec<f32>, String> {
                match heap.map_get(entry_map, value::kw(key)) {
                    Some(given) => numbers(given, count)
                        .ok_or_else(|| format!("{whose} :{key} is {what}, not {}", print(given))),
                    None => Err(format!("{whose} :{key} is missing: {what}")),
                }
            };
            let centre = place(
                "centre",
                3,
                "the eyeball's centre in the mesh at rest, [x y z]",
            )?;
            let iris_uv = place(
                "iris-uv",
                2,
                "where the middle of its iris is in the base texture, [u v]",
            )?;
            let Some(axis) = direction(entry_map, "axis", &whose)? else {
                return Err(format!(
                    "{whose} :axis is missing: the direction it looks along in the mesh at rest, [x y z]"
                ));
            };
            Ok(EyeAt {
                centre: [centre[0], centre[1], centre[2]],
                axis,
                iris_uv: [iris_uv[0], iris_uv[1]],
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let up = direction(map, "up", "an :eye's")?.unwrap_or([0.0, 1.0, 0.0]);
    if let Some(index) = eyes.iter().position(|eye| {
        (eye.axis[0] * up[0] + eye.axis[1] * up[1] + eye.axis[2] * up[2]).abs() > 0.999
    }) {
        return Err(format!(
            "an :eye's :up runs along eye {index}'s :axis: it is the direction that is up in the iris's texture, across the axis"
        ));
    }
    let radius = sized(
        "radius",
        0.0,
        1.0e6,
        DEFAULT_EYE_RADIUS,
        "the eyeball's radius",
    )?;
    let iris_radius = sized(
        "iris-radius",
        0.0,
        1.0e6,
        DEFAULT_IRIS_RADIUS,
        "the iris's radius",
    )?;
    let cornea_radius = sized(
        "cornea-radius",
        0.0,
        1.0e6,
        DEFAULT_CORNEA_RADIUS,
        "the radius of the cornea's curve",
    )?;
    if iris_radius >= radius || iris_radius >= cornea_radius {
        return Err(format!(
            "an :eye's :iris-radius ({iris_radius}) is under its :radius ({radius}) and its :cornea-radius ({cornea_radius}): the cornea is a cap through the iris's edge on the eyeball"
        ));
    }
    let iris_depth = sized(
        "iris-depth",
        0.0,
        1.0e6,
        DEFAULT_IRIS_DEPTH,
        "how far the iris lies under the cornea's top",
    )?;
    let ior = match heap.map_get(map, value::kw("ior")) {
        None | Some(Value::Nil) => DEFAULT_EYE_IOR,
        Some(given) if is_number(given) && (1.0..=3.0).contains(&num(given)) => num(given),
        Some(given) => {
            return Err(format!(
                "an :eye's :ior is the refractive index its cornea bends light by, a number from 1 (not at all) to 3, not {}",
                print(given)
            ));
        }
    };
    let pupil = match heap.map_get(map, value::kw("pupil")) {
        None | Some(Value::Nil) => DEFAULT_PUPIL,
        Some(given) if is_number(given) && (0.0..=1.0).contains(&num(given)) => num(given),
        Some(given) => {
            return Err(format!(
                "an :eye's :pupil is the pupil's radius as a share of the iris's, a number from 0 (none drawn) to 1, not {}",
                print(given)
            ));
        }
    };
    let concavity = match heap.map_get(map, value::kw("concavity")) {
        None | Some(Value::Nil) => DEFAULT_IRIS_CONCAVITY,
        Some(given) if is_number(given) && (0.0..=MAX_IRIS_CONCAVITY).contains(&num(given)) => {
            num(given)
        }
        Some(given) => {
            return Err(format!(
                "an :eye's :concavity is how steeply the iris, lit as a shallow bowl, slopes up at its edge, a number from 0 (lit flat) to {MAX_IRIS_CONCAVITY}, not {}",
                print(given)
            ));
        }
    };
    let iris_uv_radius = match heap.map_get(map, value::kw("iris-uv-radius")) {
        Some(given) if is_number(given) && num(given) > 0.0 && num(given) <= 1.0 => num(given),
        other => {
            return Err(format!(
                "an :eye's :iris-uv-radius is how far the iris's edge is from its middle in the base texture, a number above 0 and at most 1, not {}",
                other.map_or_else(|| "nothing".to_string(), print)
            ));
        }
    };
    Ok(Some(Eye {
        eyes,
        up,
        radius,
        iris_radius,
        cornea_radius,
        iris_depth,
        ior,
        iris_uv_radius,
        pupil,
        concavity,
    }))
}

/// A scene spec's `:lights` (absent or nil: none) read into `mesh::Light`s, or why they are
/// not lights: not a vector, more than `MAX_LIGHTS`, a light that is not one ([`light_spec`]),
/// more than `MAX_SHADOWS` shadowed, or shadow maps past `MAX_SHADOW_TEXELS` together as the
/// two map arrays hold them (`ShadowArrays::texels`, ADR-800).
fn lights_spec(
    heap: &Heap,
    given: Option<Value>,
    num: &dyn Fn(Value) -> f32,
) -> Result<Vec<crate::host::gui::mesh::Light>, String> {
    use crate::host::gui::mesh::{MAX_LIGHTS, MAX_SHADOW_TEXELS, MAX_SHADOWS, ShadowArrays};
    let id = match given {
        None | Some(Value::Nil) => return Ok(Vec::new()),
        Some(Value::Vector(id)) => id,
        Some(other) => {
            return Err(format!(
                ":lights is a vector of light maps, not {}",
                crate::syntax::printer::print(heap, other)
            ));
        }
    };
    let entries = heap.vector(id);
    if entries.len() > MAX_LIGHTS {
        return Err(format!(
            "{} :lights is more than the {MAX_LIGHTS} a scene lights with",
            entries.len()
        ));
    }
    let lights = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            light_spec(heap, *entry, num).map_err(|why| format!("light {index}: {why}"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let shadowed: Vec<_> = lights
        .iter()
        .filter(|light| light.shadow.is_some())
        .collect();
    if shadowed.len() > MAX_SHADOWS {
        return Err(format!(
            "{} lights cast shadows, more than the {MAX_SHADOWS} a scene may have",
            shadowed.len()
        ));
    }
    // the GPU target keeps the maps in two arrays (ADR-800): the largest maps' at their size,
    // every smaller map as a layer as big as the largest of those
    let resolutions: Vec<u32> = shadowed
        .iter()
        .filter_map(|light| {
            let shadow = light.shadow.as_ref()?;
            Some(std::iter::repeat_n(shadow.resolution, light.shadow_views()))
        })
        .flatten()
        .collect();
    let arrays = ShadowArrays::of(&resolutions);
    let texels = arrays.texels();
    if texels > MAX_SHADOW_TEXELS {
        let [large, small] = arrays.sides;
        let [in_large, in_small] = arrays.layers;
        let maps_of = |count: u32, side: u32| {
            let plural = if count == 1 { "" } else { "s" };
            format!("{count} map{plural} of {side}²")
        };
        let maps = if in_small == 0 {
            maps_of(in_large, large)
        } else {
            format!(
                "{} and {}",
                maps_of(in_large, large),
                maps_of(in_small, small)
            )
        };
        return Err(format!(
            "the shadow maps need {texels} texels together ({maps}), more than \
             {MAX_SHADOW_TEXELS} (a point light's shadow is 6 maps): lower a :resolution"
        ));
    }
    Ok(lights)
}

/// One `:lights` entry read into a `mesh::Light` (ADR-788), or why it is not one. Its keys
/// say its kind: `:position` and `:cone` a spot, `:position` alone a point light, neither a
/// directional light; a key that kind does not take is refused, as is a value of the wrong
/// shape.
///
/// - directional: `{:direction [x y z] :color [r g b] :shadow …}`, `:direction` required, the
///   way the light travels.
/// - point: `{:position [x y z] :color [r g b] :range r :shadow …}`.
/// - spot: `{:position [x y z] :direction [x y z] :color [r g b] :range r :cone [inner outer]
///   :shadow …}`, the half-angles in radians, `0 ≤ inner ≤ outer < π/2`.
///
/// `:color` (linear, 0 or more; white by default) is a point or spot light's light 1 m away.
/// `:size` (a point or spot light's diameter, metres) or `:angle` (a directional light's apparent
/// diameter, radians), 0 or more, softens its shadow by distance (ADR-793).
/// `:range` (above 0) windows its falloff to nothing there. `:shadow` is `true`, `false` or a
/// map of `:resolution` (texels, 16 … 4096), `:bias` and `:normal-bias` (texels, 0 or more),
/// `:softness` (metres, 0 or more: the radius its edge is filtered over at the receiver; with
/// a `:size` or `:angle`, the most its penumbra grows to),
/// for a directional light `:extent` (metres, above 0) and `:center [x y z]`, for a spot or
/// point light `:near` (metres, above 0).
fn light_spec(
    heap: &Heap,
    given: Value,
    num: &dyn Fn(Value) -> f32,
) -> Result<crate::host::gui::mesh::Light, String> {
    use crate::host::gui::mesh::{
        EmitterShape, Light, LightKind, LightShape, MAX_SHADOW_SIDE, MIN_SHADOW_SIDE, Shadow,
    };
    let shown = |value: Value| crate::syntax::printer::print(heap, value);
    let Value::Map(map) = given else {
        return Err(format!("a light is a map, not {}", shown(given)));
    };
    let get = |within, key: &str| heap.map_get(within, value::kw(key));
    let number = |value: Value| match value {
        Value::Int(_) | Value::Float(_) | Value::Ratio(_) | Value::Decimal(_) => {
            Some(num(value)).filter(|number| number.is_finite())
        }
        _ => None,
    };
    let numbers = |key: &str, value: Value, count: usize| -> Result<Vec<f32>, String> {
        let parsed = match value {
            Value::Vector(id) if heap.vector(id).len() == count => heap
                .vector(id)
                .iter()
                .map(|item| number(*item))
                .collect::<Option<Vec<f32>>>(),
            _ => None,
        };
        parsed.ok_or_else(|| format!(":{key} is {count} numbers, not {}", shown(value)))
    };
    let triple = |key: &str, value: Value| {
        numbers(key, value, 3).map(|values| [values[0], values[1], values[2]])
    };
    let positive = |key: &str, value: Value| match number(value) {
        Some(amount) if amount > 0.0 => Ok(amount),
        _ => Err(format!(":{key} is a number above 0, not {}", shown(value))),
    };
    let kind = match (get(map, "position").is_some(), get(map, "cone").is_some()) {
        (false, _) => LightKind::Directional,
        (true, true) => LightKind::Spot,
        (true, false) => LightKind::Point,
    };
    let (name, takes): (&str, &[&str]) = match kind {
        LightKind::Directional => (
            "a directional light",
            &["direction", "color", "angle", "shadow", "shape"],
        ),
        LightKind::Point => (
            "a point light",
            &["position", "color", "range", "size", "shadow", "shape"],
        ),
        LightKind::Spot => (
            "a spot light",
            &[
                "position",
                "direction",
                "color",
                "range",
                "cone",
                "size",
                "shadow",
                "shape",
                "extent",
                "up",
            ],
        ),
    };
    let mut light = Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0]);
    light.kind = kind;
    let mut has_direction = false;
    // the shape glossy surfaces show it as (ADR-795), put together after every key is read
    let mut shape = None;
    let mut extent = None;
    let mut up = None;
    for (key, value) in heap.map_entries(map) {
        let key_name = match key {
            Value::Keyword(symbol) => value::symbol_name_ref(symbol),
            _ => "",
        };
        if !takes.contains(&key_name) {
            let hint = match (kind, key_name) {
                (LightKind::Point, "direction") => " (give it a :cone to make it a spot)",
                (LightKind::Directional, "range" | "cone") => {
                    " (give it a :position to make it a point or spot light)"
                }
                (LightKind::Directional, "size") => {
                    " (a directional light's size is its :angle, in radians)"
                }
                (_, "angle") => " (a point or spot light's size is its :size, in metres)",
                (_, "extent" | "up") => " (a :shape :rect's, which only a spot light takes)",
                _ => "",
            };
            return Err(format!(
                "{name} has no {}{hint}; it takes {}",
                shown(key),
                takes
                    .iter()
                    .map(|key| format!(":{key}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        match key_name {
            "direction" => {
                let direction = triple("direction", value)?;
                if direction.iter().all(|axis| *axis == 0.0) {
                    return Err(":direction must not be [0 0 0]".to_string());
                }
                light.direction = direction;
                has_direction = true;
            }
            "position" => light.position = triple("position", value)?,
            "color" => {
                let color = triple("color", value)?;
                if color.iter().any(|channel| *channel < 0.0) {
                    return Err(format!(":color is 0 or more, not {}", shown(value)));
                }
                light.color = color;
            }
            "range" => light.range = positive("range", value)?,
            "shape" => shape = Some(emitter_shape(heap, value)?),
            "extent" => {
                let sides = numbers("extent", value, 2)?;
                if sides.iter().any(|side| *side <= 0.0) {
                    return Err(format!(
                        ":extent is [width height], metres above 0, not {}",
                        shown(value)
                    ));
                }
                extent = Some([sides[0], sides[1]]);
            }
            "up" => {
                let heading = triple("up", value)?;
                if heading.iter().all(|axis| *axis == 0.0) {
                    return Err(":up must not be [0 0 0]".to_string());
                }
                up = Some(heading);
            }
            "size" | "angle" => {
                light.size = number(value)
                    .filter(|amount| *amount >= 0.0)
                    .ok_or_else(|| {
                        format!(
                            ":{key_name} is 0 or more {}, not {}",
                            if key_name == "size" {
                                "metres"
                            } else {
                                "radians"
                            },
                            shown(value)
                        )
                    })?;
            }
            "cone" => {
                let cone = numbers("cone", value, 2)?;
                if !(0.0 <= cone[0] && cone[0] <= cone[1] && cone[1] < std::f32::consts::FRAC_PI_2)
                {
                    return Err(format!(
                        ":cone is [inner outer] half-angles in radians, 0 ≤ inner ≤ outer < π/2, not {}",
                        shown(value)
                    ));
                }
                light.cone = [cone[0], cone[1]];
            }
            _ => {
                // "shadow"
                light.shadow = match value {
                    Value::Bool(false) | Value::Nil => None,
                    Value::Bool(true) => Some(Shadow::default()),
                    Value::Map(options) => {
                        let mut shadow = Shadow::default();
                        let directional = kind == LightKind::Directional;
                        for (option, setting) in heap.map_entries(options) {
                            let option_name = match option {
                                Value::Keyword(symbol) => value::symbol_name_ref(symbol),
                                _ => "",
                            };
                            match (option_name, directional) {
                                ("resolution", _) => {
                                    shadow.resolution = match setting {
                                        Value::Int(side)
                                            if (i64::from(MIN_SHADOW_SIDE)
                                                ..=i64::from(MAX_SHADOW_SIDE))
                                                .contains(&side) =>
                                        {
                                            side as u32
                                        }
                                        _ => {
                                            return Err(format!(
                                                ":shadow :resolution is a whole number of texels, {MIN_SHADOW_SIDE} … {MAX_SHADOW_SIDE}, not {}",
                                                shown(setting)
                                            ));
                                        }
                                    }
                                }
                                ("bias" | "normal-bias", _) => {
                                    let amount = number(setting)
                                        .filter(|amount| *amount >= 0.0)
                                        .ok_or_else(|| {
                                            format!(
                                                ":shadow :{option_name} is 0 or more texels, not {}",
                                                shown(setting)
                                            )
                                        })?;
                                    if option_name == "bias" {
                                        shadow.bias = amount;
                                    } else {
                                        shadow.normal_bias = amount;
                                    }
                                }
                                ("extent", true) => {
                                    shadow.extent = Some(positive("extent", setting)?)
                                }
                                ("center", true) => {
                                    shadow.center = Some(triple("center", setting)?)
                                }
                                ("near", false) => shadow.near = positive("near", setting)?,
                                ("softness", _) => {
                                    shadow.softness = number(setting)
                                        .filter(|amount| *amount >= 0.0 && amount.is_finite())
                                        .ok_or_else(|| {
                                            format!(
                                                ":shadow :softness is 0 or more metres, not {}",
                                                shown(setting)
                                            )
                                        })?
                                }
                                _ => {
                                    return Err(format!(
                                        "{name}'s :shadow has no {}; it takes :resolution :bias :normal-bias :softness {}",
                                        shown(option),
                                        if directional {
                                            ":extent :center"
                                        } else {
                                            ":near"
                                        }
                                    ));
                                }
                            }
                        }
                        Some(shadow)
                    }
                    other => {
                        return Err(format!(
                            ":shadow is true, false or a map of options, not {}",
                            shown(other)
                        ));
                    }
                };
            }
        }
    }
    if kind != LightKind::Point && !has_direction {
        return Err(format!("{name} needs a :direction"));
    }
    // a shape needs the size it is drawn at, and only the kind of light that can have it
    let is_rect = shape == Some(EmitterShape::Rect);
    if !is_rect && (extent.is_some() || up.is_some()) {
        return Err(":extent and :up are a :shape :rect's".to_string());
    }
    if let Some(shape) = shape {
        match (kind, shape) {
            (LightKind::Directional, EmitterShape::Rect) => {
                return Err(
                    "a directional light's :shape is a :disc (or :sphere) of its :angle, not :rect"
                        .to_string(),
                );
            }
            (LightKind::Point, EmitterShape::Disc | EmitterShape::Rect) => {
                return Err(
                    "a point light's :shape is :sphere; a :disc or :rect faces a :direction (give it a :cone to make it a spot)"
                        .to_string(),
                );
            }
            (_, EmitterShape::Rect) if extent.is_none() => {
                return Err(
                    ":shape :rect needs an :extent [width height], metres above 0".to_string(),
                );
            }
            (LightKind::Directional, _) if light.size <= 0.0 => {
                return Err(
                    "a directional light's :shape is as big as its :angle, radians above 0: give it one"
                        .to_string(),
                );
            }
            (_, EmitterShape::Disc | EmitterShape::Sphere) if light.size <= 0.0 => {
                return Err(format!(
                    ":shape {} is as big as the light's :size, its diameter in metres above 0: give it one",
                    if shape == EmitterShape::Disc {
                        ":disc"
                    } else {
                        ":sphere"
                    }
                ));
            }
            _ => {}
        }
        light.shape = Some(LightShape {
            shape,
            extent: extent.unwrap_or([0.0, 0.0]),
            up: up.unwrap_or([0.0, 1.0, 0.0]),
        });
    }
    Ok(light)
}

/// A byte-valued key of a `%gui-mesh` spec: its bytes, or none when the key is absent or nil.
fn mesh_bytes(
    heap: &mut Heap,
    map: crate::core::value::MapId,
    key: &str,
) -> Result<Vec<u8>, LispError> {
    match heap.map_get(map, value::kw(key)) {
        None | Some(Value::Nil) => Ok(Vec::new()),
        Some(given) => collect_bytes("%gui-mesh", given, heap),
    }
}

/// `(%gui-mesh id handle spec)` — keep the mesh `spec` describes under `handle` for window
/// `id`'s `[:scene …]` ops (ADR-786). Its attributes are little-endian bytes as a glTF buffer
/// holds them: `:positions` (3 f32 a vertex, required), `:normals` (3 f32), `:uvs` (2 f32),
/// `:tangents` (4 f32), `:joints` (4 a vertex, `:joint-type :u8` or `:u16`), `:weights`
/// (4 f32), `:indices` (`:index-type :u16` or `:u32`) and `:morphs`, a vector of
/// `{:positions bytes :normals bytes}`. Validated here, so a mesh that lies about its counts
/// is a clean error, never a draw that reads past its data. The handle is the caller's
/// (`gui/mesh` allocates it). Returns nil.
pub(in crate::builtins) fn gui_mesh(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-mesh", arg(args, 0))?;
    let requested = expect_int(heap, "%gui-mesh", arg(args, 1))?;
    let Ok(handle) = u32::try_from(requested) else {
        return Err(LispError::runtime(format!(
            "%gui-mesh: mesh handle must be a non-negative int, got {requested}"
        )));
    };
    let Value::Map(map) = arg(args, 2) else {
        return Err(LispError::wrong_type(
            heap,
            "%gui-mesh",
            "a mesh spec map",
            arg(args, 2),
        ));
    };
    let keyword_is = |heap: &Heap, key: &str, wanted: &str| matches!(heap.map_get(map, value::kw(key)), Some(Value::Keyword(found)) if value::symbol_name_ref(found) == wanted);
    let joints_wide = keyword_is(heap, "joint-type", "u16");
    let indices_wide = keyword_is(heap, "index-type", "u32");
    let mut parts = crate::host::gui::mesh::MeshParts {
        positions: mesh_bytes(heap, map, "positions")?,
        normals: mesh_bytes(heap, map, "normals")?,
        uvs: mesh_bytes(heap, map, "uvs")?,
        tangents: mesh_bytes(heap, map, "tangents")?,
        joints: mesh_bytes(heap, map, "joints")?,
        joints_wide,
        weights: mesh_bytes(heap, map, "weights")?,
        indices: mesh_bytes(heap, map, "indices")?,
        indices_wide,
        morphs: Vec::new(),
        softness: mesh_bytes(heap, map, "softness")?,
    };
    if let Some(Value::Vector(morphs)) = heap.map_get(map, value::kw("morphs")) {
        let targets = heap.vector(morphs).to_vec();
        for target in targets {
            let Value::Map(target) = target else {
                return Err(LispError::wrong_type(
                    heap,
                    "%gui-mesh",
                    "a morph target map",
                    target,
                ));
            };
            let positions = mesh_bytes(heap, target, "positions")?;
            let normals = mesh_bytes(heap, target, "normals")?;
            parts.morphs.push((positions, normals));
        }
    }
    let mesh = crate::host::gui::mesh::Mesh::from_parts(parts)
        .map_err(|why| LispError::runtime(format!("%gui-mesh: {why}")))?;
    crate::host::gui::mesh(id, handle, std::sync::Arc::new(mesh)).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(%gui-mesh-free id handle)` — forget mesh `handle` of window `id`.
pub(in crate::builtins) fn gui_mesh_free(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-mesh-free", arg(args, 0))?;
    let requested = expect_int(heap, "%gui-mesh-free", arg(args, 1))?;
    let Ok(handle) = u32::try_from(requested) else {
        return Ok(Value::nil());
    };
    crate::host::gui::mesh_free(id, handle).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

pub(in crate::builtins) fn gui_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let win = gui_window_id(heap, "%gui-draw", arg(args, 0))?;
    let parsed = frame_ops(heap, arg(args, 1), "%gui-draw", "vector (a frame)")?;
    let tags = GuiOpTags::new();
    let mut pass = fragments::FragmentPass::for_window(win);
    let ops = parse_gui_ops(heap, parsed, &tags, &mut pass);
    tags.refused("%gui-draw")?;
    crate::host::gui::draw(win, ops).map_err(LispError::runtime)?;
    pass.store(win);
    Ok(Value::nil())
}

/// `(%gui-render-frame frame spec)` — paint `frame` into memory as a window `spec`
/// describes would paint it, and answer the picture (ADR-609). The frame is parsed as
/// `%gui-draw` parses one; a fragment marker paints its ops (nothing is kept for a
/// later frame — there is no window to keep them for).
pub(in crate::builtins) fn gui_render_frame(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let parsed = frame_ops(heap, arg(args, 0), "%gui-render-frame", "vector (a frame)")?;
    let spec = render_spec(heap, arg(args, 1))?;
    let tags = GuiOpTags::new();
    let mut pass = fragments::FragmentPass::empty();
    let ops = parse_gui_ops(heap, parsed, &tags, &mut pass);
    tags.refused("%gui-render-frame")?;
    let image = crate::host::gui::render_frame(ops, spec).map_err(LispError::runtime)?;
    Ok(frame_image_value(heap, image))
}

/// `(%gui-capture id)` — window `id`'s current frame painted into memory (ADR-609).
pub(in crate::builtins) fn gui_capture(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let win = gui_window_id(heap, "%gui-capture", arg(args, 0))?;
    let image = crate::host::gui::capture(win).map_err(LispError::runtime)?;
    Ok(frame_image_value(heap, image))
}

/// `(%gui-windows)` — the ids of the open windows, oldest first.
/// `(%gui-scene-gpu-times)` — the GPU's own time on recent frames' scenes (ADR-801), oldest
/// first, as maps of milliseconds: `:shadow`, `:scene`, and `:scenes` timed.
pub(in crate::builtins) fn gui_scene_gpu_times(
    _: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let samples = crate::host::gui::scene_gpu_times::scene_gpu_times();
    let mut maps = Vec::with_capacity(samples.len());
    for sample in samples {
        maps.push(heap.map_from_pairs(vec![
            (value::kw("shadow"), Value::float(sample.shadow_us / 1000.0)),
            (value::kw("scene"), Value::float(sample.scene_us / 1000.0)),
            (value::kw("scenes"), Value::int(i64::from(sample.scenes))),
        ]));
    }
    Ok(heap.alloc_vector(maps))
}

pub(in crate::builtins) fn gui_windows(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let ids = crate::host::gui::window_ids()
        .into_iter()
        .map(|id| Value::int(id as i64))
        .collect();
    Ok(heap.alloc_vector(ids))
}

/// A picture as Brood sees it: `{:width :height :rgba :cell [w h] :grid [cols rows]
/// :origin [x y] :inset px :seam row-or-nil :row-tops [y …]}` — `gui/image-thumb`'s `{:width :height :rgba}` plus the
/// geometry it was painted with, so `gui/write-png` and `gui/texture` take it as is.
fn frame_image_value(heap: &mut Heap, image: crate::host::gui::FrameImage) -> Value {
    let pair = |heap: &mut Heap, a: u32, b: u32| {
        heap.alloc_vector(vec![Value::int(a as i64), Value::int(b as i64)])
    };
    // GC-safe: a builtin never collects mid-execution, so the handles below stay live
    // until the map holds them (as `image_thumb` does).
    let rgba = crate::builtins::bytes::bytes_to_value(&image.rgba, heap);
    let cell = pair(heap, image.cell.0, image.cell.1);
    let grid = pair(heap, image.grid.0 as u32, image.grid.1 as u32);
    let origin = pair(heap, image.origin.0, image.origin.1);
    let row_tops = heap.alloc_vector(
        image
            .row_tops
            .iter()
            .map(|&y| Value::int(y as i64))
            .collect(),
    );
    let seam = image
        .seam
        .map_or(Value::nil(), |row| Value::int(row as i64));
    let pairs = vec![
        (value::kw("width"), Value::int(image.width as i64)),
        (value::kw("height"), Value::int(image.height as i64)),
        (value::kw("rgba"), rgba),
        (value::kw("cell"), cell),
        (value::kw("grid"), grid),
        (value::kw("origin"), origin),
        (value::kw("inset"), Value::int(image.inset as i64)),
        (value::kw("seam"), seam),
        (value::kw("row-tops"), row_tops),
    ];
    heap.map_from_pairs(pairs)
}

/// The window a `%gui-render-frame` spec map describes: `:width`/`:height` (physical
/// px), `:font` (an installed family's name, or a family keyword), `:size` (the font px),
/// `:line-height`, `:inset`, `:bg` and `:fg` (colours), `:scale`, `:text-aa` and `:ligatures`.
/// nil is every default.
fn render_spec(heap: &Heap, spec: Value) -> Result<crate::host::gui::RenderSpec, LispError> {
    use crate::host::gui::{FontChoice, RenderSpec, TextAa};
    let who = "%gui-render-frame";
    let map = match spec {
        Value::Nil => return Ok(RenderSpec::default()),
        Value::Map(id) => id,
        other => {
            return Err(LispError::wrong_type(
                heap,
                who,
                "map (a render spec)",
                other,
            ));
        }
    };
    let get = |name: &str| {
        heap.map_get(map, value::kw(name))
            .filter(|v| !matches!(v, Value::Nil))
    };
    let number = |name: &str| -> Result<Option<f64>, LispError> {
        match get(name) {
            None => Ok(None),
            Some(Value::Int(n)) => Ok(Some(n as f64)),
            Some(Value::Float(f)) => Ok(Some(f)),
            Some(other) => Err(LispError::wrong_type(heap, who, "a number", other)),
        }
    };
    let pixels = |name: &str| -> Result<Option<u32>, LispError> {
        Ok(number(name)?.map(|n| n.round().clamp(1.0, u32::MAX as f64) as u32))
    };
    let size = match (pixels("width")?, pixels("height")?) {
        (None, None) => None,
        (w, h) => Some((w.unwrap_or(840), h.unwrap_or(560))),
    };
    let font = match get("font") {
        None => None,
        Some(Value::Keyword(k)) => Some(FontChoice::Registered(k)),
        Some(other) => Some(FontChoice::Installed(expect_string(heap, who, other)?)),
    };
    let bg = match get("bg") {
        None => None,
        Some(colour) => Some(
            face_rgb(heap, colour)
                .ok_or_else(|| LispError::wrong_type(heap, who, "a colour for :bg", colour))?,
        ),
    };
    let fg = match get("fg") {
        None => None,
        Some(colour) => Some(
            face_rgb(heap, colour)
                .ok_or_else(|| LispError::wrong_type(heap, who, "a colour for :fg", colour))?,
        ),
    };
    let text_aa = match get("text-aa") {
        None => None,
        Some(Value::Keyword(k)) => Some(match value::symbol_name_ref(k) {
            "gray" | "grey" | "grayscale" => TextAa::Gray,
            "subpixel" | "rgb" | "lcd" => TextAa::Subpixel,
            "bgr" => TextAa::Bgr,
            _ => TextAa::Auto,
        }),
        Some(other) => {
            return Err(LispError::wrong_type(
                heap,
                who,
                "a :text-aa keyword",
                other,
            ));
        }
    };
    let text_hinting = get("text-hinting")
        .map(|mode| text_hinting_of(heap, who, mode))
        .transpose()?;
    Ok(RenderSpec {
        size,
        font,
        px: number("size")?.map(|n| n as f32),
        line_height: number("line-height")?.map(|n| n as f32),
        inset: number("inset")?.map(|n| n.max(0.0) as f32),
        bg,
        fg,
        scale: number("scale")?,
        text_aa,
        text_hinting,
        ligatures: get("ligatures").map(crate::eval::truthy),
    })
}

/// `ops` (a fragment's, as a marker carries them) as the `(tag, parts)` pairs
/// `parse_gui_ops` reads — `frame_ops` for a sequence already in hand.
fn tagged_ops(heap: &Heap, ops: Vec<Value>) -> Vec<(value::Symbol, Vec<Value>)> {
    ops.into_iter()
        .filter_map(|op| match op {
            Value::Vector(id) => {
                let parts = heap.vector(id).to_vec();
                match parts.first() {
                    Some(Value::Keyword(tag)) => Some((*tag, parts)),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// The blink of a `[:cursor … blink]` op: a map `{:interval ms :blinks n}` (`:interval`
/// defaults to 530 ms, Emacs's `blink-cursor-interval` near enough, floored at 50;
/// `:blinks` to 0, blink for as long as no input comes), or `None` for anything else.
fn cursor_blink_from(heap: &Heap, v: Value) -> Option<crate::host::gui::CursorBlink> {
    let Value::Map(id) = v else {
        return None;
    };
    let count = |name: &str, default: i64| match heap.map_get(id, value::kw(name)) {
        Some(Value::Int(n)) => n,
        _ => default,
    };
    Some(crate::host::gui::CursorBlink {
        interval_ms: count("interval", 530).clamp(50, 60_000) as u32,
        blinks: count("blinks", 0).clamp(0, 1_000_000) as u32,
    })
}
/// A `:vspans` segment colour: a face colour keyword (`:red` → the GUI palette),
/// an explicit `[r g b]` triple, or a `"#rrggbb"` hex string (all via the shared
/// `face_rgb`); anything else is `None` — "transparent", leaving the background
/// showing.
pub(in crate::builtins) fn span_color(heap: &Heap, v: Value) -> Option<[u8; 3]> {
    face_rgb(heap, v)
}

/// Read a `:height` value from a font spec as a pixel size (int or float), or None.
pub(in crate::builtins) fn font_px(heap: &Heap, id: crate::core::value::MapId) -> Option<f32> {
    match heap.map_get(id, value::kw("height")) {
        Some(Value::Int(n)) => Some(n as f32),
        Some(Value::Float(f)) => Some(f as f32),
        _ => None,
    }
}

/// `(gui-font! spec)` / `(gui-font! id spec)` — set a cell font from `spec`, a map
/// `{:family <keyword> :height <px>}` (either key optional): `:family` picks a
/// registered font family (the bundled `:mono`, or one added by
/// `gui-font-register`), `:height` the cell pixel size. With one argument it sets
/// the **global default** (every open window + any opened later); with a leading
/// window `id` it retunes **just that window**, leaving the global default and
/// other windows alone — so two windows can run different fonts. (Per-section
/// fonts within a window come from a face's `:family`/`:scale`.) Returns nil.
pub(in crate::builtins) fn gui_font(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // (gui-font! spec) → global default; (gui-font! id spec) → just window `id`.
    let (win, spec) = if args.len() >= 2 {
        (
            Some(gui_window_id(heap, "%gui-font!", arg(args, 0))?),
            arg(args, 1),
        )
    } else {
        (None, arg(args, 0))
    };
    let Value::Map(m) = spec else {
        return Err(LispError::wrong_type(
            heap,
            "%gui-font!",
            "map (a font spec)",
            spec,
        ));
    };
    let family = match heap.map_get(m, value::kw("family")) {
        Some(Value::Keyword(s)) => Some(s),
        _ => None,
    };
    crate::host::gui::font(win, family, font_px(heap, m)).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-inset! px)` — set the window content inset (logical pixels): a blank margin
/// before the cell grid on every edge, so text doesn't sit flush against the window
/// frame. Applies to every open window + the default for ones opened later. The grid
/// loses `2*px` per axis (fewer cells) and re-renders. GUI only.
pub(in crate::builtins) fn gui_inset(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let px = match arg(args, 0) {
        Value::Int(n) => n.max(0) as f32,
        Value::Float(f) => f.max(0.0) as f32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-inset!",
                "a number (pixels)",
                other,
            ));
        }
    };
    crate::host::gui::inset(px).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-bg! color)` — set the window background: the fill for `:clear`, the pre-clear,
/// and (being outside every cell) the inset margin + the cell-grid snap remainder. So a
/// GUI app's padding matches its own theme instead of the hardcoded default. `color` is
/// a keyword (`:base`-style named colour), an `[r g b]` vector, or a `"#rrggbb"` hex
/// string; `nil` restores the default. Applies to every open window + ones opened later.
/// GUI only; returns nil.
pub(in crate::builtins) fn gui_bg(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let rgb = match arg(args, 0) {
        Value::Nil => None,
        other => Some(face_rgb(heap, other).ok_or_else(|| {
            LispError::wrong_type(
                heap,
                "%gui-bg!",
                "a colour (keyword, [r g b] vector, or \"#rrggbb\" string) or nil",
                other,
            )
        })?),
    };
    crate::host::gui::bg(rgb).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-fg! color)` — set the window's default text colour: what a text op whose face
/// names no `:fg` is drawn in, as `gui-bg!` is the colour no op paints. So a light theme's
/// plain text is its own dark text instead of the hardcoded light default (ADR-613). Same
/// colour forms as `gui-bg!`; `nil` restores the default. Every open window + ones opened
/// later. GUI only; returns nil.
pub(in crate::builtins) fn gui_fg(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let rgb = match arg(args, 0) {
        Value::Nil => None,
        other => Some(face_rgb(heap, other).ok_or_else(|| {
            LispError::wrong_type(
                heap,
                "%gui-fg!",
                "a colour (keyword, [r g b] vector, or \"#rrggbb\" string) or nil",
                other,
            )
        })?),
    };
    crate::host::gui::fg(rgb).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-line-height! mult)` — set the cell height as a multiple of the font px on
/// every open window + the default for ones opened later. GUI only; returns nil.
pub(in crate::builtins) fn gui_line_height(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let mult = match arg(args, 0) {
        Value::Int(n) => n as f32,
        Value::Float(f) => f as f32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-line-height!",
                "a number (a multiple of the font size)",
                other,
            ));
        }
    };
    crate::host::gui::line_height(mult).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-text-aa! mode)` — set how monochrome text is anti-aliased (`:auto` / `:gray` /
/// `:subpixel` / `:bgr`) on every open window + the default for ones opened later.
/// GUI only; returns nil.
pub(in crate::builtins) fn gui_text_aa(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::host::gui::TextAa;
    let mode = match arg(args, 0) {
        Value::Keyword(k) => match value::symbol_name_ref(k) {
            "auto" => TextAa::Auto,
            "gray" | "grey" | "grayscale" => TextAa::Gray,
            "subpixel" | "rgb" | "lcd" => TextAa::Subpixel,
            "bgr" => TextAa::Bgr,
            _ => {
                return Err(LispError::wrong_type(
                    heap,
                    "%gui-text-aa!",
                    "one of :auto, :gray, :subpixel, :bgr",
                    arg(args, 0),
                ));
            }
        },
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-text-aa!",
                "a keyword: :auto, :gray, :subpixel, or :bgr",
                other,
            ));
        }
    };
    crate::host::gui::text_aa(mode).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-font-register name styles)` — register font family `name` (a keyword) from
/// `styles`, either an installed family by name — `{:family-name "JetBrains Mono"}`,
/// resolved on the GUI thread against the system fonts, bold/italic picked by weight and
/// slant, the bundled DejaVu standing in when nothing by that name is installed
/// (`gui-font-info` says which) — or a map of style → TTF file path: `{:regular "…"
/// :bold "…" :italic "…" :bold-italic "…"}`. With paths only `:regular` is required; a
/// missing style reuses the regular file (so a single-file family works). The fonts are
/// read here and parsed on the GUI thread; afterwards a face's `:family <name>` selects
/// them. `:family-name` wins when both are given. Returns `name`.
pub(in crate::builtins) fn gui_font_register(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-font-register",
                "keyword",
                other,
            ));
        }
    };
    let Value::Map(id) = arg(args, 1) else {
        return Err(LispError::wrong_type(
            heap,
            "%gui-font-register",
            "map (style → path, or {:family-name \"…\"})",
            arg(args, 1),
        ));
    };
    // a style's path, or None when the key is absent/nil
    let path = |key: &str| -> Result<Option<String>, LispError> {
        match heap.map_get(id, value::kw(key)) {
            None | Some(Value::Nil) => Ok(None),
            Some(v) => Ok(Some(expect_string(heap, "%gui-font-register", v)?)),
        }
    };
    if let Some(family_name) = path("family-name")? {
        crate::host::gui::register_installed_family(name, family_name)
            .map_err(LispError::runtime)?;
        return Ok(Value::keyword(name));
    }
    let read = |p: &str| -> Result<Vec<u8>, LispError> {
        std::fs::read(p).map_err(|e| LispError::runtime(format!("%gui-font-register: {p}: {e}")))
    };
    let regular_path = path("regular")?.ok_or_else(|| {
        LispError::runtime("%gui-font-register: a :regular path (or a :family-name) is required")
    })?;
    let regular = read(&regular_path)?;
    // each missing style falls back to the regular file's bytes (and its path)
    let style = |key: &str| -> Result<(Vec<u8>, String), LispError> {
        match path(key)? {
            Some(p) => Ok((read(&p)?, p)),
            None => Ok((regular.clone(), regular_path.clone())),
        }
    };
    let (bold, bold_path) = style("bold")?;
    let (italic, italic_path) = style("italic")?;
    let (bold_italic, bold_italic_path) = style("bold-italic")?;
    crate::host::gui::register_family(
        name,
        [regular, bold, italic, bold_italic],
        [regular_path, bold_path, italic_path, bold_italic_path],
    )
    .map_err(LispError::runtime)?;
    Ok(Value::keyword(name))
}

#[cfg(test)]
mod gui_face_tests {
    use super::gui_face;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    // `gui_face` is the seam between a Brood face map and the GUI backend; verify it
    // reads the per-section font keys (`:family`/`:italic`) + flags. No window needed.
    #[test]
    fn reads_family_italic_and_flags() {
        let mut heap = Heap::new();
        let mono = value::intern("mono");
        let face = heap.map_from_pairs(vec![
            (value::kw("fg"), value::kw("red")),
            (value::kw("bold"), Value::boolean(true)),
            (value::kw("italic"), Value::boolean(true)),
            (value::kw("underline"), Value::boolean(true)),
            (value::kw("family"), Value::keyword(mono)),
        ]);
        let f = gui_face(&heap, face);
        assert_eq!(f.fg, Some([0xcd, 0x31, 0x31]));
        assert!(f.bold);
        assert!(f.italic);
        assert!(f.underline);
        assert_eq!(f.family, Some(mono));
    }

    // A `#rrggbb`/`#rgb` hex colour parses; a 6-*byte* string holding a multi-byte
    // char must NOT panic on a non-char-boundary slice (it just isn't valid hex).
    #[test]
    fn parse_hex_color_handles_multibyte_without_panic() {
        use crate::builtins::display::parse_hex_color;
        assert_eq!(parse_hex_color("#ff00aa"), Some([0xff, 0x00, 0xaa]));
        assert_eq!(parse_hex_color("#f0a"), Some([0xff, 0x00, 0xaa]));
        // `#a€bc` after the `#` is 6 bytes (a, €=3, b, c) — the old `&h[0..2]` sliced
        // mid-codepoint and panicked; now it's cleanly None.
        assert_eq!(parse_hex_color("#a\u{20ac}bc"), None);
        assert_eq!(parse_hex_color("#\u{e9}\u{e9}\u{e9}"), None); // "ééé" = 6 bytes
        assert_eq!(parse_hex_color("#gggggg"), None);
    }

    // A non-map (or nil) face is the default face: no colours, no flags, no family.
    #[test]
    fn non_map_face_is_default() {
        let heap = Heap::new();
        let f = gui_face(&heap, Value::Nil);
        assert!(f.fg.is_none());
        assert!(!f.bold && !f.italic && !f.underline && !f.reverse);
        assert!(f.family.is_none());
    }

    // A curated palette needs explicit colours, not just the 16 named slots: an
    // `[r g b]` vector and a `"#rrggbb"` hex string both resolve to that true colour.
    #[test]
    fn fg_accepts_rgb_vector_and_hex_string() {
        let mut heap = Heap::new();
        let triple = heap.alloc_vector(vec![Value::int(0x28), Value::int(0x2c), Value::int(0x34)]);
        let by_vec = heap.map_from_pairs(vec![(value::kw("fg"), triple)]);
        assert_eq!(gui_face(&heap, by_vec).fg, Some([0x28, 0x2c, 0x34]));

        let hex = heap.alloc_string("#61afef");
        let by_hex = heap.map_from_pairs(vec![(value::kw("bg"), hex)]);
        assert_eq!(gui_face(&heap, by_hex).bg, Some([0x61, 0xaf, 0xef]));
    }
}

#[cfg(test)]
mod underline_face_tests {
    use super::gui_face;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    fn face_with_underline(heap: &mut Heap, underline: Value) -> crate::host::gui::Face {
        let map = heap.map_from_pairs(vec![(value::kw("underline"), underline)]);
        gui_face(heap, map)
    }

    #[test]
    fn underline_takes_a_flag_a_colour_or_a_wave_spec() {
        let mut heap = Heap::new();
        let plain = face_with_underline(&mut heap, Value::boolean(true));
        assert!(plain.underline && plain.underline_rgb.is_none() && !plain.underline_wave);

        let red = heap.alloc_string("#ff0000");
        let coloured = face_with_underline(&mut heap, red);
        assert!(coloured.underline && !coloured.underline_wave);
        assert_eq!(coloured.underline_rgb, Some([0xff, 0, 0]));

        let green = heap.alloc_string("#00ff00");
        let spec = heap.map_from_pairs(vec![
            (value::kw("color"), green),
            (value::kw("style"), value::kw("wave")),
        ]);
        let wave = face_with_underline(&mut heap, spec);
        assert!(wave.underline && wave.underline_wave);
        assert_eq!(wave.underline_rgb, Some([0, 0xff, 0]));

        let off = face_with_underline(&mut heap, Value::boolean(false));
        assert!(!off.underline);
    }
}

#[cfg(test)]
mod cursor_style_tests {
    use super::cursor_style_from;
    use crate::core::value::{self, Value};
    use crate::host::gui::CursorStyle;

    #[test]
    fn maps_keywords_with_block_default() {
        assert_eq!(cursor_style_from(value::kw("bar")), CursorStyle::Bar);
        assert_eq!(
            cursor_style_from(value::kw("underline")),
            CursorStyle::Underline
        );
        assert_eq!(cursor_style_from(value::kw("block")), CursorStyle::Block);
        assert_eq!(cursor_style_from(value::kw("hollow")), CursorStyle::Hollow);
        // a bare `[:cursor row col]` (no style) and any unknown keyword → Block
        assert_eq!(cursor_style_from(Value::Nil), CursorStyle::Block);
        assert_eq!(cursor_style_from(value::kw("wat")), CursorStyle::Block);
    }
}

#[cfg(test)]
mod gui_music_op_tests {
    use super::{GuiOpTags, parse_gui_ops};
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};
    use crate::host::gui::Op;

    // `[:music snd]` / `[:music snd vol]` read as `[:sound …]` does: a non-negative int
    // handle, an optional number volume (absent or garbage: 0, the backend's default).
    #[test]
    fn a_music_op_reads_its_handle_and_volume_and_skips_a_bad_handle() {
        let heap = Heap::new();
        let music = value::intern("music");
        let parsed = vec![
            (
                music,
                vec![Value::Keyword(music), Value::int(3), Value::Float(0.4)],
            ),
            (music, vec![Value::Keyword(music), Value::int(5)]),
            (music, vec![Value::Keyword(music), Value::int(-1)]),
            (
                music,
                vec![Value::Keyword(music), Value::int(4_294_967_296)],
            ),
        ];
        let ops = parse_gui_ops(
            &heap,
            parsed,
            &GuiOpTags::new(),
            &mut super::fragments::FragmentPass::empty(),
        );
        assert!(
            ops == vec![Op::Music { id: 3, vol: 0.4 }, Op::Music { id: 5, vol: 0.0 }],
            "{} ops",
            ops.len()
        );
    }

    // `[:ambience snd vol]` reads as `[:music …]` does, into its own op.
    #[test]
    fn an_ambience_op_reads_its_handle_and_volume() {
        let heap = Heap::new();
        let ambience = value::intern("ambience");
        let parsed = vec![
            (
                ambience,
                vec![Value::Keyword(ambience), Value::int(2), Value::Float(0.25)],
            ),
            (ambience, vec![Value::Keyword(ambience), Value::int(-2)]),
        ];
        let ops = parse_gui_ops(
            &heap,
            parsed,
            &GuiOpTags::new(),
            &mut super::fragments::FragmentPass::empty(),
        );
        assert!(
            ops == vec![Op::Ambience { id: 2, vol: 0.25 }],
            "{} ops",
            ops.len()
        );
    }

    // `[:sound snd vol pan key]`: the pan and the key are optional — the centre and no key
    // when absent, and a key that is not an int is no key.
    #[test]
    fn a_sound_op_reads_its_pan_and_key() {
        let heap = Heap::new();
        let sound = value::intern("sound");
        let parsed = vec![
            (
                sound,
                vec![
                    Value::Keyword(sound),
                    Value::int(1),
                    Value::Float(0.5),
                    Value::Float(-0.75),
                    Value::int(42),
                ],
            ),
            (
                sound,
                vec![Value::Keyword(sound), Value::int(1), Value::Float(0.5)],
            ),
            (
                sound,
                vec![
                    Value::Keyword(sound),
                    Value::int(1),
                    Value::int(1),
                    Value::int(1),
                    Value::Float(4.2),
                ],
            ),
        ];
        let ops = parse_gui_ops(
            &heap,
            parsed,
            &GuiOpTags::new(),
            &mut super::fragments::FragmentPass::empty(),
        );
        let expected = vec![
            Op::Sound {
                id: 1,
                vol: 0.5,
                pan: -0.75,
                key: Some(42),
            },
            Op::Sound {
                id: 1,
                vol: 0.5,
                pan: 0.0,
                key: None,
            },
            Op::Sound {
                id: 1,
                vol: 1.0,
                pan: 1.0,
                key: None,
            },
        ];
        assert!(ops == expected, "{} ops", ops.len());
    }

    // An op off the window's top or left edge lands where the terminal puts it (ADR-046's
    // one protocol): a negative row paints nothing and a negative column clips the text at
    // the edge — never clamped to 0, which painted `[:text -1 0 …]` over row 0. A rect is
    // cut at the edge, and a malformed op is skipped, as the terminal now skips it too.
    #[test]
    fn an_op_off_the_top_or_left_edge_is_clipped_as_the_terminal_clips_it() {
        let mut heap = Heap::new();
        let text = value::intern("text");
        let rect = value::intern("rect");
        let cursor = value::intern("cursor");
        let text_op = |heap: &mut Heap, row: i64, col: i64, s: &str| {
            let s = heap.alloc_string(s);
            (
                text,
                vec![Value::Keyword(text), Value::int(row), Value::int(col), s],
            )
        };
        let parsed = vec![
            text_op(&mut heap, -1, 0, "above"),
            text_op(&mut heap, 2, -2, "abcd"),
            text_op(&mut heap, 3, -1, "\u{4e2d}x"),
            text_op(&mut heap, 4, -9, "gone"),
            (
                rect,
                vec![
                    Value::Keyword(rect),
                    Value::int(-1),
                    Value::int(-2),
                    Value::int(5),
                    Value::int(3),
                ],
            ),
            (
                cursor,
                vec![Value::Keyword(cursor), Value::int(-1), Value::int(4)],
            ),
            (
                text,
                vec![Value::Keyword(text), Value::Keyword(text), Value::int(0)],
            ),
        ];
        let ops = parse_gui_ops(
            &heap,
            parsed,
            &GuiOpTags::new(),
            &mut super::fragments::FragmentPass::empty(),
        );
        let face = crate::host::gui::Face::default();
        assert!(
            ops == vec![
                Op::Text {
                    row: 2,
                    col: 0,
                    s: "cd".to_string(),
                    face,
                },
                Op::Text {
                    row: 3,
                    col: 0,
                    s: " x".to_string(),
                    face,
                },
                Op::Rect {
                    row: 0,
                    col: 0,
                    w: 3,
                    h: 2,
                    face,
                    radius: 0.0,
                },
            ],
            "{} ops",
            ops.len()
        );
    }
}

/// `(%gui-lose-display why)` — act as if the display went away (ADR-631): every open
/// window's app is sent `[:hangup :display-lost]`, and draws become no-ops. What the GUI
/// thread does itself when its event loop ends; exposed so the path is testable headless.
pub(in crate::builtins) fn gui_lose_display(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let why = expect_string(heap, "%gui-lose-display", arg(args, 0))?;
    crate::host::gui::lose_display(&why);
    Ok(Value::nil())
}

/// The hinting style keyword `mode` names (`:auto`, `:none`, `:slight`, `:medium`, `:full` —
/// fontconfig's names; medium draws as full, as FreeType does), or a type error naming `who`.
fn text_hinting_of(
    heap: &Heap,
    who: &str,
    mode: Value,
) -> Result<crate::host::gui::TextHinting, LispError> {
    use crate::host::gui::TextHinting;
    let named = match mode {
        Value::Keyword(k) => match value::symbol_name_ref(k) {
            "auto" => Some(TextHinting::Auto),
            "none" => Some(TextHinting::None),
            "slight" => Some(TextHinting::Slight),
            "medium" | "full" => Some(TextHinting::Full),
            _ => None,
        },
        _ => None,
    };
    named.ok_or_else(|| {
        LispError::wrong_type(
            heap,
            who,
            "one of :auto, :none, :slight, :medium, :full",
            mode,
        )
    })
}

/// `(gui/text-hinting mode)` — set how outline glyphs are hinted (ADR-730) on every open
/// window + the default for ones opened later. GUI only; returns nil.
pub(in crate::builtins) fn gui_text_hinting(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let mode = text_hinting_of(heap, "gui/text-hinting", arg(args, 0))?;
    crate::host::gui::text_hinting(mode).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

#[cfg(test)]
mod window_spec_tests {
    use super::window_spec;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    /// `gui-open`'s args with `opts` as the opts map (nil title and size).
    fn spec_with(heap: &mut Heap, opts: Vec<(Value, Value)>) -> crate::host::gui::WindowSpec {
        let map = heap.map_from_pairs(opts);
        window_spec(&[Value::Nil, Value::Nil, Value::Nil, map], heap).expect("a valid spec")
    }

    // Present events are opt-in (ADR-775): a window opened with no opts, or opts that do not
    // name `:present-events`, or name it false or nil, sends no `[:presented t-ns]`; only
    // `{:present-events true}` turns them on.
    #[test]
    fn present_events_are_off_unless_asked() {
        let mut heap = Heap::new();
        assert!(!window_spec(&[], &heap).unwrap().present_events);
        let unrelated = spec_with(&mut heap, vec![(value::kw("input"), value::kw("pixels"))]);
        assert!(!unrelated.present_events);
        let off = spec_with(
            &mut heap,
            vec![(value::kw("present-events"), Value::boolean(false))],
        );
        assert!(!off.present_events);
        let nil = spec_with(&mut heap, vec![(value::kw("present-events"), Value::Nil)]);
        assert!(!nil.present_events);
        let on = spec_with(
            &mut heap,
            vec![(value::kw("present-events"), Value::boolean(true))],
        );
        assert!(on.present_events);
    }
}

#[cfg(test)]
mod gui_scene_light_tests {
    use super::{GuiOpTags, parse_gui_ops, tagged_ops};
    use crate::core::heap::Heap;
    use crate::host::gui::Op;
    use crate::host::gui::mesh::{LightKind, Scene};

    const CAMERA: &str = "[1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1]";

    /// `[:scene 0 0 8 8 {:camera … :lights <lights>}]` through `%gui-draw`'s parse: the scene,
    /// or the error `%gui-draw` raises.
    fn scene_with(lights: &str, material: &str) -> Result<Scene, String> {
        scene_keyed(lights, material, "")
    }

    /// The same, `more` keys added to the spec.
    fn scene_keyed(lights: &str, material: &str, more: &str) -> Result<Scene, String> {
        let mut heap = Heap::new();
        let source = format!(
            "[:scene 0 0 8 8 {{:camera {CAMERA} :lights {lights} {more} :draws [{{:mesh 1 :material {material}}}]}}]"
        );
        let op = crate::syntax::reader::read_one(&mut heap, &source).expect("reads");
        let tags = GuiOpTags::new();
        let ops = parse_gui_ops(
            &heap,
            tagged_ops(&heap, vec![op]),
            &tags,
            &mut super::fragments::FragmentPass::empty(),
        );
        tags.refused("%gui-draw")
            .map_err(|error| error.to_string())?;
        match ops.as_slice() {
            [Op::Scene(scene)] => Ok((**scene).clone()),
            _ => panic!("{} ops and no refusal", ops.len()),
        }
    }

    fn lights(lights: &str) -> Result<Scene, String> {
        scene_with(lights, "{}")
    }

    #[test]
    fn every_light_kind_reads_and_the_old_form_still_does() {
        let scene = lights(
            "[{:direction [0 -1 0] :color [1 1 1]}
              {:position [0 2 0] :color [3 3 3] :range 6}
              {:position [0 2 0] :direction [0 -1 0] :color [5 4 3] :range 8 :cone [0.3 0.6]
               :shadow {:resolution 512 :bias 2 :normal-bias 1 :near 0.1}}
              {:direction [1 -1 0] :shadow {:extent 3 :center [0 1 0]}}
              {:position [1 1 1] :shadow true}]",
        )
        .expect("lights");
        let kinds: Vec<LightKind> = scene.lights.iter().map(|light| light.kind).collect();
        assert_eq!(
            kinds,
            [
                LightKind::Directional,
                LightKind::Point,
                LightKind::Spot,
                LightKind::Directional,
                LightKind::Point
            ]
        );
        assert_eq!(scene.lights[1].range, 6.0);
        assert_eq!(scene.lights[2].cone, [0.3, 0.6]);
        let spot_shadow = scene.lights[2].shadow.as_ref().expect("a shadow");
        assert_eq!(
            (
                spot_shadow.resolution,
                spot_shadow.bias,
                spot_shadow.normal_bias,
                spot_shadow.near
            ),
            (512, 2.0, 1.0, 0.1)
        );
        let sun_shadow = scene.lights[3].shadow.as_ref().expect("a shadow");
        assert_eq!(
            (sun_shadow.extent, sun_shadow.center),
            (Some(3.0), Some([0.0, 1.0, 0.0]))
        );
        assert!(scene.lights[4].shadow.is_some());
        assert!(scene.lights[0].shadow.is_none());
        assert!(
            scene.draws[0].material.cast_shadow,
            "a draw casts by default"
        );
        let quiet = scene_with("[]", "{:cast-shadow false}").expect("a scene");
        assert!(!quiet.draws[0].material.cast_shadow);
    }

    #[test]
    fn a_malformed_light_is_refused_with_why() {
        let refused = |spec: &str, why: &str| {
            let error = lights(spec).expect_err(spec);
            assert!(
                error.contains("%gui-draw") && error.contains(why),
                "{spec}: {error:?} does not say {why:?}"
            );
        };
        refused("{:direction [0 -1 0]}", ":lights is a vector");
        refused("[[0 -1 0]]", "a light is a map");
        refused("[{:direction [0 -1 0] :intensity 2}]", "has no :intensity");
        refused(
            "[{:position [0 1 0] :direction [0 -1 0]}]",
            "give it a :cone",
        );
        refused("[{:direction [0 -1 0] :range 4}]", "give it a :position");
        refused("[{:color [1 1 1]}]", "needs a :direction");
        refused(
            "[{:position [0 1 0] :cone [0.2 0.4]}]",
            "needs a :direction",
        );
        refused("[{:direction [0 0 0]}]", "must not be [0 0 0]");
        refused("[{:direction [0 -1]}]", ":direction is 3 numbers");
        refused(
            "[{:direction [0 -1 0] :color [1 -1 1]}]",
            ":color is 0 or more",
        );
        refused(
            "[{:position [0 1 0] :range 0}]",
            ":range is a number above 0",
        );
        refused(
            "[{:position [0 1 0] :direction [0 -1 0] :cone [0.5 0.3]}]",
            "0 ≤ inner ≤ outer < π/2",
        );
        refused(
            "[{:position [0 1 0] :direction [0 -1 0] :cone [0.1 1.6]}]",
            "0 ≤ inner ≤ outer < π/2",
        );
        refused(
            "[{:direction [0 -1 0] :shadow :yes}]",
            ":shadow is true, false or a map",
        );
        refused(
            "[{:direction [0 -1 0] :shadow {:resolution 8}}]",
            ":resolution is a whole number",
        );
        refused(
            "[{:direction [0 -1 0] :shadow {:near 0.1}}]",
            "has no :near",
        );
        refused(
            "[{:position [0 1 0] :shadow {:extent 2}}]",
            "has no :extent",
        );
        refused(
            "[{:direction [0 -1 0] :shadow {:bias -1}}]",
            ":bias is 0 or more",
        );
        let nine = "{:direction [0 -1 0]} ".repeat(9);
        refused(&format!("[{nine}]"), "more than the 8");
        let five = "{:direction [0 -1 0] :shadow true} ".repeat(5);
        refused(&format!("[{five}]"), "more than the 4");
        let big = "{:position [0 1 0] :shadow {:resolution 4096}} ".repeat(2);
        refused(&format!("[{big}]"), "lower a :resolution");
        refused(
            &format!("[{big}]"),
            "need 201326592 texels together (12 maps of 4096²), more than 33554432",
        );
        // ADR-800: counted as the two arrays hold them, a 4096 spot with a 2048 point light's
        // cube is over the limit and with a 1024 one within it
        refused(
            "[{:position [0 2 0] :direction [0 -1 0] :cone [0.3 0.6] :shadow {:resolution 4096}}
              {:position [1 2 0] :shadow {:resolution 2048}}]",
            "need 41943040 texels together (1 map of 4096² and 6 maps of 2048²)",
        );
        let within = lights(
            "[{:position [0 2 0] :direction [0 -1 0] :cone [0.3 0.6] :shadow {:resolution 4096}}
              {:position [1 2 0] :shadow {:resolution 1024}}]",
        )
        .expect("a 4096 key and a 1024 point light's cube are within the limit");
        assert_eq!(within.lights.len(), 2);
        let error = scene_with("[]", "{:cast-shadow :no}").expect_err("cast-shadow");
        assert!(error.contains(":cast-shadow is true or false"), "{error}");
    }

    #[test]
    fn soft_shadows_sh_ambient_lobes_and_occlusion_read_and_default() {
        // ADR-793: every new key read, and what each is without it
        let scene = scene_keyed(
            "[{:position [0 2 0] :direction [0 -1 0] :cone [0.3 0.6] :size 0.4
               :shadow {:softness 24}}
              {:direction [1 -1 0] :angle 0.0093 :shadow true}
              {:position [0 1 0] :shadow true}]",
            "{:surface :skin :specular 0.35 :lobes [0.8 1.4 0.9] :occlusion 5}",
            ":ambient-sh [[1 1 1] [0 0.5 0] [0 0 0] [0 0 0] [0 0 0] [0 0 0] [0 0 0] [0 0 0] [0.1 0.2 0.3]]",
        )
        .expect("a scene");
        let spot = &scene.lights[0];
        assert_eq!(spot.size, 0.4);
        assert_eq!(spot.shadow.as_ref().unwrap().softness, 24.0);
        assert_eq!(scene.lights[1].size, 0.0093);
        assert_eq!(scene.lights[1].shadow.as_ref().unwrap().softness, 0.0);
        assert_eq!(scene.lights[2].size, 0.0, "a light is a point by default");
        let sh = scene.ambient_sh.expect("the SH ambient");
        assert_eq!(
            (sh[0], sh[1], sh[8]),
            ([1.0; 3], [0.0, 0.5, 0.0], [0.1, 0.2, 0.3])
        );
        let material = &scene.draws[0].material;
        assert_eq!(material.lobes, Some([0.8, 1.4, 0.9]));
        assert_eq!(material.occlusion, Some(5));
        assert!(
            (material.f0() - 0.028).abs() < 1e-6,
            "skin's F0: {}",
            material.f0()
        );
        let plain = lights("[]").expect("a scene");
        assert_eq!(plain.ambient_sh, None, "no :ambient-sh: the sky and ground");
        let default = &plain.draws[0].material;
        assert_eq!(
            (default.lobes(), default.occlusion),
            ([1.0, 1.0, 1.0], None)
        );
        assert!((default.f0() - 0.04).abs() < 1e-6, "the default F0 is 4 %");
        let skin = scene_with("[]", "{:surface :skin}").expect("a scene");
        assert_eq!(
            skin.draws[0].material.lobes(),
            crate::host::gui::mesh::SKIN_LOBES
        );
    }

    #[test]
    fn a_malformed_soft_shadow_sh_ambient_or_lobes_is_refused_with_why() {
        let refused = |result: Result<Scene, String>, why: &str| {
            let error = result.expect_err(why);
            assert!(
                error.contains("%gui-draw") && error.contains(why),
                "{error:?} does not say {why:?}"
            );
        };
        refused(
            lights("[{:direction [0 -1 0] :shadow {:softness -0.1}}]"),
            ":shadow :softness is 0 or more metres",
        );
        refused(
            lights("[{:position [0 1 0] :size -1}]"),
            ":size is 0 or more metres",
        );
        refused(
            lights("[{:direction [0 -1 0] :angle :wide}]"),
            ":angle is 0 or more radians",
        );
        refused(
            lights("[{:direction [0 -1 0] :size 0.4}]"),
            "its :angle, in radians",
        );
        refused(
            lights("[{:position [0 1 0] :angle 0.1}]"),
            "its :size, in metres",
        );
        refused(
            scene_keyed("[]", "{}", ":ambient-sh [[1 1 1]]"),
            ":ambient-sh is 9 [r g b]",
        );
        let short = "[1 1] ".repeat(9);
        refused(
            scene_keyed("[]", "{}", &format!(":ambient-sh [{short}]")),
            ":ambient-sh is 9 [r g b]",
        );
        refused(
            scene_with("[]", "{:lobes [0.75 1.3]}"),
            ":lobes is [rough-a rough-b mix]",
        );
        refused(
            scene_with("[]", "{:lobes [0 1.3 0.85]}"),
            ":lobes is [rough-a rough-b mix]",
        );
        refused(
            scene_with("[]", "{:lobes [0.75 1.3 1.5]}"),
            ":lobes is [rough-a rough-b mix]",
        );
    }

    #[test]
    fn a_skins_scatter_reads_and_defaults_and_its_old_meaning_is_refused_with_why() {
        // ADR-794: `:scatter` is how far light travels under skin; without it, human skin on
        // a mesh in metres (a curvature a metre is a thousandth of one a millimetre)
        use crate::host::gui::mesh::DEFAULT_SCATTER;
        let default = scene_with("[]", "{:surface :skin}").expect("a scene");
        let material = &default.draws[0].material;
        assert_eq!(material.scatter, None);
        assert_eq!(DEFAULT_SCATTER, 1.0);
        assert_eq!(material.scatter_scale(), 0.001);
        let given = scene_with("[]", "{:surface :skin :scatter 2.5}").expect("a scene");
        assert_eq!(given.draws[0].material.scatter, Some(2.5));
        assert_eq!(given.draws[0].material.scatter_scale(), 0.0025);
        let none = scene_with("[]", "{:surface :skin :scatter 0}").expect("a scene");
        assert_eq!(none.draws[0].material.scatter, Some(0.0));
        assert_eq!(none.draws[0].material.scatter_scale(), 0.0);
        let refused = |material: &str, why: &str| {
            let error = scene_with("[]", material).expect_err(material);
            assert!(
                error.contains("%gui-draw") && error.contains(why),
                "{material}: {error:?} does not say {why:?}"
            );
        };
        let scatter = ":scatter is how far light travels under a :skin surface";
        // the wrapped diffuse's colour and reach, as a game wrote them before
        refused("{:surface :skin :scatter [0.8 0.62 0.55]}", scatter);
        refused("{:surface :skin :scatter -1}", scatter);
        refused("{:surface :skin :scatter :deep}", scatter);
        refused("{:surface :skin :scatter 1e9}", scatter);
    }

    #[test]
    fn a_detail_tile_reads_and_defaults_and_a_malformed_one_is_refused_with_why() {
        // ADR-797: without the keys a material has no tile; with only `:detail`, the tile once
        // across the UV square, as baked
        use crate::host::gui::mesh::{DEFAULT_DETAIL_CAVITY, DEFAULT_DETAIL_STRENGTH, Material};
        let none = scene_with("[]", "{:surface :skin}").expect("a scene");
        let material = &none.draws[0].material;
        assert_eq!(material.detail, None);
        assert_eq!(material.detail_scale, [1.0, 1.0]);
        assert_eq!(
            (material.detail_strength, material.detail_cavity),
            (DEFAULT_DETAIL_STRENGTH, DEFAULT_DETAIL_CAVITY)
        );
        assert_eq!((DEFAULT_DETAIL_STRENGTH, DEFAULT_DETAIL_CAVITY), (1.0, 1.0));
        let bare = scene_with("[]", "{:detail 7}").expect("a scene");
        let material = &bare.draws[0].material;
        assert_eq!(material.detail, Some(7));
        assert_eq!(material.detail_texture(), Some(7));
        assert_eq!(material.detail_scale, [1.0, 1.0]);
        // every key, the scale as one number and as [u v]
        let given = scene_with(
            "[]",
            "{:surface :skin :detail 7 :detail-scale 60 :detail-strength 0.5 :detail-cavity 0.25}",
        )
        .expect("a scene");
        let material = &given.draws[0].material;
        assert_eq!(material.detail_scale, [60.0, 60.0]);
        assert_eq!(
            (material.detail_strength, material.detail_cavity),
            (0.5, 0.25)
        );
        let pair = scene_with("[]", "{:detail 7 :detail-scale [40 80.5]}").expect("a scene");
        assert_eq!(pair.draws[0].material.detail_scale, [40.0, 80.5]);
        let ends = scene_with(
            "[]",
            "{:detail 0 :detail-scale 4096 :detail-strength 16 :detail-cavity 0}",
        )
        .expect("a scene");
        let material = &ends.draws[0].material;
        assert_eq!(material.detail, Some(0));
        assert_eq!(material.detail_scale, [4096.0, 4096.0]);
        assert_eq!(
            (material.detail_strength, material.detail_cavity),
            (16.0, 0.0)
        );
        // nil is the key left out; a hair surface keeps the handle and reads no tile
        let nil = scene_with(
            "[]",
            "{:detail nil :detail-scale nil :detail-strength nil :detail-cavity nil}",
        )
        .expect("a scene");
        assert_eq!(nil.draws[0].material, Material::default());
        let hair = scene_with("[]", "{:surface :hair :detail 7}").expect("a scene");
        assert_eq!(hair.draws[0].material.detail, Some(7));
        assert_eq!(hair.draws[0].material.detail_texture(), None);
        let refused = |material: &str, why: &str| {
            let error = scene_with("[]", material).expect_err(material);
            assert!(
                error.contains("%gui-draw") && error.contains(why),
                "{material}: {error:?} does not say {why:?}"
            );
        };
        let handle = ":detail is a texture handle";
        refused("{:detail :pores}", handle);
        refused("{:detail -1}", handle);
        refused("{:detail 1.5}", handle);
        refused("{:detail [7]}", handle);
        let scale = ":detail-scale is how many times its :detail tile repeats";
        refused("{:detail 7 :detail-scale 0}", scale);
        refused("{:detail 7 :detail-scale -60}", scale);
        refused("{:detail 7 :detail-scale 5000}", scale);
        refused("{:detail 7 :detail-scale [60]}", scale);
        refused("{:detail 7 :detail-scale [60 0]}", scale);
        refused("{:detail 7 :detail-scale [60 60 60]}", scale);
        refused("{:detail 7 :detail-scale :fine}", scale);
        let strength = ":detail-strength is what its :detail normal's slope is multiplied by";
        refused("{:detail 7 :detail-strength -0.5}", strength);
        refused("{:detail 7 :detail-strength 17}", strength);
        refused("{:detail 7 :detail-strength [1 1]}", strength);
        let cavity = ":detail-cavity is how much its :detail tile's cavity damps the specular";
        refused("{:detail 7 :detail-cavity 1.5}", cavity);
        refused("{:detail 7 :detail-cavity -0.1}", cavity);
        refused("{:detail 7 :detail-cavity true}", cavity);
    }

    #[test]
    fn a_lights_shape_the_emitters_and_a_coat_read_and_default() {
        // ADR-795: without the keys a light is a point, a scene has no emitters, a material no
        // coat; `:rim` is a key a material no longer has, ignored as any unknown one
        use crate::host::gui::mesh::{EmitterShape, LightShape, Material};
        let plain = scene_with(
            "[{:position [0 2 0] :direction [0 -1 0] :cone [0.3 0.6] :size 0.4}]",
            "{:rim 0.5}",
        )
        .expect("a scene");
        assert_eq!(plain.lights[0].shape, None);
        assert!(plain.emitters.is_empty());
        assert_eq!(plain.draws[0].material, Material::default());
        let scene = scene_keyed(
            "[{:position [0 2 1] :direction [0 -1 -1] :cone [0.5 0.8] :size 0.3
               :shape :rect :extent [1.2 0.8] :up [0 0 1]}
              {:position [0 2 1] :direction [0 -1 -1] :cone [0.5 0.8] :shape :rect :extent [1 2]}
              {:position [0 2 0] :size 0.1 :shape :sphere}
              {:direction [0 -1 0] :angle 0.02 :shape :disc}]",
            "{:coat [0.5 0.15]}",
            ":emitters [{:shape :rect :position [1.7 1.6 -4.5] :direction [0 0 1] :extent [1.5 1.3]
                         :radiance [6 7 8]}
                        {:shape :rect :position [0 1 0] :direction [1 0 0] :up [0 0 1]
                         :extent [1 2] :radiance [0 0 0]}
                        {:shape :disc :position [0 1 0] :direction [0 -1 0] :size 0.5
                         :radiance [1 1 1]}
                        {:shape :sphere :position [0 1 0] :size 0.25 :radiance [2 2 2]}]",
        )
        .expect("a scene");
        assert_eq!(
            scene.lights[0].shape,
            Some(LightShape {
                shape: EmitterShape::Rect,
                extent: [1.2, 0.8],
                up: [0.0, 0.0, 1.0]
            })
        );
        assert_eq!(scene.lights[0].size, 0.3, "its shadow's softness still");
        assert_eq!(
            scene.lights[1].shape,
            Some(LightShape {
                shape: EmitterShape::Rect,
                extent: [1.0, 2.0],
                up: [0.0, 1.0, 0.0]
            })
        );
        let shapes: Vec<_> = scene.lights[2..]
            .iter()
            .map(|light| light.shape.map(|shape| shape.shape))
            .collect();
        assert_eq!(
            shapes,
            [Some(EmitterShape::Sphere), Some(EmitterShape::Disc)]
        );
        let disc = lights(
            "[{:position [0 2 1] :direction [0 -1 0] :cone [0.5 0.8] :size 0.3 :shape :disc}]",
        )
        .expect("a disc");
        assert_eq!(
            disc.lights[0].shape.map(|shape| shape.shape),
            Some(EmitterShape::Disc)
        );
        assert_eq!(scene.draws[0].material.coat, Some([0.5, 0.15]));
        let window = &scene.emitters[0];
        assert_eq!(
            (window.shape, window.position, window.direction, window.up),
            (
                EmitterShape::Rect,
                [1.7, 1.6, -4.5],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0]
            )
        );
        assert_eq!(
            (window.extent, window.radiance),
            ([1.5, 1.3], [6.0, 7.0, 8.0])
        );
        assert_eq!(scene.emitters[1].up, [0.0, 0.0, 1.0]);
        assert_eq!(
            (scene.emitters[2].shape, scene.emitters[2].extent),
            (EmitterShape::Disc, [0.5, 0.5])
        );
        assert_eq!(
            (scene.emitters[3].shape, scene.emitters[3].extent),
            (EmitterShape::Sphere, [0.25, 0.25])
        );
        // nil is the key left out; the ends of a coat's range
        let nil = scene_keyed("[]", "{:coat nil}", ":emitters nil").expect("a scene");
        assert_eq!(nil.draws[0].material.coat, None);
        assert!(nil.emitters.is_empty());
        let ends = scene_with("[]", "{:coat [1 0]}").expect("a scene");
        assert_eq!(ends.draws[0].material.coat, Some([1.0, 0.0]));
        // eight shapes together: five lights' and three emitters'
        let five = "{:position [0 2 0] :size 0.1 :shape :sphere} ".repeat(5);
        let emitter = "{:shape :sphere :position [0 1 0] :size 0.25 :radiance [2 2 2]} ";
        let full = scene_keyed(
            &format!("[{five}]"),
            "{}",
            &format!(":emitters [{}]", emitter.repeat(3)),
        )
        .expect("eight shapes");
        assert_eq!(full.emitters.len(), 3);
    }

    #[test]
    fn a_malformed_shape_emitter_or_coat_is_refused_with_why() {
        let refused = |result: Result<Scene, String>, why: &str| {
            let error = result.expect_err(why);
            assert!(
                error.contains("%gui-draw") && error.contains(why),
                "{error:?} does not say {why:?}"
            );
        };
        let spot = "{:position [0 2 0] :direction [0 -1 0] :cone [0.3 0.6]";
        refused(
            lights(&format!("[{spot} :shape :square}}]")),
            ":shape is :disc, :rect or :sphere",
        );
        refused(
            lights(&format!("[{spot} :shape \"rect\"}}]")),
            ":shape is :disc, :rect or :sphere",
        );
        // a shape without the size it is drawn at
        refused(
            lights(&format!("[{spot} :shape :disc}}]")),
            ":shape :disc is as big as the light's :size",
        );
        refused(
            lights(&format!("[{spot} :shape :sphere :size 0}}]")),
            ":shape :sphere is as big as the light's :size",
        );
        refused(
            lights(&format!("[{spot} :shape :rect :size 0.5}}]")),
            ":shape :rect needs an :extent",
        );
        refused(
            lights("[{:direction [0 -1 0] :shape :disc}]"),
            "as big as its :angle",
        );
        // an extent that is not two lengths, an up that points nowhere
        for extent in ["[1]", "[1 0]", "[1 -2]", "[1 2 3]", "1.5", ":wide"] {
            refused(
                lights(&format!("[{spot} :shape :rect :extent {extent}}}]")),
                ":extent is",
            );
        }
        refused(
            lights(&format!(
                "[{spot} :shape :rect :extent [1 2] :up [0 0 0]}}]"
            )),
            ":up must not be [0 0 0]",
        );
        refused(
            lights(&format!("[{spot} :shape :rect :extent [1 2] :up [0 1]}}]")),
            ":up is 3 numbers",
        );
        // a rectangle's keys on what is not one, and shapes a kind of light cannot have
        refused(
            lights(&format!("[{spot} :size 0.5 :shape :disc :extent [1 2]}}]")),
            ":extent and :up are a :shape :rect's",
        );
        refused(
            lights(&format!("[{spot} :up [0 1 0]}}]")),
            ":extent and :up are a :shape :rect's",
        );
        refused(
            lights("[{:position [0 2 0] :shape :rect :extent [1 2]}]"),
            "has no :extent (a :shape :rect's, which only a spot light takes)",
        );
        refused(
            lights("[{:position [0 2 0] :size 0.5 :shape :disc}]"),
            "a point light's :shape is :sphere",
        );
        refused(
            lights("[{:direction [0 -1 0] :angle 0.1 :shape :rect}]"),
            "a directional light's :shape is a :disc",
        );
        refused(
            lights("[{:direction [0 -1 0] :angle 0.1 :shape :disc :up [0 1 0]}]"),
            "has no :up",
        );
        // the emitters
        let emitters = |spec: &str| scene_keyed("[]", "{}", &format!(":emitters {spec}"));
        refused(emitters("{:shape :rect}"), ":emitters is a vector");
        refused(emitters("[:window]"), "emitter 0: an emitter is a map");
        refused(
            emitters("[{:position [0 1 0] :radiance [1 1 1]}]"),
            "an emitter needs a :shape",
        );
        refused(
            emitters("[{:shape :cube :position [0 1 0] :radiance [1 1 1]}]"),
            ":shape is :disc, :rect or :sphere",
        );
        let rect =
            "{:shape :rect :position [0 1 0] :direction [0 0 1] :extent [1 2] :radiance [1 1 1]";
        refused(
            emitters(&format!("[{rect}}} {rect} :color [1 1 1]}}]")),
            "emitter 1: a :rect emitter has no :color",
        );
        refused(
            emitters(&format!("[{rect} :size 2}}]")),
            "a :rect emitter has no :size",
        );
        refused(
            emitters("[{:shape :rect :direction [0 0 1] :extent [1 2] :radiance [1 1 1]}]"),
            "a :rect emitter needs a :position",
        );
        refused(
            emitters("[{:shape :rect :position [0 1 0] :extent [1 2] :radiance [1 1 1]}]"),
            "a :rect emitter needs a :direction",
        );
        refused(
            emitters("[{:shape :rect :position [0 1 0] :direction [0 0 1] :radiance [1 1 1]}]"),
            "a :rect emitter needs a :extent",
        );
        refused(
            emitters("[{:shape :rect :position [0 1 0] :direction [0 0 1] :extent [1 2]}]"),
            "a :rect emitter needs a :radiance",
        );
        refused(
            emitters(
                "[{:shape :rect :position [0 1 0] :direction [0 0 0] :extent [1 2] :radiance [1 1 1]}]",
            ),
            ":direction must not be [0 0 0]",
        );
        refused(
            emitters(
                "[{:shape :rect :position [0 1 0] :direction [0 0 1] :extent [1 0] :radiance [1 1 1]}]",
            ),
            ":extent is [width height], metres above 0",
        );
        refused(
            emitters(
                "[{:shape :rect :position [0 1] :direction [0 0 1] :extent [1 2] :radiance [1 1 1]}]",
            ),
            ":position is 3 numbers",
        );
        refused(
            emitters(&format!("[{rect} :up [0 0 0]}}]")),
            ":up must not be [0 0 0]",
        );
        refused(
            emitters(
                "[{:shape :rect :position [0 1 0] :direction [0 0 1] :extent [1 2] :radiance [1 -1 1]}]",
            ),
            ":radiance is 0 or more",
        );
        refused(
            emitters("[{:shape :disc :position [0 1 0] :direction [0 0 1] :radiance [1 1 1]}]"),
            "a :disc emitter needs a :size",
        );
        refused(
            emitters(
                "[{:shape :disc :position [0 1 0] :direction [0 0 1] :size 0 :radiance [1 1 1]}]",
            ),
            ":size is its diameter, metres above 0",
        );
        refused(
            emitters("[{:shape :sphere :position [0 1 0] :size [1 1] :radiance [1 1 1]}]"),
            ":size is its diameter, metres above 0",
        );
        refused(
            emitters(
                "[{:shape :sphere :position [0 1 0] :direction [0 0 1] :size 1 :radiance [1 1 1]}]",
            ),
            "a :sphere emitter has no :direction",
        );
        // more shapes than a scene's reflections show: nine emitters, or eight and a shaped light
        let sphere = "{:shape :sphere :position [0 1 0] :size 0.25 :radiance [2 2 2]} ";
        refused(
            emitters(&format!("[{}]", sphere.repeat(9))),
            "9 :emitters is more than the 8 shapes",
        );
        refused(
            scene_keyed(
                "[{:position [0 2 0] :size 0.1 :shape :sphere}]",
                "{}",
                &format!(":emitters [{}]", sphere.repeat(8)),
            ),
            "1 lights with a :shape and 8 :emitters are more than the 8 shapes",
        );
        // a coat
        let coat = ":coat is [strength roughness]";
        for given in [
            "0.5",
            "[0.5]",
            "[0.5 0.1 0.2]",
            "[1.5 0.1]",
            "[0.5 -0.1]",
            "[0.5 2]",
            ":wet",
            "true",
        ] {
            refused(scene_with("[]", &format!("{{:coat {given}}}")), coat);
        }
    }

    #[test]
    fn an_eyes_optics_read_and_default() {
        // ADR-796: where its eyes are in the mesh at rest, and their shape
        use crate::host::gui::mesh::{
            DEFAULT_CORNEA_RADIUS, DEFAULT_EYE_IOR, DEFAULT_EYE_RADIUS, DEFAULT_IRIS_CONCAVITY,
            DEFAULT_IRIS_DEPTH, DEFAULT_IRIS_RADIUS, DEFAULT_PUPIL, EyeAt, Surface,
        };
        let one = "{:centre [0.03 1.5 0.1] :axis [0 0 2] :iris-uv [0.3 0.7]}";
        let scene = scene_with(
            "[]",
            &format!("{{:surface :eye :eye {{:eyes [{one}] :iris-uv-radius 0.1}}}}"),
        )
        .expect("an eye");
        let material = &scene.draws[0].material;
        assert_eq!(material.surface, Surface::Eye);
        let eye = material.eye_optics().expect("its optics");
        // the axis made unit length; everything it did not say a grown person's
        assert_eq!(
            eye.eyes,
            [EyeAt {
                centre: [0.03, 1.5, 0.1],
                axis: [0.0, 0.0, 1.0],
                iris_uv: [0.3, 0.7],
            }]
        );
        assert_eq!(eye.up, [0.0, 1.0, 0.0]);
        assert_eq!(
            (
                eye.radius,
                eye.iris_radius,
                eye.cornea_radius,
                eye.iris_depth
            ),
            (
                DEFAULT_EYE_RADIUS,
                DEFAULT_IRIS_RADIUS,
                DEFAULT_CORNEA_RADIUS,
                DEFAULT_IRIS_DEPTH
            )
        );
        assert_eq!(
            (eye.ior, eye.pupil, eye.iris_uv_radius),
            (DEFAULT_EYE_IOR, DEFAULT_PUPIL, 0.1)
        );
        assert_eq!(eye.concavity, DEFAULT_IRIS_CONCAVITY);
        assert_eq!(
            (DEFAULT_EYE_IOR, DEFAULT_PUPIL, DEFAULT_IRIS_CONCAVITY),
            (1.376, 0.35, 0.35),
            "what std/gui.blsp says"
        );
        // a pair, every key given
        let other = "{:centre [-0.03 1.5 0.1] :axis [0 0 1] :iris-uv [0.7 0.3]}";
        let scene = scene_with(
            "[]",
            &format!(
                "{{:surface :eye :coat [1 0.04] :eye {{:eyes [{one} {other}] :iris-uv-radius 0.1 :up [0 3 0] :radius 0.016 :iris-radius 0.006 :cornea-radius 0.008 :iris-depth 0.003 :ior 1.3 :pupil 0 :concavity 0.6}}}}"
            ),
        )
        .expect("two eyes");
        let material = &scene.draws[0].material;
        let eye = material.eye_optics().expect("its optics");
        assert_eq!(eye.eyes.len(), 2);
        assert_eq!(eye.eyes[1].iris_uv, [0.7, 0.3]);
        assert_eq!(eye.up, [0.0, 1.0, 0.0]);
        assert_eq!(
            (
                eye.radius,
                eye.iris_radius,
                eye.cornea_radius,
                eye.iris_depth
            ),
            (0.016, 0.006, 0.008, 0.003)
        );
        assert_eq!((eye.ior, eye.pupil, eye.concavity), (1.3, 0.0, 0.6));
        assert_eq!(material.coat, Some([1.0, 0.04]), "an eye keeps its coat");
        // on another surface the optics are read and checked, and used by none
        let scene = scene_with(
            "[]",
            &format!("{{:surface :cutout :eye {{:eyes [{one}] :iris-uv-radius 0.1}}}}"),
        )
        .expect("a cutout");
        assert!(scene.draws[0].material.eye.is_some());
        assert!(scene.draws[0].material.eye_optics().is_none());
        // nil is none
        let scene = scene_with("[]", "{:eye nil}").expect("a scene");
        assert_eq!(scene.draws[0].material.eye, None);
    }

    #[test]
    fn a_malformed_eye_is_refused_with_why() {
        let refused = |material: String, why: &str| {
            let error = scene_with("[]", &material).expect_err(why);
            assert!(
                error.contains("%gui-draw") && error.contains(why),
                "{error:?} does not say {why:?}"
            );
        };
        let one = "{:centre [0 0 0] :axis [0 0 1] :iris-uv [0.5 0.5]}";
        let eye = |keys: &str| {
            format!("{{:surface :eye :eye {{:eyes [{one}] :iris-uv-radius 0.1 {keys}}}}}")
        };
        // an eye surface with nothing to draw an eye by
        refused(
            "{:surface :eye}".to_string(),
            "an :eye surface needs its :eye",
        );
        refused(
            "{:surface :eye :eye nil}".to_string(),
            "an :eye surface needs its :eye",
        );
        for given in ["true", "[0 0 0]", ":left"] {
            refused(format!("{{:eye {given}}}"), "a material's :eye is a map");
        }
        // its eyes: one or two, each whole
        for given in [
            "",
            ":eyes []",
            ":eyes 2",
            &format!(":eyes [{one} {one} {one}]"),
        ] {
            refused(
                format!("{{:eye {{{given} :iris-uv-radius 0.1}}}}"),
                ":eyes is a vector of one or two eyes",
            );
        }
        let with_eye = |entry: &str| format!("{{:eye {{:eyes [{entry}] :iris-uv-radius 0.1}}}}");
        refused(with_eye("7"), "eye 0 is a map");
        refused(
            with_eye("{:axis [0 0 1] :iris-uv [0.5 0.5]}"),
            "eye 0's :centre is missing",
        );
        refused(
            with_eye("{:centre [0 0] :axis [0 0 1] :iris-uv [0.5 0.5]}"),
            "eye 0's :centre is the eyeball's centre",
        );
        refused(
            with_eye("{:centre [0 0 0] :iris-uv [0.5 0.5]}"),
            "eye 0's :axis is missing",
        );
        refused(
            with_eye("{:centre [0 0 0] :axis [0 0 0] :iris-uv [0.5 0.5]}"),
            "it has no length",
        );
        refused(
            with_eye("{:centre [0 0 0] :axis :forward :iris-uv [0.5 0.5]}"),
            "eye 0's :axis is a direction [x y z]",
        );
        refused(
            with_eye("{:centre [0 0 0] :axis [0 0 1]}"),
            "eye 0's :iris-uv is missing",
        );
        refused(
            with_eye("{:centre [0 0 0] :axis [0 0 1] :iris-uv [0.5 0.5 0.1]}"),
            "eye 0's :iris-uv is where the middle of its iris is",
        );
        // the iris in the texture
        for given in [
            "",
            ":iris-uv-radius 0",
            ":iris-uv-radius 2",
            ":iris-uv-radius [0.1]",
        ] {
            refused(
                format!("{{:eye {{:eyes [{one}] {given}}}}}"),
                ":iris-uv-radius is how far the iris's edge is",
            );
        }
        // its shape
        refused(eye(":up [0 0 0]"), "an :eye's :up is a direction");
        refused(eye(":up [0 0 -5]"), ":up runs along eye 0's :axis");
        for key in ["radius", "iris-radius", "cornea-radius", "iris-depth"] {
            for given in ["0", "-1", ":big", "[1]"] {
                refused(
                    eye(&format!(":{key} {given}")),
                    &format!("an :eye's :{key} is"),
                );
            }
        }
        refused(
            eye(":iris-radius 0.02"),
            ":iris-radius (0.02) is under its :radius",
        );
        refused(
            eye(":cornea-radius 0.005"),
            "and its :cornea-radius (0.005)",
        );
        for given in ["0.9", "4", ":glass"] {
            refused(
                eye(&format!(":ior {given}")),
                ":ior is the refractive index",
            );
        }
        for given in ["-0.1", "4.5", ":deep"] {
            refused(
                eye(&format!(":concavity {given}")),
                ":concavity is how steeply the iris",
            );
        }
        for given in ["-0.1", "1.5", "[0.3]"] {
            refused(
                eye(&format!(":pupil {given}")),
                ":pupil is the pupil's radius",
            );
        }
    }
}
