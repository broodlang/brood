//! Building a window inside the running event loop: its winit window and attributes, the
//! render backend it paints with (the CPU softbuffer, or the GPU target with a fallback to
//! the CPU painter), and a renderer set up from the global defaults.

use super::*;

/// One open window's GUI-thread-side state.
/// A window's render backend, chosen ONCE at `build_window`. The default `gui` build
/// only has `Cpu` (softbuffer). The `gui-gpu` build also has `Gpu` and picks it when
/// `BROOD_GUI_GPU` is set in the environment — so ONE installed binary defaults to the
/// CPU softbuffer (safe for every app) and a single project opts into the GPU
/// per-process via that env var, without a separate build.
pub(super) enum Backend {
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

/// Build a window + softbuffer surface + glyph renderer inside the running event
/// loop. Errors (window / surface creation) propagate to the `open` caller.
pub(super) fn build_window(
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
            Ok(gpu) => {
                for (what, reason) in gpu.degradations(spec.vsync) {
                    report_degraded(what, &reason, Some(subscriber));
                }
                Backend::Gpu(Box::new(gpu))
            }
            Err(e) => {
                report_degraded(
                    "gpu",
                    &format!("GPU render target unavailable ({e}); drawing with the CPU painter"),
                    Some(subscriber),
                );
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
    renderer.set_fg(defaults.fg);
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
        cursor_partial: false,
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
