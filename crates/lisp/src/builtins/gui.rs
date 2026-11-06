//! The windowed frontend's primitives (ADR-046): `%gui-*`, the same seam as `term-*`, painting
//! the render-op protocol to a native window (`std/gui.blsp` is the policy). Two
//! implementations share this table: [`native`] and, on wasm32, [`wasm`], a stub that keeps
//! every name and signature but reports that no window exists.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::builtins) use native::*;

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub(in crate::builtins) use wasm::*;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    // The windowed (GUI) frontend — the same seam as `term-*`, painting the same
    // render-op protocol to a native window (feature "gui"; the symbols always
    // exist, erroring at call time without the feature). Unlike the single
    // terminal, there can be many windows: `gui-open` returns an integer window id
    // and the other primitives take it, so `(observe)` can spawn several at once.
    // std/tool/observer.blsp's `gui-display` wraps an id as a display map. See gui.rs.
    primitives.def(
        "%gui-open",
        Arity::range(0, 4),
        // Every optional arg is also nil-able in place (`(gui-open title nil nil
        // opts)` opens at the default size), so the params say so — the runtime
        // treats nil as "use the default" for each.
        Sig::new(
            vec![
                string.union(nil_ty),
                int.union(nil_ty),
                int.union(nil_ty),
                map_ty.union(nil_ty),
            ],
            int,
        ),
        &["title?", "width?", "height?", "opts?"],
        "Open a new native window and return its integer id (needs the runtime built with --features gui; errors otherwise). An optional `title` string sets the OS title-bar text (default `Brood`); change it later with gui-title!. Optional `width` `height` (logical pixels, both required together) set the initial window size (default 840x560). Optional `opts` map, the attributes fixed when the window is built: `{:decorations false}` opens a **borderless** window — no OS title bar or frame — for an app that draws its own chrome (a browser's tab strip and toolbar) and would otherwise sit under a redundant second title; `{:app-id \"my-app\"}` sets the desktop application id (Wayland `app_id`, X11 `WM_CLASS`), which the desktop matches against the installed `my-app.desktop` entry to give the window its real icon and name in the dash / alt-tab — without one it is unidentifiable and draws the desktop's generic fallback icon (on Wayland a client cannot supply icon pixels at all, so this, not gui-icon!, is how a window gets an icon there). Its key/mouse input is delivered to the CALLING process's mailbox as messages — a key as a 1-char string / keyword (`:up`, `:ctrl-c`), the mouse as `[:mouse action button row col mods]` (action `:press`/`:release`/`:drag`/`:move`/`:scroll-up`/`:scroll-down` — `:drag` is motion with a button held and `:move` is bare motion with none (button nil), both delivered once per cell crossed (so mouse-look / hover need no click); `mods` a vector of held modifier keywords in `:ctrl :alt :shift` order, `[]` when none, so Ctrl+wheel / Ctrl+drag are bindable; a `:press` carries a trailing 7th element, its click-chain count `[… mods n]` — 1 single, 2 double, 3 triple, … for repeated presses of the same button in the same cell within the double-click window, so double-click-to-select-word and triple-click-to-select-line are bindable; the terminal reports 1), a resize as `[:resize cols rows]` (the new cell grid, so the loop re-renders at the new size) — so the consumer parks in `(receive)` instead of polling (ADR-059). Clicking the window's close button delivers a dedicated `:close` message — distinct from the Escape *key* (`:escape`), so an app can quit on the X without conflating it with Escape (which an editor binds to cancel/normal-mode); `ui-run` quits on `:close` automatically. Starts the GUI thread on the first call; each call is an independent window, so several observers can run at once. Pass the id to the other gui-* primitives; pair with gui-close.",
        gui_open);
    primitives.def(
        "%gui-compiled?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "Whether this runtime was built with the GUI backend (--features gui). Ask it before a gui/* call meant to be skipped headlessly, instead of calling and catching.",
        gui_compiled_p,
    );
    primitives.def(
        "%gui-close",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &["id"],
        "Close window id (the teardown for gui-open). Idempotent; an unknown id is a no-op.",
        gui_close,
    );
    primitives.def(
        "%gui-title!",
        Arity::exact(2),
        Sig::new(vec![int, string], nil_ty),
        &["id", "text"],
        "Set window id's OS title-bar text to the string text at runtime (the title gui-open gave it, or the default, otherwise). Needs --features gui; a no-op if the GUI thread never started or id isn't a live window. Returns nil.",
        gui_title);
    primitives.def(
        "%gui-icon!",
        Arity::exact(4),
        Sig::new(vec![int, vec_ty, int, int], nil_ty),
        &["id", "rgba", "w", "h"],
        "Set window id's taskbar / title-bar icon from raw RGBA pixels: rgba is a vector of w*h*4 byte ints (0-255), row-major, 4 per pixel (red, green, blue, alpha). Needs --features gui; a silent no-op if the GUI thread never started, id isn't a live window, or the data length isn't w*h*4. Where the OS shows it depends on the platform (X11/Windows use it directly; Wayland prefers a .desktop file). Returns nil.",
        gui_icon);
    primitives.def(
        "%gui-focus",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &["id"],
        "Raise window id to the front and give it OS keyboard focus, un-minimising it first. Lets an app surface an already-open (singleton) window instead of opening a duplicate — e.g. `(observe)` focuses its existing window rather than spawning a second. Errors only if id isn't a live window. Needs --features gui. Returns nil.",
        gui_focus);
    primitives.def(
        "%gui-grab-cursor",
        Arity::exact(2),
        Sig::new(vec![int, bool_ty], nil_ty),
        &["id", "on"],
        "Confine the pointer to window id while `on` is truthy, release it otherwise — for mouse-look that shouldn't let the cursor slip out of the window and click another app. Uses the platform's `Confined` grab (cursor stays inside but keeps moving, so an absolute position-based look maps edge-to-edge), falling back to `Locked` where that's all the platform offers. Off by default; an app opts in. Errors only if id isn't a live window. Needs --features gui. Returns nil.",
        gui_grab_cursor);
    primitives.def(
        "%gui-busy",
        Arity::exact(2),
        Sig::new(vec![int, bool_ty], nil_ty),
        &["id", "on"],
        "Show window id's busy (wait) cursor while `on` is truthy, as if the loop its input feeds were past its quit key's busy-ms (ADR-491) — for a thin client whose loop runs on another node (`editor/serve`), which the window cannot ask itself. Off restores the hover shape, unless the window's own loop is busy. Errors only if id isn't a live window. Needs --features gui. Returns nil.",
        gui_busy);
    primitives.def(
        "%gui-fullscreen!",
        Arity::exact(2),
        Sig::new(vec![int, bool_ty], nil_ty),
        &["id", "on"],
        "Make window id borderless-fullscreen while `on` is truthy (covering the whole monitor it's on, NO title bar / decorations — distraction-free), or restore it to a normal window otherwise. For a big-but-normal window that keeps its title bar, use gui-maximize! instead. The fullscreen/restore triggers a resize, so the consumer gets the usual [:resize cols rows] message and re-renders at the new size. Errors only if id isn't a live window. Needs --features gui. Returns nil.",
        gui_fullscreen);
    primitives.def(
        "%gui-maximize!",
        Arity::exact(2),
        Sig::new(vec![int, bool_ty], nil_ty),
        &["id", "on"],
        "Maximise window id while `on` is truthy (fill the screen's work area, KEEPING the title bar / decorations), or restore it to its previous size otherwise — e.g. an editor's init file opening big without going true-fullscreen. The maximise/restore triggers a resize, so the consumer gets the usual [:resize cols rows] message and re-renders at the new size. Errors only if id isn't a live window. Needs --features gui. Returns nil.",
        gui_maximize);
    primitives.def(
        "%gui-minimize!",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &["id"],
        "Iconify window `id`. The counterpart of gui-maximize! for an app that draws its own window controls, which a borderless window (gui-open with {:decorations false}) must.",
        gui_minimize);
    primitives.def(
        "%gui-drag-move",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &["id"],
        "Hand window `id` to the window manager for an interactive move, for the rest of the currently-held press. What a borderless window needs to stay movable: with no OS title bar there is nothing to grab, so the app nominates a region of its own chrome (a browser's tab strip) and calls this when a press lands there. A platform that declines the gesture is a no-op, not an error.",
        gui_drag_move);
    primitives.def(
        "%gui-drag-resize",
        Arity::exact(2),
        Sig::new(vec![int, kw], nil_ty),
        &["id", "dir"],
        "Hand window `id` to the window manager for an interactive resize from `dir` — :north :south :east :west :north-east :north-west :south-east :south-west. The window-frame counterpart of gui-drag-move, for a borderless window that draws its own edges. A platform that declines the gesture is a no-op, not an error.",
        gui_drag_resize);
    primitives.def(
        "%gui-size",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        &["id"],
        "Window id's size as [cols rows] in character cells (tracks resize / HiDPI), same shape as term-size.",
        gui_size);
    primitives.def(
        "%gui-size-px",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        &["id"],
        "Window id's inner size as [w h] in PHYSICAL pixels (tracks resize / HiDPI) — the frame a pixel-space app (`:sprite` / `:quad` ops, `{:input :pixels}`) lays itself out in, where gui-size is the cell grid.",
        gui_size_px);
    primitives.def(
        "%gui-latency",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        &["id"],
        "Window id's recent input-to-present samples, oldest first (at most 64), each a map of milliseconds, with :input (:key or :pointer, ADR-612): :total — from the GUI thread receiving an input (a key press, a mouse button, a wheel step) to the paint of the first frame that arrived after it returning from its present — and :paint, that paint's share; a frame drawn by the GPU render target adds its phases :encode :upload :acquire :submit :present. Which frame answers an input cannot be known across the app's mailbox, so the next frame drawn is taken as the answer; a present returning means the compositor has the frame, not that the panel shows it (up to one refresh more). Empty headless. BROOD_GUI_TRACE=1 prints each sample as it is taken. ADR-440.",
        gui_latency);
    primitives.def(
        "%gui-latency-histogram",
        Arity::exact(1),
        Sig::new(vec![int], map_ty),
        &["id"],
        "Window id's input-to-present times since it opened as a distribution, keys apart from the pointer: {:key h :pointer h}, each h a map of :count (inputs answered), :max-us (the slowest, microseconds) and :buckets, a map from a bucket's largest value in microseconds to how many inputs fell in it — exact below 16 us, then 16 buckets per power of two, so a bucket is at most 6.25% wide. A key is a press, an OS repeat or an input method's commit; the pointer a button or a wheel step. Histograms add bucket by bucket (gui/latency-merge, gui/latency-since) and gui/latency-percentiles reads p50 / p99 / p99.9 off one. Both empty headless. ADR-612.",
        gui_latency_histogram);
    primitives.def(
        "%gui-key-repeat?",
        Arity::exact(1),
        Sig::new(vec![int], bool_ty),
        &["id"],
        "Whether the last key press window id's app took from its mailbox was an OS auto-repeat of a key held down (under {:key-repeat :os}) rather than a fresh press. Each press marks it as it leaves the mailbox, so asked while folding a press it answers for that press exactly; a repeat taken back at the release marks nothing. False before any key, under {:key-repeat :app} (no repeat is delivered), for a window not open, and headless. ADR-612.",
        gui_key_repeat);
    primitives.def(
        "%gui-key-repeats",
        Arity::exact(1),
        Sig::new(vec![int], map_ty),
        &["id"],
        "How window id's held keys have repeated since it opened, as a map: :delivered, the OS repeats delivered to the app; :dropped, those dropped because one was still waiting (the app fell behind the repeat rate); and, from the last key that repeated, the desktop's :delay before its first repeat and the :interval between its last two, in milliseconds (nil before any key has repeated). Counts every repeat under {:key-repeat :os}; under :app nothing is delivered or dropped, the timings still kept. All zero headless. ADR-441.",
        gui_key_repeats);
    primitives.def(
        "%gui-settle",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &["id"],
        "Tell window id the app finished a turn with nothing new to draw; returns nil. What a gui-draw would have told the window about the app's progress, without a frame: the next wheel-glide step may fire, and the input the turn answered is dropped from the latency trace rather than pinned on the next unrelated frame. ui-run sends it on a turn it skips (ADR-444). No-op headless.",
        gui_settle);
    primitives.def(
        "%gui-texture",
        Arity::exact(5),
        Sig::new(vec![int, int, Ty::of_tags(&[Tag::Bytes, Tag::Vector]), int, int], nil_ty),
        &["id", "tex", "rgba", "w", "h"],
        "Upload rgba — w*h*4 bytes (a bytes value or a vector of 0-255 ints), row-major, straight alpha — as texture tex of window id, for its [:sprite …] ops. The handle tex is the caller's (gui/texture allocates it), so a sheet's size can be kept beside it; re-uploading under a live handle replaces the texture. Errors when the byte count is not w*h*4 or tex is negative; a silent no-op headless or on the CPU render target, which has no sprite arm. Returns nil.",
        gui_texture);
    primitives.def(
        "%gui-texture-free",
        Arity::exact(2),
        Sig::new(vec![int, int], nil_ty),
        &["id", "tex"],
        "Release texture tex of window id; a later [:sprite …] naming it draws nothing. Idempotent. Returns nil.",
        gui_texture_free);
    primitives.def(
        "%gui-input!",
        Arity::exact(2),
        Sig::new(vec![int, kw], nil_ty),
        &["id", "mode"],
        "Deliver window id's mouse input in :pixels or :cells from now on — the `{:input …}` gui-open option, switchable after the window exists (how a window a generic thin client opened is told by the app it attached to which unit it wants). The window then reports its size in that unit ([:resize w h] in pixels, or [:resize cols rows]). Returns nil.",
        gui_input);
    primitives.def(
        "%gui-sound",
        Arity::exact(5),
        Sig::new(vec![int, int, int, int, Ty::of_tags(&[Tag::Bytes, Tag::Vector])], nil_ty),
        &["id", "snd", "rate", "channels", "pcm"],
        "Keep pcm — interleaved PCM16 little-endian bytes (a bytes value or a vector of 0-255 ints) at rate Hz (1..384000) with channels (1 or 2) — as sound snd of window id, for its [:sound snd vol] and [:music snd vol] frame ops. The handle snd is the caller's (gui/sound allocates it), as a texture's is. Errors on a bad rate/channel count or an odd byte count; a silent no-op headless. Returns nil.",
        gui_sound);
    primitives.def(
        "%gui-sound-free",
        Arity::exact(2),
        Sig::new(vec![int, int], nil_ty),
        &["id", "snd"],
        "Forget sound snd of window id; a later [:sound snd] op plays nothing. Idempotent. Returns nil.",
        gui_sound_free);
    primitives.def(
        "%gui-cell-size",
        Arity::range(1, 2),
        Sig::with_optional(vec![int], vec![Ty::of_tags(&[Tag::Int, Tag::Float])], vec_ty),
        &["id", "px"],
        "Window id's cell size as [cell-w cell-h] in PHYSICAL pixels — for the font size the window is using, or, given px (a logical font size), for the size a [:cell-region …] of that px would paint with. The question an app must answer before laying a region out at another size: how many of THAT size's cells fit the rect it has. It is not derivable from px — a cell is whatever shaping the reference glyph produces, rounded to whole pixels — so it is measured (on the GUI thread, memoised there). Errors when the window is not open, when this brood has no gui feature, and headless (no font engine to measure with), so a caller that also runs headless falls back to the size ratio.",
        gui_cell_size);
    primitives.def(
        "%gui-held-key",
        Arity::exact(1),
        Sig::new(vec![int], string.union(kw).union(nil_ty)),
        &["id"],
        "The key window id currently sees as physically held — the same value its press delivered (a 1-char string, or a keyword like :ctrl-n / :up) — or nil when none is held. Tracked from press/release transitions in the event loop (NOT winit's ke.repeat, unreliable on Wayland), so it's the source of truth for a held key: a consumer-paced auto-repeat polls it each tick and stops the instant it no longer matches, so a missed key-up (e.g. lost on focus change) can't cause runaway repeat.",
        gui_held_key);
    primitives.def(
        "%gui-draw",
        Arity::exact(2),
        Sig::new(vec![int, vec_ty], nil_ty),
        &["id", "frame"],
        "Paint a frame (the same render-op vector term-draw takes) to window id; returns nil. Unknown ops are skipped (forward-compatible). A text op's face may carry :scale n (GUI only, integer >=1, capped at 16): the text is drawn n× larger in an n×n block of cells anchored at its row/col — the per-pane/per-buffer font knob; the terminal frontend renders scale 1. A `[:cursor row col]` op may carry an optional `style` keyword (`[:cursor row col style]`) — :block (default, a 50% overlay), :bar (a thin caret on the cell's left edge), :underline (a rule along the cell bottom), or :hollow (an outline box, the cursor of an unfocused window) — and an optional colour after it (`[:cursor row col style colour]`, a face colour; `style` may be nil), the caret's colour: a block washes its cell in it at 50% and rims it solid, the other shapes are solid in it. A sixth element `{:interval ms :blinks n}` blinks the caret in the window itself, between frames (ADR-469). A `[:ui/memo key deps ops reused?]` marker (a `:fragments?` display's frame) paints its ops in place; a reused one is not parsed again (ADR-470). A `[:seam row]` op says how the rows sit in the window (ADR-610): rows above `row` are anchored to the top, rows from it down to the bottom, so the last row ends exactly at the bottom inset at every window height and the leftover pixels lie between row `row - 1` and row `row`, painted by the row above (an editor names its bottom mode line; nil keeps the leftover under the last row); `[:seam row {:top-band false}]` also says row 0 is not a band, so the top gets the inset like the sides (the grid loses those rows, and the window sends a resize when this changes). Rows stay whole cells, and the mouse hit-test inverts the same map; frame state, read at the top level, skipped by the terminal. An `[:above-seam ops]` block (ADR-615) lays its ops out on the rows above the seam carried on past it and cuts them off where the seam row begins, so what it puts on the seam row shows its top in the leftover strip, the next line partly visible, once the strip is at least half a row tall (under that the strip stays the row above's, as without the block); a pointer in a strip showing the line is reported on the seam row with `:partial true` in the trailing map. 
A `[:rect row col w h face]` op fills a w×h cell block with the face's background colour — a solid panel painted directly (no glyphs), the multi-row generalisation of a status bar. A `[:cursor-zone x y w h shape]` op marks a hover hot-zone: while the pointer is over it the window shows the resize cursor `shape` (:col-resize ↔ / :row-resize ↕), hit-tested on the GUI thread (ADR-080); it draws nothing and the terminal ignores it. A `[:vspans row0 col0 cols]` op is the column-renderer fast path (raycasters, spectrum bars): `cols` is a vector with one entry per cell-column (`col0`, `col0+1`, …), each a top-to-bottom stack of `[height colour]` segments painted from `row0` down — `colour` a face keyword (`:red`), an `[r g b]` triple (0..255), or nil (transparent). The per-cell fill happens natively here, so a wide scene costs the Brood side O(columns), not O(cells); GUI-only (the terminal ignores it).",
        gui_draw);
    // Frame capture (ADR-609): what a window paints, as pixels, through the window's own
    // CPU painter — for a test, an agent, a person reviewing a UI change.
    primitives.def(
        "%gui-render-frame",
        Arity::exact(2),
        Sig::new(vec![vec_ty, map_ty.union(nil_ty)], map_ty),
        &["frame", "spec"],
        "Paint frame (a %gui-draw frame) into memory as a window described by spec would paint it, and return the picture {:width :height :rgba :cell [w h] :grid [cols rows] :origin [x y] :inset px :seam row :row-tops [y …]} — :rgba width*height*4 bytes, :row-tops the pixel row each grid row starts at (ADR-610). spec keys (all optional): :width/:height (physical px, 840x560), :font (an installed family's name, or a family keyword the offscreen painter knows), :size (font px, 15), :line-height (1.4), :inset (logical px, 0), :bg (a colour), :scale (1), :text-aa, :ligatures. The window's own CPU painter, layout rule and edge bleed; needs no window or display. Needs --features gui.",
        gui_render_frame);
    primitives.def(
        "%gui-capture",
        Arity::exact(1),
        Sig::new(vec![int], map_ty),
        &["id"],
        "Window id's current frame painted into memory as the window paints it — the picture map %gui-render-frame returns. Works for a real window and, headless, for a window opened under BROOD_GUI_HEADLESS=paint; a plain headless window has no painter, so that errors. Needs --features gui.",
        gui_capture);
    primitives.def(
        "%gui-windows",
        Arity::exact(0),
        Sig::new(vec![], vec_ty),
        &[],
        "The ids of the windows open in this runtime, oldest first — empty without --features gui.",
        gui_windows,
    );
    // The font seam: a global default cell font (`gui-font!`) and runtime family
    // registration (`gui-font-register`); a face's `:family`/`:italic` then select
    // per-section, within the fixed cell grid. (gui feature; error without it.)
    primitives.def(
        "%gui-font!",
        // (gui-font! spec) or (gui-font! id spec): arg 0 is a window id (int) or the
        // spec map; the optional arg 1 is the spec map when an id leads.
        Arity::range(1, 2),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Map]), map_ty], nil_ty),
        &["id?", "spec"],
        "Set a cell font from spec, a map {:family <keyword> :height <px>} (both keys optional): :family picks a registered font family (bundled :mono, or one added by gui-font-register), :height the cell pixel size. (gui-font! spec) sets the global default — every open window and ones opened later; (gui-font! id spec) retunes just window id, leaving the global default and other windows alone, so two windows can run different fonts. Per-section fonts within a window come from a face's :family/:scale. Needs --features gui. Returns nil.",
        gui_font);
    primitives.def(
        "%gui-font-register",
        Arity::exact(2),
        Sig::new(vec![kw, map_ty], kw),
        &["name", "styles"],
        "Register font family name (a keyword) from styles: either an INSTALLED family by its name, {:family-name \"JetBrains Mono\"} — looked up (case-insensitively) in the system fonts, which are scanned the first time one is asked for and never at startup, with the bold / italic / bold-italic faces picked by weight and slant, and the bundled DejaVu Sans Mono standing in when nothing by that name is installed (gui-font-info says which file each style got, or :not-found) — or a map of style → TTF file path {:regular \"…\" :bold \"…\" :italic \"…\" :bold-italic \"…\"}, where only :regular is required and a missing style reuses the regular file. :family-name wins when both are given. Afterwards a face's :family <name> (or gui-font!) selects it; re-registering a family a window already uses re-measures its cell. Needs --features gui. Returns name.",
        gui_font_register);
    // The window content inset (`gui-inset!`): a blank pixel margin before the cell
    // grid on every edge, so a GUI app's text breathes instead of sitting flush.
    primitives.def(
        "%gui-inset!",
        Arity::exact(1),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Float])], nil_ty),
        &["px"],
        "Set the window content inset to px logical pixels: a blank margin on the left, right and bottom of the cell grid, so a GUI app's text breathes instead of sitting flush against the frame. There is none above row 0 unless the frame says row 0 is not a band (`[:seam row {:top-band false}]`, ADR-610): the first row starts at the window's top edge (ADR-508). Applies to every open window and the default for ones opened later; the grid loses 2*px of width and px of height (fewer cells) and re-renders. The inset is shared by the renderer and mouse hit-testing, so clicks stay aligned. Needs --features gui. Returns nil.",
        gui_inset);
    // The window background (`gui-bg!`): the clear / inset-margin / snap-remainder fill,
    // so a GUI app's padding matches its theme instead of the hardcoded default.
    primitives.def(
        "%gui-bg!",
        Arity::exact(1),
        Sig::new(
            vec![Ty::of_tags(&[
                Tag::Keyword,
                Tag::Vector,
                Tag::Str,
                Tag::Nil,
            ])],
            nil_ty,
        ),
        &["color"],
        "Set the window background colour: the fill for :clear, the per-frame pre-clear, and — being outside every cell — the gui-inset! margin and the cell-grid snap remainder. So a GUI app's padding matches its own theme background instead of the hardcoded default. color is a keyword named colour, an [r g b] vector (0..255 per channel), or a \"#rrggbb\"/\"#rgb\" hex string; nil restores the default. Applies to every open window and the default for ones opened later (a pure repaint — no grid change). Needs --features gui. Returns nil.",
        gui_bg);
    // The default text colour (`gui-fg!`): what a text op whose face names no :fg is
    // drawn in — gui-bg!'s partner, so a light theme's plain text is not the hardcoded
    // light default (ADR-613).
    primitives.def(
        "%gui-fg!",
        Arity::exact(1),
        Sig::new(
            vec![Ty::of_tags(&[
                Tag::Keyword,
                Tag::Vector,
                Tag::Str,
                Tag::Nil,
            ])],
            nil_ty,
        ),
        &["color"],
        "Set the window's default text colour: what a text op whose face names no :fg is drawn in (and the background of a reversed face that names none), as gui-bg! is the colour of what no op paints. So a light theme's plain text is its own dark text instead of the hardcoded light default. color is a keyword named colour, an [r g b] vector (0..255 per channel), or a \"#rrggbb\"/\"#rgb\" hex string; nil restores the default. Applies to every open window and the default for ones opened later (a pure repaint — no grid change). Needs --features gui. Returns nil.",
        gui_fg);
    // The line height (`gui-line-height!`): the cell height as a multiple of the font
    // px — 1.4 by default (an editor's breathing room); denser code wants ~1.25.
    primitives.def(
        "%gui-line-height!",
        Arity::exact(1),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Float])], nil_ty),
        &["mult"],
        "Set the cell height as a multiple of the font pixel size (the line height): 1.4 by default — an editor's vertical breathing room — clamped to 0.8..3.0. A metric change like gui-font!: the row count moves, so every open window is told its new (cols, rows) and re-renders; also the default for windows opened later. Needs --features gui. Returns nil.",
        gui_line_height);
    // Text anti-aliasing (`gui-text-aa!`): gray (one coverage per pixel) or subpixel
    // (one per colour channel — LCD text, three times the horizontal resolution).
    primitives.def(
        "%gui-text-aa!",
        Arity::exact(1),
        Sig::new(vec![Ty::of_tags(&[Tag::Keyword])], nil_ty),
        &["mode"],
        "Set how monochrome text is anti-aliased: :gray (one coverage value per pixel), :subpixel (one per colour channel, each a third of a pixel apart — LCD text, three times the horizontal resolution of every stem, for a panel whose subpixels run red-green-blue), :bgr (the same for a blue-first panel), or :auto (the default: subpixel at a 1× scale where text has the fewest pixels to spend, gray on HiDPI where grayscale is already sharp and a scaled or rotated surface would turn subpixel fringes into colour noise). Applies to every open window and the default for ones opened later; a pure repaint. Needs --features gui. Returns nil.",
        gui_text_aa);
}
