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
    let spec = crate::host::gui::WindowSpec {
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
    };
    let id =
        crate::host::gui::open(crate::process::self_pid(), spec).map_err(LispError::runtime)?;
    Ok(Value::int(id as i64))
}

/// `(%gui-size-px id)` — window `id`'s inner size as `[w h]` in PHYSICAL pixels.
pub(in crate::builtins) fn gui_size_px(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-size-px", arg(args, 0))?;
    let (w, h) = crate::host::gui::size_px(id).map_err(LispError::runtime)?;
    Ok(heap.alloc_vector2(Value::int(w as i64), Value::int(h as i64)))
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

/// `(%gui-texture id tex rgba w h)` — upload `rgba` (`w*h*4` bytes, row-major straight
/// alpha, a bytes value or a vector of ints) as texture `tex` of window `id`. The handle
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
    crate::host::gui::texture(id, tex, rgba, w, h).map_err(LispError::runtime)?;
    Ok(Value::nil())
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
    frect_t: value::Symbol,
    quad_t: value::Symbol,
    sprite_t: value::Symbol,
    text_px_t: value::Symbol,
    sound_t: value::Symbol,
    music_t: value::Symbol,
    ambient_t: value::Symbol,
    light_t: value::Symbol,
    overlay_t: value::Symbol,
    bloom_t: value::Symbol,
    crt_t: value::Symbol,
    emitter_t: value::Symbol,
    burst_t: value::Symbol,
    seam_t: value::Symbol,
    above_seam_t: value::Symbol,
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
            frect_t: value::intern("frect"),
            quad_t: value::intern("quad"),
            sprite_t: value::intern("sprite"),
            text_px_t: value::intern("text-px"),
            sound_t: value::intern("sound"),
            music_t: value::intern("music"),
            ambient_t: value::intern("ambient"),
            light_t: value::intern("light"),
            overlay_t: value::intern("overlay"),
            bloom_t: value::intern("bloom"),
            crt_t: value::intern("crt"),
            emitter_t: value::intern("emitter"),
            burst_t: value::intern("burst"),
            seam_t: value::intern("seam"),
            above_seam_t: value::intern("above-seam"),
        }
    }
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
            // `[:sound id]` / `[:sound id vol]` — a handle that is not a non-negative int
            // is not a sound; an absent or non-number volume is the default (0).
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
            ops.push(crate::host::gui::Op::Sound { id, vol });
        } else if tag == tags.music_t {
            // `[:music id]` / `[:music id vol]` — read exactly as `[:sound …]` is.
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
            ops.push(crate::host::gui::Op::Music { id, vol });
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

pub(in crate::builtins) fn gui_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let win = gui_window_id(heap, "%gui-draw", arg(args, 0))?;
    let parsed = frame_ops(heap, arg(args, 1), "%gui-draw", "vector (a frame)")?;
    let tags = GuiOpTags::new();
    let mut pass = fragments::FragmentPass::for_window(win);
    let ops = parse_gui_ops(heap, parsed, &tags, &mut pass);
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
