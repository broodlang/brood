use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use crate::builtins::args::{arg, expect_int, expect_string};
use crate::builtins::display::*;
use crate::builtins::io::capture_write;
use crate::builtins::numeric::num_to_f64;

#[cfg(unix)]
mod decoder;
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
    let Some(face) = read_face(heap, face) else {
        return screen::Style::default();
    };
    let colour = |value: Option<Value>| {
        value
            .and_then(|v| color_of(heap, v))
            .and_then(screen::Colour::of)
    };
    let (underline, underline_colour) = match &face.underline {
        None => (screen::Underline::None, None),
        Some(spec) if spec.wave => (screen::Underline::Curly, colour(spec.colour)),
        Some(spec) => (screen::Underline::Straight, colour(spec.colour)),
    };
    screen::Style {
        fg: colour(face.fg),
        bg: colour(face.bg),
        bold: face.bold,
        italic: face.italic,
        underline,
        underline_colour,
        reverse: face.reverse,
    }
}

/// `(term-draw frame)` — paint a frame: a vector of op vectors `[:clear]`,
/// `[:text row col str]`, `[:text row col str face]`, `[:rect row col w h face]`,
/// `[:cursor row col]` / `[:cursor row col style]`, `[:scroll-region dy ops]` and
/// `[:above-seam ops]` (their ops drawn
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
    let above_seam_t = value::intern("above-seam");
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
        } else if tag == above_seam_t {
            // a terminal has no leftover strip (ADR-615): the inner ops are drawn where they are
            if let Ok(inner_parsed) =
                frame_ops(heap, arg(&parts, 1), "%term-draw", "above-seam ops")
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

/// Apply a face map (`{:fg :red :bg :blue :bold true :reverse true}`) as
/// crossterm style commands. A non-map (or nil) face is a no-op. Unknown colour
/// names are skipped. Callers reset attributes after the text.
pub(in crate::builtins) fn apply_face<W: std::io::Write>(
    out: &mut W,
    heap: &Heap,
    face: Value,
) -> Result<(), LispError> {
    use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
    let Some(face) = read_face(heap, face) else {
        return Ok(());
    };
    if let Some(fg) = face.fg.and_then(|v| color_of(heap, v)) {
        crossterm::queue!(out, SetForegroundColor(fg)).map_err(term_err)?;
    }
    if let Some(bg) = face.bg.and_then(|v| color_of(heap, v)) {
        crossterm::queue!(out, SetBackgroundColor(bg)).map_err(term_err)?;
    }
    if face.bold {
        crossterm::queue!(out, SetAttribute(Attribute::Bold)).map_err(term_err)?;
    }
    if face.italic {
        crossterm::queue!(out, SetAttribute(Attribute::Italic)).map_err(term_err)?;
    }
    if let Some(underline) = &face.underline {
        // a terminal that knows the extended underline (kitty, foot, wezterm) draws it
        // coloured and curly, one that does not falls back to a plain underline
        let style = if underline.wave {
            Attribute::Undercurled
        } else {
            Attribute::Underlined
        };
        crossterm::queue!(out, SetAttribute(style)).map_err(term_err)?;
        if let Some(colour) = underline.colour.and_then(|v| color_of(heap, v)) {
            crossterm::queue!(out, crossterm::style::SetUnderlineColor(colour))
                .map_err(term_err)?;
        }
    }
    if face.reverse {
        crossterm::queue!(out, SetAttribute(Attribute::Reverse)).map_err(term_err)?;
    }
    Ok(())
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
