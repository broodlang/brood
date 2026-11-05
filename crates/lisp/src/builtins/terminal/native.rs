use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use crate::builtins::args::{arg, expect_bigint, expect_int, expect_string};
use crate::builtins::bytes::collect_bytes;
use crate::builtins::io::capture_write;
use crate::builtins::numeric::num_to_f64;

#[cfg(unix)]
mod decoder;
mod fragments;
mod keyboard;
mod screen;
#[cfg(unix)]
mod tty;
// The thin crossterm seam: enter/leave the alternate screen, read keys, and
// paint a *frame* — a Brood vector of render ops. The protocol's meaning is
// data (the ops); these primitives are the in-process frontend that interprets
// it, so a remote frontend can implement the identical op vocabulary later.
// Errors surface as clean `LispError`s (never a crossterm panic), mirroring the
// rope primitives' discipline.

/// Map a crossterm I/O error into a runtime `LispError`.
pub(in crate::builtins) fn term_err(e: std::io::Error) -> LispError {
    LispError::runtime(format!("terminal I/O failed: {}", e))
}

/// `(term-enter)` — take over the terminal: raw mode + alternate screen, cursor
/// hidden. Pair with `term-leave`. The Rust-side `nest observe` guard also
/// restores the terminal if the program panics, so a crash never wrecks it.
pub(in crate::builtins) fn term_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Hide;
    use crossterm::event::{EnableFocusChange, EnableMouseCapture};
    use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
    enable_raw_mode().map_err(term_err)?;
    // Mouse capture rides with the full-screen path only (not the inline REPL
    // seam), so click/scroll reach `term-poll`. It costs terminal text-selection
    // while active — standard for a TUI, and only for the duration of the UI. Focus
    // reports (`CSI ?1004h`) too: the terminal says when it gains and loses the keyboard,
    // delivered as a window's `:focus-in` / `:blur` (ADR-536), off again on leave. And
    // bracketed paste (`CSI ?2004h`, ADR-586): a paste arrives as ONE `[:paste text]`, not as
    // keys an editor would indent, pair and complete one at a time.
    crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange,
        crossterm::event::EnableBracketedPaste,
        Hide
    )
    .map_err(term_err)?;
    // the alternate screen starts blank: the first frame is written whole
    invalidate_screen();
    // The kitty keyboard protocol, when the terminal speaks it: repeats and releases are
    // reported, so a held key stops with the hand (`keyboard`). Pushed on the alternate
    // screen (each screen has its own flag stack), only here — the inline REPL seam keeps
    // the legacy keyboard.
    keyboard::enhance();
    Ok(Value::nil())
}

/// `(term-leave)` — restore the terminal (show cursor, leave alternate screen,
/// disable raw mode). The normal-path teardown for `term-enter`.
pub(in crate::builtins) fn term_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::{DisableFocusChange, DisableMouseCapture};
    use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
    keyboard::restore();
    invalidate_screen();
    // Best effort, never an error: leaving is what every exit path does, and after a hang-up
    // the terminal is gone — a write there fails (EIO), and a leave that threw then turned a
    // clean exit into a crash a supervisor restarted on the dead terminal.
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::cursor::SetCursorStyle::DefaultUserShape,
        Show,
        DisableMouseCapture,
        DisableFocusChange,
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    Ok(Value::nil())
}

/// Best-effort terminal restore — the abnormal-path backstop a host binary holds
/// in an RAII guard so a panic or error during a full-screen UI (`nest observe`)
/// never leaves the terminal in raw mode / the alternate screen. Idempotent and
/// errors are swallowed (the normal path is the Brood `term-leave`).
pub fn restore_terminal() {
    use crossterm::cursor::Show;
    use crossterm::event::{DisableFocusChange, DisableMouseCapture};
    use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
    // before leaving the alternate screen, whose flag stack the push went on
    keyboard::restore();
    invalidate_screen();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::cursor::SetCursorStyle::DefaultUserShape,
        Show,
        DisableMouseCapture,
        DisableFocusChange,
        crossterm::event::DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// Lighter restore for the *inline* seam (the REPL line editor, `term-raw-enter`):
/// only leave raw mode. Unlike `restore_terminal` it writes no escape sequences —
/// `disable_raw_mode` is a termios ioctl — so a host binary can hold this in an
/// RAII guard around the REPL without polluting a piped (non-TTY) stdout on exit.
/// Idempotent; errors are swallowed (the normal path is the Brood `term-raw-leave`).
pub fn restore_raw() {
    let _ = crossterm::terminal::disable_raw_mode();
}

/// The terminal restore the binaries call on **every** exit path — normal
/// return, error report, and the broken-pipe exit in `print`. A program that
/// entered raw mode / the alternate screen (`term-raw-enter` / `term-enter`)
/// and then threw — or simply returned without a matching `term-raw-leave` —
/// would otherwise leave the shell wedged in raw mode (the hung-terminal bug).
///
/// It is gated on `is_raw_mode_enabled`, so it is a precise no-op whenever the
/// terminal was never left raw: that lets it sit on the common path (e.g. a
/// `nest test` run that never touched the terminal) without emitting a single
/// stray escape. When a restore *is* needed: on a TTY it does the full
/// [`restore_terminal`] (show cursor, leave the alternate screen and raw mode);
/// when stdout is piped/redirected it only leaves raw mode ([`restore_raw`], a
/// termios ioctl) so it never writes escape bytes into a captured/closed
/// stream. Idempotent.
pub fn restore_terminal_on_exit() {
    // Only act if the program actually left the terminal in raw mode. This is
    // what makes the call safe to drop onto the success path too, not just the
    // error/broken-pipe paths.
    if !matches!(crossterm::terminal::is_raw_mode_enabled(), Ok(true)) {
        return;
    }
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        restore_terminal();
    } else {
        restore_raw();
    }
}

/// `(term-size)` — the terminal size as `[cols rows]` (character cells).
pub(in crate::builtins) fn term_size(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (cols, rows) = crossterm::terminal::size().map_err(term_err)?;
    Ok(heap.alloc_vector2(Value::int(cols as i64), Value::int(rows as i64)))
}

/// `(term-poll ms)` — wait up to `ms` ms for a key; return it (a 1-char string,
/// or a keyword for specials), a mouse event, `[:key-up press]` when a key comes up
/// (reported under the kitty keyboard protocol, inferred from its repeats stopping
/// otherwise; `keyboard`), or `nil` on timeout. A held key's repeat already superseded in
/// the buffered input is dropped there. Always called with a
/// finite `ms`: the observer is the root process, so blocking here blocks only the
/// root thread (never a scheduler worker), but an *infinite* poll on a green process
/// would pin a worker (native blocking can't be preempted) — hence finite.
pub(in crate::builtins) fn term_poll(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let ms = expect_int(heap, "%term-poll", arg(args, 0))?.max(0) as u64;
    match keyboard::next(std::time::Duration::from_millis(ms)).map_err(term_err)? {
        Some(polled) => Ok(polled_value(heap, polled)),
        None => Ok(Value::nil()),
    }
}

/// One poll's yield as the app sees it — the same value `term-poll` answers and the input
/// forwarder delivers: a key, `[:key-up press]`, a `[:mouse …]`, `:focus-in` / `:blur` (a
/// window's spelling, so an app handles both frontends' focus alike), `[:resize cols rows]`
/// (a window's too) or `[:paste text]` — or nil for a mouse event or key it does not report.
fn polled_value(heap: &mut Heap, polled: keyboard::Polled) -> Value {
    match polled {
        keyboard::Polled::Key(keyboard::KeyDelivery::Press(event)) => key_to_value(heap, event),
        keyboard::Polled::Key(keyboard::KeyDelivery::Release(press)) => key_up_value(heap, press),
        keyboard::Polled::Mouse(mouse) => mouse_to_value(heap, mouse),
        keyboard::Polled::Focus(true) => value::kw("focus-in"),
        keyboard::Polled::Focus(false) => value::kw("blur"),
        keyboard::Polled::Resize(cols, rows) => {
            // what the terminal showed is gone with the old size: the next frame repaints it
            invalidate_screen();
            heap.alloc_vector(vec![
                value::kw("resize"),
                Value::int(cols as i64),
                Value::int(rows as i64),
            ])
        }
        keyboard::Polled::Paste(text) => {
            let text = heap.alloc_string(&text);
            heap.alloc_vector2(value::kw("paste"), text)
        }
    }
}

/// A release as the app sees it: `[:key-up press]`, `press` what `press_event` spelled
/// when it was delivered — nil for a key that spells nothing.
fn key_up_value(heap: &mut Heap, press_event: crossterm::event::KeyEvent) -> Value {
    match key_to_value(heap, press_event) {
        Value::Nil => Value::nil(),
        press => heap.alloc_vector2(value::kw("key-up"), press),
    }
}

/// `(term-held-key)` — the key held in the terminal now, as its press delivered it, or nil.
/// Reported under the kitty keyboard protocol, inferred from a run of repeats otherwise, and
/// nil outside `term-enter` (`keyboard`).
pub(in crate::builtins) fn term_held_key(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(match keyboard::held().map_err(term_err)? {
        Some(press) => key_to_value(heap, press),
        None => Value::nil(),
    })
}

/// Encode a Brood mouse event as the vector `[:mouse action button row col mods]`
/// — the shared shape both frontends yield, so the observer (and any future UI)
/// reads one form. `action` is a keyword, `button` a keyword or nil, `row`/`col`
/// 0-based cell coordinates, `mods` a vector of the held modifier keywords (in a
/// stable `[:ctrl :alt :shift]` order, `[]` when none) so an app can bind
/// Ctrl+wheel etc.
pub(in crate::builtins) fn mouse_value(
    heap: &mut Heap,
    action: &str,
    button: Option<&str>,
    row: u16,
    col: u16,
    mods: (bool, bool, bool),
    count: u8,
) -> Value {
    let btn = button.map(value::kw).unwrap_or(Value::nil());
    let (ctrl, alt, shift) = mods;
    let mut ms = Vec::new();
    if ctrl {
        ms.push(value::kw("ctrl"));
    }
    if alt {
        ms.push(value::kw("alt"));
    }
    if shift {
        ms.push(value::kw("shift"));
    }
    let ms = heap.alloc_vector(ms);
    let mut v = vec![
        value::kw("mouse"),
        value::kw(action),
        btn,
        Value::int(row as i64),
        Value::int(col as i64),
        ms,
    ];
    // A press carries its click-chain count as a trailing 7th element; other actions
    // (count 0) stay 6-element. The terminal can't detect multi-click, so it reports 1
    // for every press — keeping the GUI and terminal shapes identical.
    if count > 0 {
        v.push(Value::int(count as i64));
    }
    heap.alloc_vector(v)
}

/// Translate a crossterm mouse event into the shared `[:mouse …]` vector.
/// Press, release, drag, and vertical scroll are surfaced (the `:release`/`:drag`
/// vocabulary `gui::MouseAction` also produces per ADR-077). Only bare `Moved`
/// (motion with no button held) and horizontal scroll fall through to nil (a
/// no-op poll), so both frontends emit exactly the same set.
pub(in crate::builtins) fn mouse_to_value(
    heap: &mut Heap,
    m: crossterm::event::MouseEvent,
) -> Value {
    use crossterm::event::{KeyModifiers, MouseButton as CB, MouseEventKind as MK};
    let button = |b: CB| match b {
        CB::Left => "left",
        CB::Right => "right",
        CB::Middle => "middle",
    };
    let (action, btn, count) = match m.kind {
        // The terminal reports no click chain, so a press always counts as 1 (single);
        // the trailing count keeps the terminal vector shape identical to the GUI's.
        MK::Down(b) => ("press", Some(button(b)), 1),
        MK::Up(b) => ("release", Some(button(b)), 0),
        // Motion with a button held — a drag (e.g. resizing a divider, ADR-077).
        // Crossterm already reports this per-cell, matching the GUI's cell-granular
        // throttle. Bare `Moved` (no button) falls through to nil, as before.
        MK::Drag(b) => ("drag", Some(button(b)), 0),
        MK::ScrollUp => ("scroll-up", None, 0),
        MK::ScrollDown => ("scroll-down", None, 0),
        _ => return Value::nil(),
    };
    let mods = (
        m.modifiers.contains(KeyModifiers::CONTROL),
        m.modifiers.contains(KeyModifiers::ALT),
        m.modifiers.contains(KeyModifiers::SHIFT),
    );
    mouse_value(heap, action, btn, m.row, m.column, mods, count)
}

/// Encode a crossterm key event as a Brood value: a printable char becomes a
/// 1-char string; a control combo and the named special keys become keywords.
pub(in crate::builtins) fn key_to_value(heap: &mut Heap, k: crossterm::event::KeyEvent) -> Value {
    use crossterm::event::{KeyCode, KeyModifiers};
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    match k.code {
        // A chord — Ctrl, Alt/Meta (some terminals send it as an Esc prefix; crossterm
        // normalises that to ALT) or Super (reported only under the kitty keyboard
        // protocol) — spells exactly as the GUI's does: `crate::host::gui::character_chord`,
        // the one rule for both frontends (`:ctrl-x`, `:alt-X`, `:ctrl-meta-f`, `:super-x`).
        // A capital is shifted: under the kitty protocol's alternate keys the terminal sends
        // the shifted glyph and crossterm drops Shift (`X` for Alt+Shift+x), where a legacy
        // Esc-prefixed `X` comes with Shift set — the same chord either way.
        KeyCode::Char(c) if ctrl || alt || k.modifiers.contains(KeyModifiers::SUPER) => {
            gui_key_to_value(
                heap,
                crate::host::gui::character_chord(
                    c,
                    crate::host::gui::KeyModifiers {
                        ctrl,
                        alt,
                        shift: k.modifiers.contains(KeyModifiers::SHIFT) || c.is_uppercase(),
                        super_key: k.modifiers.contains(KeyModifiers::SUPER),
                    },
                ),
            )
        }
        KeyCode::Char(c) => heap.alloc_string(&c.to_string()),
        // The named keys carry their modifiers in the name — `crate::host::gui::named_key`,
        // the one rule the GUI frontend uses too: `:ctrl-left`, `:alt-shift-up`, … so an
        // Emacs `C-<left>` or a `C-S-<arrow>` window swap binds in a terminal as well.
        KeyCode::Up => named_kw("up", &k),
        KeyCode::Down => named_kw("down", &k),
        KeyCode::Left => named_kw("left", &k),
        KeyCode::Right => named_kw("right", &k),
        KeyCode::Enter => named_kw("enter", &k),
        KeyCode::Esc => value::kw("escape"),
        KeyCode::Backspace => named_kw("backspace", &k),
        KeyCode::Tab => value::kw("tab"),
        KeyCode::BackTab => value::kw("back-tab"),
        KeyCode::Delete => named_kw("delete", &k),
        KeyCode::Home => named_kw("home", &k),
        KeyCode::End => named_kw("end", &k),
        KeyCode::PageUp => named_kw("page-up", &k),
        KeyCode::PageDown => named_kw("page-down", &k),
        KeyCode::Insert => named_kw("insert", &k),
        KeyCode::F(number) => match crate::host::gui::function_key(number) {
            Some(name) => named_kw(name, &k),
            None => Value::nil(),
        },
        KeyCode::Menu => named_kw("menu", &k),
        KeyCode::Pause => named_kw("pause", &k),
        KeyCode::ScrollLock => named_kw("scroll-lock", &k),
        _ => Value::nil(),
    }
}

/// A named key's keyword under the event's modifiers (`named_key`'s spelling).
fn named_kw(base: &'static str, k: &crossterm::event::KeyEvent) -> Value {
    use crossterm::event::KeyModifiers;
    value::kw(crate::host::gui::named_key(
        base,
        crate::host::gui::KeyModifiers {
            ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
            alt: k.modifiers.contains(KeyModifiers::ALT),
            shift: k.modifiers.contains(KeyModifiers::SHIFT),
            super_key: k.modifiers.contains(KeyModifiers::SUPER),
        },
    ))
}

/// Write rendered terminal bytes (escape sequences) to stdout — unless an MCP
/// stdout-capture is active on this thread, in which case divert them into the
/// capture buffer instead. During a `nest mcp` `tools/call`, stdout *is* the
/// JSON-RPC channel, so a `term-draw` / `term-emit` writing raw escapes there would
/// corrupt the protocol and wedge the client (the `print` capture only catches
/// Brood `print`, not these direct crossterm writes). Diverting keeps the channel
/// pure and rides the rendered bytes back in the result envelope, so an agent can
/// still inspect what a frame produced. Mirrors `print`'s capture check.
pub(in crate::builtins) fn write_term_bytes(bytes: &[u8]) -> std::io::Result<()> {
    if !capture_write(&String::from_utf8_lossy(bytes)) {
        use std::io::Write;
        let mut real = std::io::stdout().lock();
        let mut rest = bytes;
        // stdout may be a NON-BLOCKING descriptor (a pipe a parent set so, a pty under load):
        // a full buffer is EAGAIN, a transient — wait until it drains, never fail the frame
        while !rest.is_empty() {
            match real.write(rest) {
                Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                Ok(written) => rest = &rest[written..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => wait_writable(),
                Err(e) => return Err(e),
            }
        }
        loop {
            match real.flush() {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => wait_writable(),
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

/// Block until stdout can take more bytes (or 100 ms pass): what a write that met EAGAIN on a
/// non-blocking stdout waits for before it tries again.
fn wait_writable() {
    #[cfg(unix)]
    {
        let mut descriptor = libc::pollfd {
            fd: libc::STDOUT_FILENO,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one valid pollfd, a bounded timeout.
        unsafe {
            libc::poll(&mut descriptor, 1, 100);
        }
    }
    #[cfg(not(unix))]
    std::thread::sleep(std::time::Duration::from_millis(10));
}

/// Parse a frame value (the op-vector `term-draw`/`term-emit`/`gui-draw` all
/// take) into `(tag, parts)` pairs: the frame must be a `Vector` (else a
/// `wrong_type` attributed to `who`); each op that is itself a `Vector` whose
/// first element is a `Keyword` yields `(that-keyword, the-op-parts)`; any op
/// that isn't a keyword-led vector is silently skipped (forward-compatible —
/// unknown ops are no-ops). This is the one extraction shared verbatim by the
/// three frame dispatchers; they deliberately *diverge downstream* (e.g. gui-draw
/// clamps coords at parse time, term-draw at use), so they must not drift on this
/// shared prologue — keep it here, in one place.
pub(in crate::builtins) fn frame_ops(
    heap: &Heap,
    frame: Value,
    who: &str,
    expected: &str,
) -> Result<Vec<(value::Symbol, Vec<Value>)>, LispError> {
    let ops: Vec<Value> = match frame {
        Value::Vector(id) => heap.vector(id).to_vec(),
        other => return Err(LispError::wrong_type(heap, who, expected, other)),
    };
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        let parts: Vec<Value> = match op {
            Value::Vector(id) => heap.vector(id).to_vec(),
            _ => continue,
        };
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => continue,
        };
        out.push((tag, parts));
    }
    Ok(out)
}

/// What the terminal shows now, as the last `term-draw` left it, and what its cursor is doing
/// (ADR-586) — so the next frame writes only the cells that changed. `None` until the first
/// frame after `term-enter` (and after `term-invalidate`, or a resize): that frame clears the
/// screen and writes every cell. One per OS process, as the terminal is.
struct TerminalScreen {
    shown: Option<screen::Screen>,
    cursor: screen::CursorState,
    /// The colour depth set by `term-colors`, overriding what `$COLORTERM` / `$TERM` say.
    depth: Option<screen::ColorDepth>,
    /// The default face `term-bg` set: what `[:clear]` fills with, and the colours of a face
    /// that names none — a window's background (`gui/bg`) for the terminal.
    default_face: screen::Style,
}

static TERMINAL_SCREEN: std::sync::Mutex<TerminalScreen> = std::sync::Mutex::new(TerminalScreen {
    shown: None,
    cursor: screen::CursorState {
        visible: false,
        shape: None,
    },
    depth: None,
    default_face: screen::Style {
        fg: None,
        bg: None,
        bold: false,
        italic: false,
        underline: screen::Underline::None,
        underline_colour: None,
        reverse: false,
    },
});

fn terminal_screen() -> std::sync::MutexGuard<'static, TerminalScreen> {
    TERMINAL_SCREEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Forget what the terminal shows, so the next frame repaints it whole: on entering and
/// leaving the full screen, on a resize, and when something else may have written to it.
///
/// `try_lock`, never `lock`: [`restore_terminal`] calls this from the panic path, which may
/// run on the very thread whose `term-draw` holds the lock — waiting there would hang the
/// crash. A screen it cannot take is one a draw is replacing anyway.
pub(in crate::builtins) fn invalidate_screen() {
    let mut state = match TERMINAL_SCREEN.try_lock() {
        Ok(state) => state,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    state.shown = None;
    // `term-enter` hid the cursor; so does every leave path's restore before the next enter
    state.cursor = screen::CursorState::default();
}

/// The colour depth frames are written at: the one `term-colors` set, else what the
/// environment says ([`screen::ColorDepth::detect`]).
fn color_depth(state: &TerminalScreen) -> screen::ColorDepth {
    state.depth.unwrap_or_else(|| {
        screen::ColorDepth::detect(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    })
}

/// A face map as the [`screen::Style`] its cells are drawn in — the same reading of `:fg`,
/// `:bg`, `:bold`, `:italic`, `:underline` (a flag, a colour, or `{:color c :style :wave}`)
/// and `:reverse` as [`apply_face`]. A non-map face is the terminal's default style.
fn style_of(heap: &Heap, face: Value) -> screen::Style {
    let Value::Map(id) = face else {
        return screen::Style::default();
    };
    let k = &*FACE_KEYS;
    let colour = |key| {
        heap.map_get(id, key)
            .and_then(|v| color_of(heap, v))
            .and_then(screen::Colour::of)
    };
    let (underline, underline_colour) = match heap
        .map_get(id, k.underline)
        .filter(|v| face_truthy(*v))
    {
        None => (screen::Underline::None, None),
        Some(Value::Map(spec)) => {
            let wave = matches!((heap.map_get(spec, k.style), k.wave), (Some(Value::Keyword(a)), Value::Keyword(b)) if a == b);
            (
                if wave {
                    screen::Underline::Curly
                } else {
                    screen::Underline::Straight
                },
                heap.map_get(spec, k.color)
                    .and_then(|v| color_of(heap, v))
                    .and_then(screen::Colour::of),
            )
        }
        Some(Value::Bool(_)) => (screen::Underline::Straight, None),
        Some(other) => (
            screen::Underline::Straight,
            color_of(heap, other).and_then(screen::Colour::of),
        ),
    };
    screen::Style {
        fg: colour(k.fg),
        bg: colour(k.bg),
        bold: heap.map_get(id, k.bold).is_some_and(face_truthy),
        italic: heap.map_get(id, k.italic).is_some_and(face_truthy),
        underline,
        underline_colour,
        reverse: heap.map_get(id, k.reverse).is_some_and(face_truthy),
    }
}

/// `(term-draw frame)` — paint a frame: a vector of op vectors `[:clear]`,
/// `[:text row col str]`, `[:text row col str face]`, `[:rect row col w h face]`,
/// `[:cursor row col]` / `[:cursor row col style]`, `[:scroll-region dy ops]` (its ops drawn
/// in place). Unknown ops are skipped (forward-compatible protocol). The frame is painted
/// over the screen the terminal shows ([`TERMINAL_SCREEN`]) and only the cells it changed
/// are written, in one flush (ADR-586). The cursor shows where the frame's last `[:cursor]`
/// puts it — a `:hollow` one (an unfocused pane's) only when there is no other — and is
/// hidden in a frame with none.
pub(in crate::builtins) fn term_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parsed = frame_ops(heap, arg(args, 0), "%term-draw", "vector (a frame)")?;
    let (cols, rows) = draw_size();
    let mut state = terminal_screen();
    let mut next = match &state.shown {
        Some(shown) if shown.cols == cols && shown.rows == rows => shown.clone(),
        _ => screen::Screen::new(cols, rows),
    };
    next.cursor = None;
    paint_ops(heap, parsed, &mut next, state.default_face)?;
    let depth = color_depth(&state);
    let mut out: Vec<u8> = Vec::new();
    let TerminalScreen { shown, cursor, .. } = &mut *state;
    screen::render(shown.as_ref(), &next, depth, cursor, &mut out);
    *shown = Some(next);
    drop(state);
    if !out.is_empty()
        && let Err(error) = write_term_bytes(&out)
    {
        // what the terminal shows is no longer known: the next frame repaints it whole,
        // rather than diffing against a screen this one never reached
        invalidate_screen();
        return Err(term_err(error));
    }
    Ok(Value::nil())
}

/// The size a frame is painted at: the terminal's, or — with no terminal to ask (no
/// controlling tty: a CI runner, `nest mcp`, output captured by `with-out-str`) — `$COLUMNS` ×
/// `$LINES`, else 80×24. A draw never fails for want of a terminal; it paints a screen of the
/// size a terminal would most likely have, as `term-draw` did before it diffed (ADR-586).
fn draw_size() -> (u16, u16) {
    if let Ok((cols, rows)) = crossterm::terminal::size()
        && cols > 0
        && rows > 0
    {
        return (cols, rows);
    }
    let from_environment = |name: &str, fallback: u16| {
        std::env::var(name)
            .ok()
            .and_then(|text| text.trim().parse::<u16>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(fallback)
    };
    (
        from_environment("COLUMNS", 80),
        from_environment("LINES", 24),
    )
}

/// Paint parsed frame ops onto `target` (see [`term_draw`]), a face's missing colours taken
/// from `default_face` (`term-bg`).
fn paint_ops(
    heap: &Heap,
    parsed: Vec<(value::Symbol, Vec<Value>)>,
    target: &mut screen::Screen,
    default_face: screen::Style,
) -> Result<(), LispError> {
    use crate::host::gui::CursorStyle;
    let clear_t = value::intern("clear");
    let text_t = value::intern("text");
    let cursor_t = value::intern("cursor");
    let rect_t = value::intern("rect");
    let scroll_region_t = value::intern("scroll-region");
    // a scroll region's ops are drawn in place, in order: a deque they are prepended to
    let mut queue: std::collections::VecDeque<(value::Symbol, Vec<Value>)> = parsed.into();
    let mut hollow: Option<screen::Cursor> = None;
    let mut solid: Option<screen::Cursor> = None;
    while let Some((tag, parts)) = queue.pop_front() {
        if tag == clear_t {
            target.clear(default_face);
        } else if tag == rect_t {
            let row = expect_int(heap, "%term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "%term-draw", arg(&parts, 2))?;
            // `w`/`h` clamped as they always were (MAX_EXTENT); the screen clips the rest
            let width = clamp_extent(expect_int(heap, "%term-draw", arg(&parts, 3))?) as i64;
            let height = clamp_extent(expect_int(heap, "%term-draw", arg(&parts, 4))?) as i64;
            let face = parts.get(5).copied().unwrap_or(Value::nil());
            target.fill(
                row,
                col,
                width,
                height,
                style_of(heap, face).over(default_face),
            );
        } else if tag == cursor_t {
            let row = expect_int(heap, "%term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "%term-draw", arg(&parts, 2))?;
            if row < 0 || col < 0 || row >= target.rows as i64 || col >= target.cols as i64 {
                continue;
            }
            let style = cursor_style_from(parts.get(3).copied().unwrap_or(Value::nil()));
            let at = screen::Cursor {
                row: row as u16,
                col: col as u16,
                shape: match style {
                    CursorStyle::Bar => screen::CursorShape::Bar,
                    CursorStyle::Underline => screen::CursorShape::Underline,
                    CursorStyle::Block | CursorStyle::Hollow => screen::CursorShape::Block,
                },
            };
            if matches!(style, CursorStyle::Hollow) {
                hollow = Some(at);
            } else {
                solid = Some(at);
            }
        } else if tag == text_t {
            let row = expect_int(heap, "%term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "%term-draw", arg(&parts, 2))?;
            let text = expect_string(heap, "%term-draw", arg(&parts, 3))?;
            let face = parts.get(4).copied().unwrap_or(Value::nil());
            target.put_text(row, col, &text, style_of(heap, face).over(default_face));
        } else if tag == scroll_region_t {
            // a terminal can't do sub-cell offsets: the inner ops are drawn where they are
            let inner_val = arg(&parts, 2);
            if let Ok(inner_parsed) = frame_ops(heap, inner_val, "%term-draw", "scroll-region ops")
            {
                for item in inner_parsed.into_iter().rev() {
                    queue.push_front(item);
                }
            }
        }
    }
    target.cursor = solid.or(hollow);
    Ok(())
}

/// `(term-invalidate)` — forget what the terminal shows: the next `term-draw` clears it and
/// writes every cell. For when something other than `term-draw` wrote to the screen.
pub(in crate::builtins) fn term_invalidate(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    invalidate_screen();
    Ok(Value::nil())
}

/// Whether a shell can continue this process if it stops: job control is there. A process
/// group is ORPHANED — nobody left to `fg` it — when its parent is not in another group of the
/// same session: `gnome-terminal -- bedit -nw`, `ssh host bedit -nw`, `exec bedit -nw` all make
/// the editor its own session's leader. The kernel drops a SIGTSTP sent to an orphaned group
/// for exactly this reason; this asks first, so a suspend that cannot happen is said.
#[cfg(unix)]
pub(in crate::builtins) fn job_control_available() -> bool {
    // SAFETY: these only read process ids.
    unsafe {
        let group = libc::getpgrp();
        let parent = libc::getppid();
        let parent_group = libc::getpgid(parent);
        parent_group >= 0
            && parent_group != group
            && libc::getsid(parent) == libc::getsid(0)
            && terminal_foreground_group() == Some(group)
    }
}

/// The foreground process group of the terminal the keyboard is read from: stdin when it is a
/// terminal, else `/dev/tty` — as `tty.rs` chooses — so a program whose stdin is a pipe still
/// asks the terminal it actually reads keys from.
#[cfg(unix)]
fn terminal_foreground_group() -> Option<libc::pid_t> {
    // SAFETY: isatty / open / tcgetpgrp / close on descriptors this function owns or stdin.
    unsafe {
        if libc::isatty(libc::STDIN_FILENO) == 1 {
            let group = libc::tcgetpgrp(libc::STDIN_FILENO);
            return (group >= 0).then_some(group);
        }
        let descriptor = libc::open(
            c"/dev/tty".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOCTTY,
        );
        if descriptor < 0 {
            return None;
        }
        let group = libc::tcgetpgrp(descriptor);
        libc::close(descriptor);
        (group >= 0).then_some(group)
    }
}

/// `(term-suspend)` — give the terminal back and stop this process, as a shell's C-z stops a
/// job (Emacs's `suspend-frame` in a terminal): the screen left (`term-leave`), SIGTSTP raised
/// with its default action — the shell takes over until `fg` — and on continuing the screen
/// taken again (`term-enter`), to be repainted whole. `:suspended`; or `:no-job-control`, the
/// terminal kept, when nothing could continue a stopped editor ([`job_control_available`]):
/// stopped there, it would stay frozen with its unsaved text. Unix only. (ADR-586)
pub(in crate::builtins) fn term_suspend(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    #[cfg(unix)]
    {
        if !job_control_available() {
            return Ok(value::kw("no-job-control"));
        }
        term_leave(args, env, heap)?;
        // SIGTSTP with its DEFAULT action (a trap may have taken it): the kernel's own
        // orphaned-group rule then applies too, where SIGSTOP would stop unconditionally.
        // SAFETY: sigaction with a zeroed struct naming SIG_DFL, restored after.
        unsafe {
            let mut default: libc::sigaction = std::mem::zeroed();
            default.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut default.sa_mask);
            let mut previous: libc::sigaction = std::mem::zeroed();
            libc::sigaction(libc::SIGTSTP, &default, &mut previous);
            libc::raise(libc::SIGTSTP);
            libc::sigaction(libc::SIGTSTP, &previous, std::ptr::null_mut());
        }
        term_enter(args, env, heap)?;
        Ok(value::kw("suspended"))
    }
    #[cfg(not(unix))]
    {
        let _ = (args, env, heap);
        Err(LispError::runtime(
            "term/suspend: this platform has no job control",
        ))
    }
}

/// `(term-refresh)` — after this process was stopped and continued by someone else
/// (SIGSTOP, SIGCONT): raw mode back (a shell resets the tty when a job stops) and the next
/// frame painted whole. Returns nil. (ADR-586)
pub(in crate::builtins) fn term_refresh(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    crossterm::terminal::enable_raw_mode().map_err(term_err)?;
    invalidate_screen();
    Ok(Value::nil())
}

/// `(term-repaint-due)` — whether the next frame must be painted whole though the app's model
/// did not change: the screen was forgotten (a resize, a continue, `term-invalidate`), so a
/// loop that skips drawing an unchanged model must draw this time. (ADR-586)
pub(in crate::builtins) fn term_repaint_due(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(terminal_screen().shown.is_none()))
}

/// `(term-bg bg fg)` — the terminal's default face: `[:clear]` fills with `bg`, and a cell whose
/// face names no `:bg` / `:fg` is drawn in `bg` / `fg` — the terminal's `gui/bg`, so a theme
/// designed for a dark window is not drawn on a light terminal's own background. nil for
/// either is the terminal's own. The next frame repaints whole.
pub(in crate::builtins) fn term_bg(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let colour = |v: Value| -> Result<Option<screen::Colour>, LispError> {
        match v {
            Value::Nil => Ok(None),
            other => color_of(heap, other)
                .and_then(screen::Colour::of)
                .map(Some)
                .ok_or_else(|| LispError::wrong_type(heap, "term/bg", "a colour or nil", other)),
        }
    };
    let bg = colour(arg(args, 0))?;
    let fg = colour(arg(args, 1))?;
    let mut state = terminal_screen();
    state.default_face = screen::Style {
        fg,
        bg,
        ..screen::Style::default()
    };
    state.shown = None;
    Ok(Value::nil())
}

/// `(term-colors)` — the colour depth frames are written at, `:truecolor`, `:256` or `:16`;
/// `(term-colors depth)` sets it (nil goes back to what `$COLORTERM` / `$TERM` say) and
/// repaints the next frame whole.
pub(in crate::builtins) fn term_colors(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut state = terminal_screen();
    if !args.is_empty() {
        state.depth = match arg(args, 0) {
            Value::Nil => None,
            Value::Keyword(name) => Some(
                screen::ColorDepth::named(value::symbol_name(name).as_str()).ok_or_else(|| {
                    LispError::runtime(format!(
                        "term/colors: not a colour depth: :{} (:truecolor, :256 or :16)",
                        value::symbol_name(name)
                    ))
                })?,
            ),
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "term/colors",
                    "a colour depth keyword or nil",
                    other,
                ));
            }
        };
        state.shown = None;
    }
    Ok(value::kw(color_depth(&state).name()))
}

/// `(term-raw-enter)` — raw mode only: no alternate screen, the cursor stays
/// visible, scrollback is preserved. The seam for an *inline* line editor (the
/// self-hosted REPL, std/editor/lineedit.blsp), as opposed to `term-enter` which takes
/// over the whole screen for a full-screen TUI. Pair with `term-raw-leave`.
///
/// Defensively *shows the cursor and disables mouse capture* on entry: terminal
/// state persists across processes, so a prior full-screen app (`term-enter` hides
/// the cursor + captures the mouse) that exited without restoring — a crash, a
/// hard `Ctrl-C`, a killed observer — would otherwise leave the inline editor with
/// no cursor and mouse-movement escape sequences injected as input. The inline
/// editor only runs on a TTY (the REPL gates `lineedit-read` on stdin+stdout being
/// terminals; the piped path uses `read-line`), so these escapes never reach a
/// redirected stream. Idempotent: showing a visible cursor / disabling inactive
/// capture are no-ops.
pub(in crate::builtins) fn term_raw_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::{DisableFocusChange, DisableMouseCapture};
    crossterm::terminal::enable_raw_mode().map_err(term_err)?;
    // focus reports off too: a full-screen app that died without `term-leave` left them on,
    // and the line editor has no use for them (its reader drops any that still arrive)
    crossterm::execute!(
        std::io::stdout(),
        Show,
        DisableMouseCapture,
        DisableFocusChange
    )
    .map_err(term_err)?;
    Ok(Value::nil())
}

/// `(term-raw-leave)` — leave raw mode (the teardown for `term-raw-enter`).
/// Idempotent with the panic-path `restore_terminal`.
pub(in crate::builtins) fn term_raw_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    crossterm::terminal::disable_raw_mode().map_err(term_err)?;
    Ok(Value::nil())
}

/// `(term-emit ops)` — inline, relative-motion rendering for an in-place editor
/// that must not take over the screen (unlike `term-draw`, which paints absolute
/// cells on the alternate screen). Interprets a vector of op vectors, queued then
/// flushed once so a repaint doesn't tear:
///   `[:print str]` / `[:print str face]`  print at the cursor (face via apply_face)
///   `[:cr]`                                carriage return to column 0
///   `[:nl]`                                newline (`"\r\n"`)
///   `[:up n]` / `[:down n]`                move the cursor n rows
///   `[:col n]`                             move to absolute column n (0-based)
///   `[:clear-eol]`                         clear from the cursor to end of line
///   `[:clear-below]`                       clear from the cursor to end of screen
///   `[:clear-screen]`                      clear the whole screen, cursor to (0,0)
/// Unknown ops are skipped (forward-compatible, like `term-draw`).
pub(in crate::builtins) fn term_emit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::cursor::{MoveDown, MoveToColumn, MoveUp};
    use crossterm::style::{Attribute, Print, ResetColor, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};

    let parsed = frame_ops(heap, arg(args, 0), "%term-emit", "vector (ops)")?;
    let print_t = value::intern("print");
    let cr_t = value::intern("cr");
    let nl_t = value::intern("nl");
    let up_t = value::intern("up");
    let down_t = value::intern("down");
    let col_t = value::intern("col");
    let clear_eol_t = value::intern("clear-eol");
    let clear_below_t = value::intern("clear-below");
    let clear_screen_t = value::intern("clear-screen");
    let mut out: Vec<u8> = Vec::new();
    for (tag, parts) in parsed {
        if tag == print_t {
            let s = expect_string(heap, "%term-emit", arg(&parts, 1))?;
            apply_face(
                &mut out,
                heap,
                parts.get(2).copied().unwrap_or(Value::nil()),
            )?;
            crossterm::queue!(out, Print(s), SetAttribute(Attribute::Reset), ResetColor)
                .map_err(term_err)?;
        } else if tag == cr_t {
            crossterm::queue!(out, MoveToColumn(0)).map_err(term_err)?;
        } else if tag == nl_t {
            crossterm::queue!(out, Print("\r\n")).map_err(term_err)?;
        } else if tag == up_t {
            let n = expect_int(heap, "%term-emit", arg(&parts, 1))?;
            if n > 0 {
                crossterm::queue!(out, MoveUp(clamp_u16(n))).map_err(term_err)?;
            }
        } else if tag == down_t {
            let n = expect_int(heap, "%term-emit", arg(&parts, 1))?;
            if n > 0 {
                crossterm::queue!(out, MoveDown(clamp_u16(n))).map_err(term_err)?;
            }
        } else if tag == col_t {
            let n = expect_int(heap, "%term-emit", arg(&parts, 1))?;
            crossterm::queue!(out, MoveToColumn(clamp_u16(n))).map_err(term_err)?;
        } else if tag == clear_eol_t {
            crossterm::queue!(out, Clear(ClearType::UntilNewLine)).map_err(term_err)?;
        } else if tag == clear_below_t {
            crossterm::queue!(out, Clear(ClearType::FromCursorDown)).map_err(term_err)?;
        } else if tag == clear_screen_t {
            crossterm::queue!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))
                .map_err(term_err)?;
        }
    }
    write_term_bytes(&out).map_err(term_err)?;
    Ok(Value::nil())
}

/// The face-map keys (`:fg`/`:bg`/`:bold`/…) interned once for the whole process,
/// not re-interned per text op per frame on the render path — the same pre-intern
/// the frame dispatchers do for their op tags. Keyword interning is global and
/// append-only, so these stay valid for the process's life.
struct FaceKeys {
    fg: Value,
    bg: Value,
    bold: Value,
    italic: Value,
    underline: Value,
    reverse: Value,
    family: Value,
    scale: Value,
    color: Value,
    style: Value,
    wave: Value,
}
static FACE_KEYS: std::sync::LazyLock<FaceKeys> = std::sync::LazyLock::new(|| FaceKeys {
    fg: value::kw("fg"),
    bg: value::kw("bg"),
    bold: value::kw("bold"),
    italic: value::kw("italic"),
    underline: value::kw("underline"),
    reverse: value::kw("reverse"),
    family: value::kw("family"),
    scale: value::kw("scale"),
    color: value::kw("color"),
    style: value::kw("style"),
    wave: value::kw("wave"),
});

/// Apply a face map (`{:fg :red :bg :blue :bold true :reverse true}`) as
/// crossterm style commands. A non-map (or nil) face is a no-op. Unknown colour
/// names are skipped. Callers reset attributes after the text.
pub(in crate::builtins) fn apply_face<W: std::io::Write>(
    out: &mut W,
    heap: &Heap,
    face: Value,
) -> Result<(), LispError> {
    use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
    let Value::Map(id) = face else { return Ok(()) };
    let k = &*FACE_KEYS;
    if let Some(fg) = heap.map_get(id, k.fg).and_then(|v| color_of(heap, v)) {
        crossterm::queue!(out, SetForegroundColor(fg)).map_err(term_err)?;
    }
    if let Some(bg) = heap.map_get(id, k.bg).and_then(|v| color_of(heap, v)) {
        crossterm::queue!(out, SetBackgroundColor(bg)).map_err(term_err)?;
    }
    if heap.map_get(id, k.bold).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Bold)).map_err(term_err)?;
    }
    if heap.map_get(id, k.italic).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Italic)).map_err(term_err)?;
    }
    if let Some(underline) = heap.map_get(id, k.underline).filter(|v| face_truthy(*v)) {
        // a colour (`"#ff5f59"`) or `{:color c :style :wave}`: a terminal that knows the
        // extended underline (kitty, foot, wezterm) draws it coloured and curly, one that
        // does not falls back to a plain underline
        let (colour, wave) = match underline {
            Value::Map(spec) => (
                heap.map_get(spec, k.color).and_then(|v| color_of(heap, v)),
                matches!((heap.map_get(spec, k.style), k.wave), (Some(Value::Keyword(a)), Value::Keyword(b)) if a == b),
            ),
            Value::Bool(_) => (None, false),
            other => (color_of(heap, other), false),
        };
        let style = if wave {
            Attribute::Undercurled
        } else {
            Attribute::Underlined
        };
        crossterm::queue!(out, SetAttribute(style)).map_err(term_err)?;
        if let Some(colour) = colour {
            crossterm::queue!(out, crossterm::style::SetUnderlineColor(colour))
                .map_err(term_err)?;
        }
    }
    if heap.map_get(id, k.reverse).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Reverse)).map_err(term_err)?;
    }
    Ok(())
}

/// Brood truthiness for a face flag: only `nil`/`false` are falsy.
pub(in crate::builtins) fn face_truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// A face colour value to a crossterm `Color`. A palette keyword (`:red`,
/// `:dark-grey`, …) maps to the terminal's *named* colour, so it honours the
/// user's terminal theme; an explicit `[r g b]` vector or `"#rrggbb"` string maps
/// to a true-colour cell (`Color::Rgb`) — the same RGB the GUI frontend paints, so
/// a curated palette renders identically in both.
pub(in crate::builtins) fn color_of(heap: &Heap, v: Value) -> Option<crossterm::style::Color> {
    use crossterm::style::Color;
    if let Value::Keyword(s) = v {
        return Some(match value::symbol_name(s).as_str() {
            "black" => Color::Black,
            "red" => Color::Red,
            "green" => Color::Green,
            "yellow" => Color::Yellow,
            "blue" => Color::Blue,
            "magenta" => Color::Magenta,
            "cyan" => Color::Cyan,
            "white" => Color::White,
            "grey" | "gray" => Color::Grey,
            "dark-grey" | "dark-gray" => Color::DarkGrey,
            _ => return None,
        });
    }
    face_rgb(heap, v).map(|[r, g, b]| Color::Rgb { r, g, b })
}

/// Clamp a Brood int to a terminal coordinate (crossterm uses `u16`).
///
/// The ceiling is `u16::MAX - 1`, not `u16::MAX`: crossterm's `MoveTo` writes
/// **1-based** ANSI coordinates, so it does `self.0 + 1` on the `u16` we hand it and
/// `65535` panics there with "attempt to add with overflow" under debug-assertions —
/// on a scheduler worker, where no `try` can catch it. One unreachable cell column is
/// not worth a reachable panic.
pub(in crate::builtins) fn clamp_u16(n: i64) -> u16 {
    n.clamp(0, u16::MAX as i64 - 1) as u16
}

/// The largest render *extent* (a `w`/`h` in cells) a frame op may ask for.
///
/// Coordinates go through `clamp_u16`, but an extent needs its own bound and for a
/// different reason: `w` sizes an *allocation* and `h` sizes a *loop*, so an
/// unbounded `i64` straight out of a frame is an allocator abort (SIGABRT, no crash
/// dump, uncatchable by Brood `try`) rather than a wrong pixel. `u16::MAX` on both
/// at once would still be a 4 GB fill, so the cap is the render bound instead: no
/// terminal is 4096 cells wide or tall, and nothing beyond the terminal renders, so
/// clamping here costs nothing real and turns a hostile frame into a no-op.
const MAX_EXTENT: i64 = 4096;

/// Clamp a Brood int to a drawable extent — see [`MAX_EXTENT`].
pub(in crate::builtins) fn clamp_extent(n: i64) -> u16 {
    n.clamp(0, MAX_EXTENT) as u16
}

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

/// A face colour keyword (`:red`, `:dark-grey`, …) to an RGB triple for the GUI
/// framebuffer. The same palette `color_of` maps to crossterm `Color`s, so the
/// two frontends agree on what `:red` looks like.
pub(in crate::builtins) fn color_rgb(v: Value) -> Option<[u8; 3]> {
    let Value::Keyword(s) = v else { return None };
    Some(match value::symbol_name(s).as_str() {
        "black" => [0x00, 0x00, 0x00],
        "red" => [0xcd, 0x31, 0x31],
        "green" => [0x0d, 0xbc, 0x79],
        "yellow" => [0xe5, 0xe5, 0x10],
        "blue" => [0x24, 0x72, 0xc8],
        "magenta" => [0xbc, 0x3f, 0xbc],
        "cyan" => [0x11, 0xa8, 0xcd],
        "white" => [0xe5, 0xe5, 0xe5],
        "grey" | "gray" => [0x80, 0x80, 0x80],
        "dark-grey" | "dark-gray" => [0x50, 0x50, 0x50],
        _ => return None,
    })
}

/// Resolve a face colour VALUE to an RGB triple — the one place every frontend
/// agrees on what a colour means. Accepts a palette keyword (`:red`, via
/// `color_rgb`), an explicit `[r g b]` vector (each channel clamped to 0..255), or
/// a `"#rgb"` / `"#rrggbb"` hex string. Anything else is `None` (the default face).
/// This is what lets a UI curate a soft RGB palette instead of the harsh
/// ANSI-16 keywords — and the `:vspans` fast path shares it too.
pub(in crate::builtins) fn face_rgb(heap: &Heap, v: Value) -> Option<[u8; 3]> {
    match v {
        Value::Keyword(_) => color_rgb(v),
        Value::Vector(id) => {
            let xs = heap.vector(id);
            if xs.len() == 3 {
                let chan = |k: usize| match xs[k] {
                    Value::Int(n) => n.clamp(0, 255) as u8,
                    _ => 0,
                };
                Some([chan(0), chan(1), chan(2)])
            } else {
                None
            }
        }
        Value::Str(id) => parse_hex_color(heap.string(id)),
        _ => None,
    }
}

/// Parse a `"#rgb"` or `"#rrggbb"` hex colour to an RGB triple. `None` for any
/// other shape (no leading `#`, a bad length, or a non-hex digit). The 3-digit
/// shorthand expands each nibble (`#f0a` → `[0xff 0x00 0xaa]`).
pub(in crate::builtins) fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    let b = h.as_bytes();
    match h.len() {
        3 => {
            let d = |i: usize| (b[i] as char).to_digit(16).map(|n| (n * 17) as u8);
            Some([d(0)?, d(1)?, d(2)?])
        }
        6 => {
            // Index the bytes (like the 3-nibble case) rather than `&h[i..i+2]`: a
            // 6-*byte* `h` can hold a multi-byte char (`#a€bc`), and slicing at a
            // non-char-boundary panics. A non-ASCII byte just fails `to_digit` → `None`.
            let d = |i: usize| (b[i] as char).to_digit(16);
            let p = |i: usize| Some((d(i)? * 16 + d(i + 1)?) as u8);
            Some([p(0)?, p(2)?, p(4)?])
        }
        _ => None,
    }
}

/// Resolve a face map (`{:fg :red :bg :blue :bold true :reverse true}`) into the
/// plain `gui::Face` the backend renders. A non-map face is the default face.
pub(in crate::builtins) fn gui_face(heap: &Heap, face: Value) -> crate::host::gui::Face {
    let mut f = crate::host::gui::Face::default();
    let Value::Map(id) = face else { return f };
    let k = &*FACE_KEYS;
    f.fg = heap.map_get(id, k.fg).and_then(|v| face_rgb(heap, v));
    f.bg = heap.map_get(id, k.bg).and_then(|v| face_rgb(heap, v));
    f.bold = heap.map_get(id, k.bold).is_some_and(face_truthy);
    f.italic = heap.map_get(id, k.italic).is_some_and(face_truthy);
    // `:underline` is a flag, a colour (`"#ff5f59"`, `:red`), or `{:color c :style :wave}`
    let underline = heap.map_get(id, k.underline);
    f.underline = underline.is_some_and(face_truthy);
    match underline {
        Some(Value::Map(spec)) => {
            f.underline_rgb = heap.map_get(spec, k.color).and_then(|v| face_rgb(heap, v));
            f.underline_wave = matches!((heap.map_get(spec, k.style), k.wave), (Some(Value::Keyword(a)), Value::Keyword(b)) if a == b);
        }
        Some(Value::Bool(_)) | Some(Value::Nil) | None => {}
        Some(colour) => f.underline_rgb = face_rgb(heap, colour),
    }
    f.reverse = heap.map_get(id, k.reverse).is_some_and(face_truthy);
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

/// `(audio-beep freq-hz ms [vol])` — play a short tone of `freq-hz` for `ms`
/// milliseconds at peak amplitude `vol` (0..1, default ~0.18). Fire-and-forget
/// (never blocks); overlapping beeps mix. A no-op without `--features audio`,
/// when there's no audio device, or when muted (`BROOD_AUDIO=0` /
/// `BROOD_GUI_HEADLESS`). Returns nil.
pub(in crate::builtins) fn audio_beep(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let freq = num_to_f64(heap, "audio/beep", arg(args, 0))?;
    let ms = num_to_f64(heap, "audio/beep", arg(args, 1))?;
    // Optional 3rd arg is peak amplitude; 0.0 (also the default) means "use the
    // backend's default volume".
    let vol = if args.len() >= 3 {
        num_to_f64(heap, "audio/beep", arg(args, 2))? as f32
    } else {
        0.0
    };
    crate::host::audio::beep(freq as f32, ms.max(0.0) as u64, vol);
    Ok(Value::Nil)
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

/// A held `gui::Key` as the same Brood value `gui-open` delivers for that press —
/// a 1-char string, else a `:ctrl-…` / `:alt-…` / `:ctrl-meta-…` / named keyword —
/// so an app can compare `(gui-held-key id)` directly against the key it last saw.
pub(in crate::builtins) fn gui_key_to_value(heap: &mut Heap, k: crate::host::gui::Key) -> Value {
    use crate::host::gui::Key;
    match k {
        Key::Char(c) => heap.alloc_string(&c.to_string()),
        Key::Named(s) => Value::keyword(value::intern(s)),
    }
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
/// :origin [x y] :inset px}` — `gui/image-thumb`'s `{:width :height :rgba}` plus the
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
    let pairs = vec![
        (value::kw("width"), Value::int(image.width as i64)),
        (value::kw("height"), Value::int(image.height as i64)),
        (value::kw("rgba"), rgba),
        (value::kw("cell"), cell),
        (value::kw("grid"), grid),
        (value::kw("origin"), origin),
        (value::kw("inset"), Value::int(image.inset as i64)),
    ];
    heap.map_from_pairs(pairs)
}

/// The window a `%gui-render-frame` spec map describes: `:width`/`:height` (physical
/// px), `:font` (an installed family's name, or a family keyword), `:size` (the font px),
/// `:line-height`, `:inset`, `:bg` (a colour), `:scale`, `:text-aa` and `:ligatures`.
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

/// The cursor style from a `[:cursor row col style]` op's optional 4th element: a
/// `:bar` / `:underline` / `:hollow` keyword, else (`:block`, nil, or anything unknown) the
/// default `Block`. Shared by both frontends so the caret shape agrees.
pub(in crate::builtins) fn cursor_style_from(v: Value) -> crate::host::gui::CursorStyle {
    use crate::host::gui::CursorStyle;
    match v {
        Value::Keyword(s) => match value::symbol_name(s).as_str() {
            "bar" => CursorStyle::Bar,
            "underline" => CursorStyle::Underline,
            "hollow" => CursorStyle::Hollow,
            _ => CursorStyle::Block,
        },
        _ => CursorStyle::Block,
    }
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

/// The terminal's input forwarder (ADR-491): a thread that reads the keyboard and the mouse
/// and delivers each event to one process's PRIORITY lane, as a window delivers its input —
/// so a key typed while that process is busy is queued where the kernel's quit key can see
/// it, rather than waiting in the tty until the busy loop next polls. One per OS process
/// (one terminal). The stop flag is checked between polls of at most [`FORWARD_POLL`].
struct Forwarder {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

static FORWARDER: std::sync::Mutex<Option<Forwarder>> = std::sync::Mutex::new(None);

/// How long the forwarder waits for input before it looks at its stop flag again: what
/// stopping it can cost.
const FORWARD_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// `(term/forward-input pid)` — deliver every terminal input event to `pid`'s priority lane
/// from now on (replacing any earlier target); `(term/forward-input nil)` stops, and waits
/// for the reader thread to finish, so `term/poll` reads the keyboard again. See
/// [`Forwarder`].
pub(in crate::builtins) fn term_forward_input(
    args: &[Value],
    _: EnvId,
    heap: &mut Heap,
) -> LispResult {
    let target = match arg(args, 0) {
        Value::Nil => None,
        Value::Pid { node, id } if crate::dist::is_local(node) => Some(id),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "term/forward-input",
                "a local pid or nil",
                other,
            ));
        }
    };
    let mut slot = FORWARDER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(previous) = slot.take() {
        previous
            .stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = previous.thread.join();
    }
    let Some(pid) = target else {
        return Ok(Value::nil());
    };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = std::sync::Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("brood-term-input".into())
        .spawn(move || forward_input(pid, &thread_stop))
        .map_err(|e| LispError::runtime(format!("term/forward-input: {e}")))?;
    *slot = Some(Forwarder { stop, thread });
    Ok(Value::nil())
}

/// The forwarder's loop: each event built as `term/poll` answers it (in a scratch heap of
/// the thread's own) and delivered on `pid`'s priority lane, until stopped, the terminal
/// fails, or `pid` is gone.
fn forward_input(pid: u64, stop: &std::sync::atomic::AtomicBool) {
    let mut heap = Heap::new();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        if !crate::process::is_alive(pid) {
            return;
        }
        let event = match keyboard::next(FORWARD_POLL) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(_) => return,
        };
        let value = polled_value(&mut heap, event);
        if matches!(value, Value::Nil) {
            continue;
        }
        if let Ok(message) = crate::process::to_message(&heap, value) {
            crate::process::deliver_priority(pid, message);
        }
        // the scratch heap only ever holds the event in flight
        heap = Heap::new();
    }
}
#[cfg(test)]
mod focus_tests {
    use super::{expect_string, keyboard, polled_value};
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    // A focus report reads as a window's focus event, so an app (bedit's clipboard read on
    // focus) handles the terminal and the window alike; so do a resize and a paste.
    fn keyword_name(value: Value) -> Option<String> {
        match value {
            Value::Keyword(symbol) => Some(value::symbol_name_ref(symbol).to_string()),
            _ => None,
        }
    }

    #[test]
    fn a_focus_report_reads_as_a_windows_focus_event() {
        let mut heap = Heap::new();
        assert_eq!(
            keyword_name(polled_value(&mut heap, keyboard::Polled::Focus(true))).as_deref(),
            Some("focus-in")
        );
        assert_eq!(
            keyword_name(polled_value(&mut heap, keyboard::Polled::Focus(false))).as_deref(),
            Some("blur")
        );
    }

    #[test]
    fn a_resize_and_a_paste_read_as_a_windows_events() {
        let mut heap = Heap::new();
        let resize = polled_value(&mut heap, keyboard::Polled::Resize(100, 30));
        let Value::Vector(id) = resize else {
            panic!("expected [:resize cols rows], got {resize:?}");
        };
        let parts = heap.vector(id).to_vec();
        assert_eq!(keyword_name(parts[0]).as_deref(), Some("resize"));
        assert!(matches!(parts[1], Value::Int(100)));
        assert!(matches!(parts[2], Value::Int(30)));
        let paste = polled_value(&mut heap, keyboard::Polled::Paste("a\nb".to_string()));
        let Value::Vector(id) = paste else {
            panic!("expected [:paste text], got {paste:?}");
        };
        let parts = heap.vector(id).to_vec();
        assert_eq!(keyword_name(parts[0]).as_deref(), Some("paste"));
        assert_eq!(
            expect_string(&heap, "%term-poll", parts[1]).unwrap(),
            "a\nb".to_string()
        );
    }
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
        use super::parse_hex_color;
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
mod color_value_tests {
    use super::{face_rgb, parse_hex_color};
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    #[test]
    fn parses_six_and_three_digit_hex() {
        assert_eq!(parse_hex_color("#61afef"), Some([0x61, 0xaf, 0xef]));
        assert_eq!(parse_hex_color("#f0a"), Some([0xff, 0x00, 0xaa])); // nibble doubling
        assert_eq!(parse_hex_color("#000000"), Some([0, 0, 0]));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(parse_hex_color("61afef"), None); // no leading #
        assert_eq!(parse_hex_color("#12g456"), None); // non-hex digit
        assert_eq!(parse_hex_color("#1234"), None); // bad length
        assert_eq!(parse_hex_color("#"), None);
    }

    #[test]
    fn face_rgb_spans_keyword_vector_and_hex() {
        let mut heap = Heap::new();
        // a palette keyword still resolves via the shared path
        assert_eq!(face_rgb(&heap, value::kw("red")), Some([0xcd, 0x31, 0x31]));
        // an explicit vector, clamped to 0..255
        let v = heap.alloc_vector(vec![Value::int(300), Value::int(-5), Value::int(128)]);
        assert_eq!(face_rgb(&heap, v), Some([255, 0, 128]));
        // a hex string
        let s = heap.alloc_string("#282c34");
        assert_eq!(face_rgb(&heap, s), Some([0x28, 0x2c, 0x34]));
        // anything else is the default face
        assert_eq!(face_rgb(&heap, Value::int(7)), None);
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
mod mouse_event_tests {
    use super::mouse_to_value;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};
    use crossterm::event::{KeyModifiers, MouseButton as CB, MouseEvent, MouseEventKind as MK};

    fn ev(kind: MK) -> MouseEvent {
        MouseEvent {
            kind,
            column: 7,
            row: 3,
            modifiers: KeyModifiers::empty(),
        }
    }

    // The `[:mouse action button row col]` shape: pull out the action keyword (idx 1)
    // and the button keyword (idx 2) as interned ids — `Value` has no `PartialEq`, so
    // we compare the underlying `u32`s. Lets us assert the crossterm → Brood mapping,
    // including the newly added :drag / :release (ADR-077).
    fn action_button(heap: &Heap, v: Value) -> (u32, u32) {
        let Value::Vector(id) = v else {
            panic!("expected a [:mouse …] vector, got {v:?}");
        };
        let xs = heap.vector(id);
        let (Value::Keyword(head), Value::Keyword(a), Value::Keyword(b)) = (xs[0], xs[1], xs[2])
        else {
            panic!("expected keywords for head/action/button, got {xs:?}");
        };
        assert_eq!(head, value::intern("mouse"));
        (a, b)
    }

    #[test]
    fn drag_and_release_map_to_keywords_carrying_their_button() {
        let mut heap = Heap::new();

        let v = mouse_to_value(&mut heap, ev(MK::Drag(CB::Left)));
        let (a, b) = action_button(&heap, v);
        assert_eq!(a, value::intern("drag"));
        assert_eq!(b, value::intern("left"));

        let v = mouse_to_value(&mut heap, ev(MK::Up(CB::Right)));
        let (a, b) = action_button(&heap, v);
        assert_eq!(a, value::intern("release"));
        assert_eq!(b, value::intern("right"));

        let v = mouse_to_value(&mut heap, ev(MK::Down(CB::Middle)));
        let (a, b) = action_button(&heap, v);
        assert_eq!(a, value::intern("press"));
        assert_eq!(b, value::intern("middle"));
    }

    // Bare motion (no button held) still isn't surfaced — it stays nil, as before, so
    // the input channel isn't flooded with per-cell moves when nothing is dragging.
    #[test]
    fn bare_motion_is_not_emitted() {
        let mut heap = Heap::new();
        assert!(matches!(
            mouse_to_value(&mut heap, ev(MK::Moved)),
            Value::Nil
        ));
    }

    // Held modifiers ride on the event as a trailing `[:ctrl …]` vector (so an app
    // can bind Ctrl+wheel for zoom). No modifiers → an empty vector, not absent.
    #[test]
    fn modifiers_ride_on_the_event() {
        let mut heap = Heap::new();

        // Ctrl held during a scroll → mods is `[:ctrl]` at index 5.
        let ctrl_scroll = MouseEvent {
            kind: MK::ScrollUp,
            column: 7,
            row: 3,
            modifiers: KeyModifiers::CONTROL,
        };
        let Value::Vector(id) = mouse_to_value(&mut heap, ctrl_scroll) else {
            panic!("expected a [:mouse …] vector");
        };
        let xs = heap.vector(id).to_vec();
        assert_eq!(xs.len(), 6, "event now carries a trailing mods vector");
        let Value::Vector(mid) = xs[5] else {
            panic!("mods should be a vector, got {:?}", xs[5]);
        };
        let mods = heap.vector(mid);
        assert_eq!(mods.len(), 1);
        assert!(matches!(mods[0], Value::Keyword(k) if k == value::intern("ctrl")));

        // No modifiers → an empty mods vector (present, so destructuring is stable).
        let Value::Vector(id) = mouse_to_value(&mut heap, ev(MK::ScrollUp)) else {
            panic!("expected a [:mouse …] vector");
        };
        let xs = heap.vector(id).to_vec();
        let Value::Vector(mid) = xs[5] else {
            panic!("mods should be a vector");
        };
        assert!(heap.vector(mid).is_empty());
    }

    // A press carries a trailing click-chain count (the 7th element); the terminal
    // can't detect multi-click, so it always reports 1. Non-press actions omit it.
    #[test]
    fn a_press_carries_a_trailing_count_others_do_not() {
        let mut heap = Heap::new();

        let Value::Vector(id) = mouse_to_value(&mut heap, ev(MK::Down(CB::Left))) else {
            panic!("expected a [:mouse …] vector");
        };
        let xs = heap.vector(id).to_vec();
        assert_eq!(xs.len(), 7, "a press has the trailing count");
        assert!(matches!(xs[6], Value::Int(1)), "terminal press counts as 1");

        // A release stays 6-element (no count).
        let Value::Vector(id) = mouse_to_value(&mut heap, ev(MK::Up(CB::Left))) else {
            panic!("expected a [:mouse …] vector");
        };
        assert_eq!(heap.vector(id).len(), 6, "a release has no trailing count");
    }
}

#[cfg(test)]
mod key_to_value_tests {
    use super::key_to_value;
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn kw_of(code: KeyCode, mods: KeyModifiers) -> String {
        let mut heap = Heap::new();
        match key_to_value(&mut heap, KeyEvent::new(code, mods)) {
            Value::Keyword(s) => value::symbol_name(s),
            other => panic!("not a keyword: {other:?}"),
        }
    }

    // The named keys spell their modifiers the way character chords do — the same rule
    // the GUI frontend applies (`host::gui::named_key`), so a binding reads the same in
    // both. A plain arrow is unchanged.
    #[test]
    fn named_keys_carry_their_modifiers() {
        assert_eq!(kw_of(KeyCode::Left, KeyModifiers::NONE), "left");
        assert_eq!(kw_of(KeyCode::Left, KeyModifiers::SHIFT), "shift-left");
        assert_eq!(kw_of(KeyCode::Left, KeyModifiers::CONTROL), "ctrl-left");
        assert_eq!(
            kw_of(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            "ctrl-shift-up"
        );
        assert_eq!(kw_of(KeyCode::Down, KeyModifiers::ALT), "alt-down");
        assert_eq!(
            kw_of(KeyCode::Backspace, KeyModifiers::CONTROL),
            "ctrl-backspace"
        );
        assert_eq!(kw_of(KeyCode::BackTab, KeyModifiers::SHIFT), "back-tab");
        assert_eq!(kw_of(KeyCode::Esc, KeyModifiers::CONTROL), "escape");
    }

    // Function keys, Insert and Super spell the way the GUI frontend spells them
    // (ADR-441), so `<f5>`, `C-<f5>`, `<insert>` and `s-x` bind the same in both.
    #[test]
    fn function_keys_insert_and_super_are_named() {
        assert_eq!(kw_of(KeyCode::F(5), KeyModifiers::NONE), "f5");
        assert_eq!(kw_of(KeyCode::F(12), KeyModifiers::CONTROL), "ctrl-f12");
        assert_eq!(kw_of(KeyCode::Insert, KeyModifiers::SHIFT), "shift-insert");
        assert_eq!(kw_of(KeyCode::Char('x'), KeyModifiers::SUPER), "super-x");
        assert_eq!(
            kw_of(
                KeyCode::Char('x'),
                KeyModifiers::SUPER | KeyModifiers::CONTROL
            ),
            "ctrl-super-x"
        );
        assert_eq!(kw_of(KeyCode::F(3), KeyModifiers::SUPER), "super-f3");
    }

    // Under the kitty protocol's alternate keys a shifted chord arrives as its glyph with
    // Shift dropped; it spells as the legacy Esc-prefixed chord, Shift set, does.
    #[test]
    fn an_alternate_key_chord_spells_as_the_legacy_one() {
        assert_eq!(
            kw_of(KeyCode::Char('X'), KeyModifiers::ALT),
            kw_of(KeyCode::Char('X'), KeyModifiers::ALT | KeyModifiers::SHIFT)
        );
        assert_eq!(kw_of(KeyCode::Char('X'), KeyModifiers::ALT), "alt-X");
        assert_eq!(kw_of(KeyCode::Char('<'), KeyModifiers::ALT), "alt-<");
        assert_eq!(kw_of(KeyCode::Char('A'), KeyModifiers::CONTROL), "ctrl-a");
    }

    // A release is `[:key-up press]`, `press` what the press event spelled.
    #[test]
    fn a_key_up_is_its_press_spelled() {
        let mut heap = Heap::new();
        let release = super::key_up_value(
            &mut heap,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
        );
        let Value::Vector(id) = release else {
            panic!("expected a [:key-up …] vector, got {release:?}");
        };
        let (Value::Keyword(head), Value::Keyword(press)) =
            (heap.vector(id)[0], heap.vector(id)[1])
        else {
            panic!("expected [:key-up :ctrl-n]");
        };
        assert_eq!(value::symbol_name(head), "key-up");
        assert_eq!(value::symbol_name(press), "ctrl-n");
        assert!(matches!(
            super::key_up_value(&mut heap, KeyEvent::new(KeyCode::Null, KeyModifiers::NONE)),
            Value::Nil
        ));
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
