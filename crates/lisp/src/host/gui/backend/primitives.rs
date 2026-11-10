//! The Brood-facing entry points behind the `%gui-*` primitives: opening and closing
//! windows, the window ops dispatched to the GUI thread, the global renderer settings, the
//! queries answered from shared state, and in-memory frame capture (ADR-609). Each routes
//! through the event-loop proxy, or the offscreen painter, or does nothing headless.

use super::*;

/// `(gui/render-frame ops spec)` — paint `ops` into memory as a window `spec` describes
/// would paint them (ADR-609): the window's own CPU painter, edge bleed and layout rule,
/// on the offscreen painter's thread. Needs no window and no display.
pub fn render_frame(
    ops: Vec<Op>,
    spec: super::super::RenderSpec,
) -> Result<super::super::FrameImage, String> {
    offscreen::ask(|reply| offscreen::Request::Render { ops, spec, reply })
}

/// `(gui/capture id)` — window `id`'s current frame, painted into memory as the window
/// paints it (ADR-609). A real window's is rasterised on the GUI thread by its own
/// renderer (a GPU window's by the CPU painter over the same frame, its sprites
/// missing); a headless painting window's on the offscreen thread. A plain headless
/// window has nothing to paint with, so that is an error naming the setting.
pub fn capture(id: u64) -> Result<super::super::FrameImage, String> {
    if !lock(windows()).contains_key(&id) {
        return Err("gui window not open".into());
    }
    if headless_paint() {
        return offscreen::ask(|reply| offscreen::Request::Capture { id, reply })?;
    }
    if headless() {
        return Err(
            "gui is headless with no painter — run with BROOD_GUI_HEADLESS=paint \
             to paint headless windows, or use gui/render-frame"
                .into(),
        );
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    lock(gui()?)
        .send_event(UserEvent::Capture {
            id,
            reply: reply_tx,
        })
        .map_err(|_| "gui thread is gone".to_string())?;
    reply_rx
        .recv()
        .map_err(|_| "gui thread did not reply".to_string())?
}

/// `(gui/windows)` — the ids of the windows open in this runtime, oldest first: how a
/// driver finds the window an app's loop opened, to `gui/capture` it.
pub fn window_ids() -> Vec<u64> {
    let mut ids: Vec<u64> = lock(windows()).keys().copied().collect();
    ids.sort_unstable();
    ids
}

/// `(gui-open subscriber)` — open a new window whose key/mouse input is
/// delivered to process `subscriber`'s mailbox; return the window id. Starts the
/// GUI thread on the first call. Each call is an independent window.
pub fn open(subscriber: u64, spec: WindowSpec) -> Result<u64, String> {
    if display_lost() {
        return Err("gui: the display is gone; no window can open".into());
    }
    // Headless: register a fake window (fixed cell grid, no input) without ever
    // starting winit, so nothing pops up.
    if headless() {
        let id = next_id();
        let size = Arc::new(Mutex::new(headless_cells(spec.size)));
        let size_px = headless_px(spec.size);
        // Headless painting (ADR-609): the window is painted offscreen, so its grid is the
        // one the real cell metrics give, measured before `open` returns.
        if headless_paint() {
            offscreen::ask(|reply| offscreen::Request::Open {
                id,
                subscriber,
                px: size_px,
                size: size.clone(),
                reply,
            })?;
        }
        lock(windows()).insert(
            id,
            WinHandle {
                subscriber,
                size,
                size_px: Arc::new(Mutex::new(size_px)),
                keys: Arc::default(),
                key_repeat: Arc::default(),
                latency: Arc::default(),
            },
        );
        deliver_undelivered_degradations(subscriber);
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
        key_repeat,
        latency,
    } = reply_rx
        .recv()
        .map_err(|_| "gui thread did not reply".to_string())??;
    lock(windows()).insert(
        id,
        WinHandle {
            subscriber,
            size,
            size_px,
            keys,
            key_repeat,
            latency,
        },
    );
    deliver_undelivered_degradations(subscriber);
    Ok(id)
}

/// `(gui-close id)` — destroy window `id` (idempotent; unknown id is a no-op).
pub fn close(id: u64) -> Result<(), String> {
    lock(windows()).remove(&id);
    if headless() {
        return headless_send(|| offscreen::Request::Close { id });
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
    if headless() || display_lost() {
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
    if headless_paint() {
        return offscreen::ask(|reply| offscreen::Request::Metrics { id, px, reply })?;
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
    let samples = lock(&h.latency).samples.iter().copied().collect();
    Ok(samples)
}

/// `(gui-latency-histogram id)` — window `id`'s input-to-present distribution since it
/// opened, keys and pointer apart (ADR-612). Both empty headless.
pub fn latency_histograms(id: u64) -> Result<(LatencyHistogram, LatencyHistogram), String> {
    let w = lock(windows());
    let h = w.get(&id).ok_or("gui window not open")?;
    let log = lock(&h.latency);
    Ok((log.keys.clone(), log.pointer.clone()))
}

/// `(gui-key-repeat? id)` — whether the last key press window `id`'s app took from its
/// mailbox was an OS auto-repeat (ADR-612): exact for the press the app is folding, since
/// the mark is set as the press leaves the mailbox. False for a window not open (one
/// closed under a turn that still names it) and headless.
pub fn key_repeat(id: u64) -> bool {
    lock(windows())
        .get(&id)
        .is_some_and(|h| h.key_repeat.load(Ordering::Acquire))
}

/// `(gui-draw id ops)` — paint a frame to window `id`.
pub fn draw(id: u64, ops: Vec<Op>) -> Result<(), String> {
    {
        let w = lock(windows());
        if !w.contains_key(&id) {
            return Err("gui window not open".into());
        }
    }
    // nothing to paint on once the display is gone: the app is ending (`[:hangup …]`)
    if display_lost() {
        return Ok(());
    }
    if headless() {
        return headless_send(|| offscreen::Request::Draw { id, ops });
    }
    gui()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .send_event(UserEvent::Draw { id, ops })
        .map_err(|_| "gui thread is gone".to_string())
}

/// `(gui-settle id)` — window `id`'s app finished a turn with nothing to draw (ADR-444).
pub fn settle(id: u64) -> Result<(), String> {
    if headless() || display_lost() {
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
        return headless_send(|| offscreen::Request::Font { id, family, px });
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
        return headless_send(|| offscreen::Request::Inset(px));
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
        return headless_send(|| offscreen::Request::Background(rgb));
    }
    if let Ok(g) = gui() {
        let _ = lock(g).send_event(UserEvent::Background { rgb });
    }
    Ok(())
}

/// `(gui-fg! rgb)` — set the default text colour (a text op whose face names no `:fg`) on
/// every window + the default for ones opened later. `None` restores `DEFAULT_FG`. No-op
/// (silently) if the GUI thread never started.
pub fn fg(rgb: Option<[u8; 3]>) -> Result<(), String> {
    if headless() {
        return headless_send(|| offscreen::Request::Foreground(rgb));
    }
    if let Ok(g) = gui() {
        let _ = lock(g).send_event(UserEvent::Foreground { rgb });
    }
    Ok(())
}

/// `(gui-line-height! mult)` — set the cell height as a multiple of the font px on
/// every window + the default for ones opened later. No-op (silently) if the GUI
/// thread never started.
pub fn line_height(mult: f32) -> Result<(), String> {
    if headless() {
        return headless_send(|| offscreen::Request::LineHeight(mult));
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
        return headless_send(|| offscreen::Request::TextAa(mode));
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
        return headless_send(|| offscreen::Request::TextContrast(gamma));
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
        return headless_send(|| offscreen::Request::Texture {
            id,
            tex,
            rgba,
            w,
            h,
        });
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
        return headless_send(|| offscreen::Request::TextureFree { id, tex });
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
        return headless_send(|| offscreen::Request::RegisterFamily {
            name,
            faces: Box::new(faces),
            paths,
        });
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
        return headless_send(|| offscreen::Request::RegisterInstalled { name, family_name });
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
pub fn font_info(family: Option<u32>) -> Result<super::super::FontInfo, String> {
    if headless_paint() {
        return offscreen::ask(|reply| offscreen::Request::FontInfo { family, reply });
    }
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
        return headless_send(|| offscreen::Request::Ligatures(on));
    }
    if let Ok(g) = gui() {
        let _ = lock(g).send_event(UserEvent::Ligatures { on });
    }
    Ok(())
}
