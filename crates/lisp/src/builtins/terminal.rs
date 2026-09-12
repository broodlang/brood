//! The terminal and GUI primitives: raw-mode entry/exit, size, polling, drawing, and
//! the `%gui-*` window surface (`std/term.blsp` / `std/gui.blsp` are the policy). Two
//! implementations share one registration table: [`native`] (crossterm + the `gui`
//! feature's winit window) and, on wasm32, [`wasm`] — a stub that keeps every name and
//! signature but reports that no terminal exists.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::*;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    // terminal frontend (ADR-046) — the thin crossterm seam that paints the
    // display protocol and reads keys. The protocol itself is Brood data (a
    // vector of render ops); these primitives are mechanism only. `term-poll`
    // returns a key (a 1-char string, or a keyword for specials) or nil on
    // timeout; `term-draw` interprets a frame vector. See std/tool/observer.blsp.
    primitives.def(
        "%term-enter",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        &[],
        "Enter raw mode + the alternate screen, hide the cursor, and enable mouse capture, taking over the terminal for a full-screen UI (so click/scroll reach term-poll). Pair with term-leave. (ADR-046 display seam.)",
        term_enter);
    primitives.def(
        "%term-leave",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        &[],
        "Restore the terminal: show the cursor, disable mouse capture, leave the alternate screen, disable raw mode. The normal-path teardown for term-enter.",
        term_leave);
    primitives.def(
        "%term-size",
        Arity::exact(0),
        Sig::new(vec![], vec_ty),
        &[],
        "The terminal size as [cols rows] in character cells.",
        term_size,
    );
    primitives.def(
        "%term-poll",
        Arity::exact(1),
        Sig::new(vec![int], string.union(kw).union(nil_ty)),
        &["ms"],
        "Wait up to ms milliseconds for an input event; return a key (a 1-char string for printables, or a keyword for specials: :up :down :left :right :enter :escape :backspace :tab :back-tab :delete :home :end :page-up :page-down, ctrl combos like :ctrl-c, alt combos like :alt-f), a mouse event as a vector [:mouse action button row col mods] (action: :press :release :drag :scroll-up :scroll-down — :drag is motion with a button held, reported once per cell crossed; button: :left :right :middle or nil for scroll; row/col 0-based cells; mods a vector of held modifier keywords in :ctrl :alt :shift order, [] when none — so Ctrl+wheel etc. are bindable), or nil on timeout. Always pass a finite ms.",
        term_poll);
    primitives.def(
        "%term-draw",
        Arity::exact(1),
        Sig::new(vec![vec_ty], nil_ty),
        &["frame"],
        "Paint a frame — a vector of render ops: [:clear], [:text row col str], [:text row col str face], [:rect row col w h face], [:cursor row col] / [:cursor row col style]. A face is a map like {:fg :red :bold true}; a colour is a palette keyword (:red … :dark-grey, the terminal's named colour) or an explicit [r g b] vector / \"#rrggbb\" hex string (a true-colour cell). [:rect …] fills a w×h cell block with the face's background (a solid panel). The optional cursor `style` is :block (default), :bar, or :underline — the steady caret shape. The in-process frontend for the display protocol; returns nil.",
        term_draw);
    // Inline (relative-motion) variant of the seam, for an in-place line editor
    // that must NOT take over the screen: `term-raw-enter`/`term-raw-leave` toggle
    // raw mode only (no alternate screen, cursor stays visible, scrollback kept),
    // and `term-emit` paints relative ops. The self-hosted REPL editor uses these
    // (std/editor/lineedit.blsp); `term-enter`/`term-draw` stay the full-screen path.
    primitives.def(
        "%term-raw-enter",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        &[],
        "Enter raw mode only — NO alternate screen, cursor stays visible, scrollback preserved. The seam for an inline line editor (the REPL); use term-enter instead for a full-screen TUI. Pair with term-raw-leave.",
        term_raw_enter);
    primitives.def(
        "%term-raw-leave",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        &[],
        "Leave raw mode (the teardown for term-raw-enter). Idempotent with the panic-path restore.",
        term_raw_leave,
    );
    primitives.def(
        "%term-emit",
        Arity::exact(1),
        Sig::new(vec![vec_ty], nil_ty),
        &["ops"],
        "Paint inline, relative-motion render ops (for an in-place editor that must not take over the screen): [:print str], [:print str face], [:cr], [:nl], [:up n], [:down n], [:col n], [:clear-eol], [:clear-below], [:clear-screen]. A face is a map like {:fg :cyan :bold true}. Queues all ops then flushes once; unknown ops are skipped; returns nil.",
        term_emit);
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
        "Open a new native window and return its integer id (needs the runtime built with --features gui; errors otherwise). An optional `title` string sets the OS title-bar text (default `Brood`); change it later with gui-title!. Optional `width` `height` (logical pixels, both required together) set the initial window size (default 840x560). Optional `opts` map, the attributes fixed when the window is built: `{:decorations false}` opens a **borderless** window — no OS title bar or frame — for an app that draws its own chrome (a browser's tab strip and toolbar) and would otherwise sit under a redundant second title; `{:app-id \"my-app\"}` sets the desktop application id (Wayland `app_id`, X11 `WM_CLASS`), which the desktop matches against the installed `my-app.desktop` entry to give the window its real icon and name in the dash / alt-tab — without one it is unidentifiable and draws the desktop's generic fallback icon (on Wayland a client cannot supply icon pixels at all, so this, not gui-icon!, is how a window gets an icon there). Its key/mouse input is delivered to the CALLING process's mailbox as messages — a key as a 1-char string / keyword (`:up`, `:ctrl-c`), the mouse as `[:mouse action button row col mods]` (action `:press`/`:release`/`:drag`/`:move`/`:scroll-up`/`:scroll-down` — `:drag` is motion with a button held and `:move` is bare motion with none (button nil), both delivered once per cell crossed (so mouse-look / hover need no click); `mods` a vector of held modifier keywords in `:ctrl :alt :shift` order, `[]` when none, so Ctrl+wheel / Ctrl+drag are bindable; a `:press` carries a trailing 7th element, its click-chain count `[… mods n]` — 1 single, 2 double, 3 triple, … for repeated presses of the same button in the same cell within the double-click window, so double-click-to-select-word and triple-click-to-select-line are bindable; the terminal reports 1), a resize as `[:resize cols rows]` (the new cell grid, so the loop re-renders at the new size) — so the consumer parks in `(receive)` instead of polling (ADR-058). Clicking the window's close button delivers a dedicated `:close` message — distinct from the Escape *key* (`:escape`), so an app can quit on the X without conflating it with Escape (which an editor binds to cancel/normal-mode); `ui-run` quits on `:close` automatically. Starts the GUI thread on the first call; each call is an independent window, so several observers can run at once. Pass the id to the other gui-* primitives; pair with gui-close.",
        gui_open);
    primitives.def(
        "%audio-beep",
        Arity::range(2, 3),
        Sig::with_rest(vec![num, num], num, nil_ty),
        &["freq-hz", "ms", "vol"],
        "Play a short tone of freq-hz for ms milliseconds, optionally at peak amplitude vol (0..1, default ~0.18 — pass a small vol for quiet/ambient sounds). Fire-and-forget — it never blocks the caller, and overlapping beeps mix — so a game can blip from its frame loop. Synthesised on a dedicated audio thread (needs --features audio). A graceful no-op without the feature, when there's no audio device, or when muted via BROOD_AUDIO=0 or BROOD_GUI_HEADLESS. Returns nil.",
        audio_beep);
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
        "Paint a frame (the same render-op vector term-draw takes) to window id; returns nil. Unknown ops are skipped (forward-compatible). A text op's face may carry :scale n (GUI only, integer >=1, capped at 16): the text is drawn n× larger in an n×n block of cells anchored at its row/col — the per-pane/per-buffer font knob; the terminal frontend renders scale 1. A `[:cursor row col]` op may carry an optional `style` keyword (`[:cursor row col style]`) — :block (default, a 50% overlay), :bar (a thin caret on the cell's left edge), or :underline (a rule along the cell bottom). A `[:rect row col w h face]` op fills a w×h cell block with the face's background colour — a solid panel painted directly (no glyphs), the multi-row generalisation of a status bar. A `[:cursor-zone x y w h shape]` op marks a hover hot-zone: while the pointer is over it the window shows the resize cursor `shape` (:col-resize ↔ / :row-resize ↕), hit-tested on the GUI thread (ADR-080); it draws nothing and the terminal ignores it. A `[:vspans row0 col0 cols]` op is the column-renderer fast path (raycasters, spectrum bars): `cols` is a vector with one entry per cell-column (`col0`, `col0+1`, …), each a top-to-bottom stack of `[height colour]` segments painted from `row0` down — `colour` a face keyword (`:red`), an `[r g b]` triple (0..255), or nil (transparent). The per-cell fill happens natively here, so a wide scene costs the Brood side O(columns), not O(cells); GUI-only (the terminal ignores it).",
        gui_draw);
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
        "Register font family name (a keyword) from styles, a map of style → TTF file path {:regular \"…\" :bold \"…\" :italic \"…\" :bold-italic \"…\"}. Only :regular is required; a missing style reuses the regular file. Afterwards a face's :family <name> (or gui-font!) selects it. Needs --features gui. Returns name.",
        gui_font_register);
    // The window content inset (`gui-inset!`): a blank pixel margin before the cell
    // grid on every edge, so a GUI app's text breathes instead of sitting flush.
    primitives.def(
        "%gui-inset!",
        Arity::exact(1),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Float])], nil_ty),
        &["px"],
        "Set the window content inset to px logical pixels: a blank margin before the cell grid on every window edge, so a GUI app's text breathes instead of sitting flush against the frame. Applies to every open window and the default for ones opened later; the grid loses 2*px per axis (fewer cells) and re-renders. The inset is shared by the renderer and mouse hit-testing, so clicks stay aligned. Needs --features gui. Returns nil.",
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
