//! The terminal primitives: raw-mode entry/exit, size, polling and drawing (`std/term.blsp`
//! is the policy). Two implementations share one registration table: [`native`] (crossterm)
//! and, on wasm32, [`wasm`], a stub that keeps every name and signature but reports that no
//! terminal exists. The window is `gui`.

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
    use crate::core::value::Arity;
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
        "Enter raw mode + the alternate screen, hide the cursor, and enable mouse capture and focus reports, taking over the terminal for a full-screen UI (so click/scroll and the terminal gaining or losing the keyboard reach term-poll). Pair with term-leave. (ADR-046 display seam, ADR-536.)",
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
        Sig::new(vec![], Ty::tuple_of(vec![int, int])),
        &[],
        "The terminal size as [cols rows] in character cells.",
        term_size,
    );
    primitives.def(
        "%term-poll",
        Arity::exact(1),
        Sig::new(vec![int], string.union(kw).union(nil_ty)),
        &["ms"],
        "Wait up to ms milliseconds for an input event; return a key (a 1-char string for printables, or a keyword for specials: :up :down :left :right :enter :escape :backspace :tab :back-tab :delete :home :end :page-up :page-down, ctrl combos like :ctrl-c, alt combos like :alt-f, and :ctrl-tab / :ctrl-shift-tab from a terminal that reports them — the kitty keyboard protocol or xterm modifyOtherKeys; elsewhere Ctrl+Tab arrives as :tab), a mouse event as a vector [:mouse action button row col mods] (action: :press :release :drag :scroll-up :scroll-down — :drag is motion with a button held, reported once per cell crossed; button: :left :right :middle or nil for scroll; row/col 0-based cells; mods a vector of held modifier keywords in :ctrl :alt :shift order, [] when none — so Ctrl+wheel etc. are bindable), or nil on timeout. After term-enter a key coming up is also delivered, as [:key-up press] with press the value that key's press delivered (though Ctrl or Shift came up first): reported by a terminal that speaks the kitty keyboard protocol, inferred in any other when a held key's repeats stop (a tapped key has no key-up there). A held key's repeat already superseded in the buffered input (by a later repeat or the release) is dropped, so a backlog never plays on after the key is up; a press is never dropped, nor typed text. term-held-key names the key held. After term-enter the terminal gaining the keyboard is also delivered, as :focus-in, and losing it as :blur — a window's spelling (ADR-536). Always pass a finite ms.",
        term_poll);
    primitives.def(
        "%term-held-key",
        Arity::exact(0),
        Sig::new(vec![], string.union(kw).union(nil_ty)),
        &[],
        "The key held down in the terminal now — the same value its press delivered (a 1-char string, or a keyword like :ctrl-n / :up) — or nil when none is. Reported by a terminal that speaks the kitty keyboard protocol (kitty, foot, WezTerm, Ghostty, Alacritty 0.13+), which term-enter turns on; inferred in any other, and for the keys the protocol still sends as text: a key is held once its repeats arrive faster than a hand presses one key, until they stop. Always nil outside term-enter.",
        term_held_key);
    primitives.def(
        "%term-forward-input",
        Arity::exact(1),
        Sig::new(vec![pid_ty.union(nil_ty.clone())], nil_ty.clone()),
        &["pid"],
        "Deliver every terminal input event — what term-poll would answer — to pid's PRIORITY lane from now on, from a reader thread, as a window delivers its input; nil stops it and waits for the thread. ADR-491.",
        term_forward_input);
    primitives.def(
        "%term-draw",
        Arity::exact(1),
        Sig::new(vec![vec_ty], nil_ty),
        &["frame"],
        "Paint a frame — a vector of render ops: [:clear], [:text row col str], [:text row col str face], [:rect row col w h face], [:cursor row col] / [:cursor row col style]. A face is a map like {:fg :red :bold true}; a colour is a palette keyword (:red … :dark-grey, the terminal's named colour) or an explicit [r g b] vector / \"#rrggbb\" hex string (a true-colour cell). [:rect …] fills a w×h cell block with the face's background (a solid panel). The optional cursor `style` is :block (default), :bar, or :underline — the steady caret shape; the terminal's cursor shows at the frame's last [:cursor] (a :hollow one only when there is no other) and is hidden in a frame with none. Only the cells that differ from what the terminal shows are written (ADR-586), in the colours the terminal has (term-colors). The in-process frontend for the display protocol; returns nil.",
        term_draw);
    primitives.def(
        "%term-invalidate",
        Arity::exact(0),
        Sig::new(vec![], nil_ty.clone()),
        &[],
        "Forget what the terminal shows, so the next term-draw clears it and writes every cell — for when something other than term-draw wrote to the screen. A resize does it by itself. Returns nil. (ADR-586)",
        term_invalidate);
    primitives.def(
        "%term-suspend",
        Arity::exact(0),
        Sig::nullary(kw),
        &[],
        "Give the terminal back (term-leave), stop this process with SIGTSTP (its default action) as a shell's C-z stops a job, and take the terminal again when it is continued (term-enter, the next frame painted whole): :suspended. When nothing could continue it — the process group is orphaned, as under `gnome-terminal -- prog` or `ssh host prog` — it keeps the terminal and answers :no-job-control. Unix only. (ADR-586)",
        term_suspend);
    primitives.def(
        "%term-refresh",
        Arity::exact(0),
        Sig::new(vec![], nil_ty.clone()),
        &[],
        "After being stopped and continued by someone else: raw mode back on and the next frame painted whole. Returns nil. (ADR-586)",
        term_refresh);
    primitives.def(
        "%term-repaint-due",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "Whether the next term-draw paints the screen whole — it was forgotten (a resize, a continue, term-invalidate) — so a loop that skips drawing an unchanged model must draw anyway. (ADR-586)",
        term_repaint_due);
    primitives.def(
        "%term-bg",
        Arity::range(1, 2),
        Sig::with_optional(vec![any], vec![any], nil_ty),
        &["bg", "fg"],
        "The terminal's default face: [:clear] fills with bg, and a cell whose face names no :bg / :fg is drawn in bg / fg — what gui-bg is to a window, so a theme made for a dark window is not drawn on a light terminal's own background. A colour is a palette keyword, [r g b] or \"#rrggbb\" (degraded to the terminal's depth); nil is the terminal's own. The next frame repaints whole. Returns nil. (ADR-586)",
        term_bg);
    primitives.def(
        "%term-colors",
        Arity::range(0, 1),
        Sig::with_optional(vec![], vec![kw.union(nil_ty.clone())], kw),
        &["depth"],
        "The colour depth term-draw writes at: :truecolor, :256 or :16 — read from $COLORTERM (truecolor / 24bit) and $TERM (…256color, …-direct) unless set. With depth, set it (nil goes back to the environment's) and repaint the next frame whole. A true colour is drawn as the nearest colour the depth has; a named colour (:red) is always the terminal's own. (ADR-586)",
        term_colors);
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
    primitives.def(
        "%audio-beep",
        Arity::range(2, 3),
        Sig::with_rest(vec![num, num], num, nil_ty),
        &["freq-hz", "ms", "vol"],
        "Play a short tone of freq-hz for ms milliseconds, optionally at peak amplitude vol (0..1, default ~0.18 — pass a small vol for quiet/ambient sounds). Fire-and-forget — it never blocks the caller, and overlapping beeps mix — so a game can blip from its frame loop. Synthesised on a dedicated audio thread (needs --features audio). A graceful no-op without the feature, when there's no audio device, or when muted via BROOD_AUDIO=0 or BROOD_GUI_HEADLESS. Returns nil.",
        audio_beep);
}
