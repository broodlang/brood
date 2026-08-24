use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::io::capture_write;
use super::numeric::{arg, expect_bigint, expect_int, expect_string, num_to_f64};
// The thin crossterm seam: enter/leave the alternate screen, read keys, and
// paint a *frame* — a Brood vector of render ops. The protocol's meaning is
// data (the ops); these primitives are the in-process frontend that interprets
// it, so a remote frontend can implement the identical op vocabulary later.
// Errors surface as clean `LispError`s (never a crossterm panic), mirroring the
// rope primitives' discipline.

/// Map a crossterm I/O error into a runtime `LispError`.
pub(super) fn term_err(e: std::io::Error) -> LispError {
    LispError::runtime(format!("terminal: {}", e))
}

/// `(term-enter)` — take over the terminal: raw mode + alternate screen, cursor
/// hidden. Pair with `term-leave`. The Rust-side `nest observe` guard also
/// restores the terminal if the program panics, so a crash never wrecks it.
pub(super) fn term_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Hide;
    use crossterm::event::EnableMouseCapture;
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
    enable_raw_mode().map_err(term_err)?;
    // Mouse capture rides with the full-screen path only (not the inline REPL
    // seam), so click/scroll reach `term-poll`. It costs terminal text-selection
    // while active — standard for a TUI, and only for the duration of the UI.
    crossterm::execute!(
        std::io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        Hide
    )
    .map_err(term_err)?;
    Ok(Value::nil())
}

/// `(term-leave)` — restore the terminal (show cursor, leave alternate screen,
/// disable raw mode). The normal-path teardown for `term-enter`.
pub(super) fn term_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    crossterm::execute!(
        std::io::stdout(),
        Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .map_err(term_err)?;
    disable_raw_mode().map_err(term_err)?;
    Ok(Value::nil())
}

/// Best-effort terminal restore — the abnormal-path backstop a host binary holds
/// in an RAII guard so a panic or error during a full-screen UI (`nest observe`)
/// never leaves the terminal in raw mode / the alternate screen. Idempotent and
/// errors are swallowed (the normal path is the Brood `term-leave`).
pub fn restore_terminal() {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    let _ = crossterm::execute!(
        std::io::stdout(),
        Show,
        DisableMouseCapture,
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
pub(super) fn term_size(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (cols, rows) = crossterm::terminal::size().map_err(term_err)?;
    Ok(heap.alloc_vector(vec![Value::int(cols as i64), Value::int(rows as i64)]))
}

/// `(term-poll ms)` — wait up to `ms` ms for a key; return it (a 1-char string,
/// or a keyword for specials) or `nil` on timeout. Always called with a finite
/// `ms`: the observer is the root process, so blocking here blocks only the root
/// thread (never a scheduler worker), but an *infinite* poll on a green process
/// would pin a worker (native blocking can't be preempted) — hence finite.
pub(super) fn term_poll(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::event::{poll, read, Event, KeyEventKind};
    let ms = expect_int(heap, "%term-poll", arg(args, 0))?.max(0) as u64;
    if poll(std::time::Duration::from_millis(ms)).map_err(term_err)? {
        match read().map_err(term_err)? {
            // Ignore key *release* events (reported on some platforms with the
            // enhanced-keyboard protocol) so a keypress isn't seen twice.
            Event::Key(k) if k.kind != KeyEventKind::Release => Ok(key_to_value(heap, k)),
            Event::Mouse(m) => Ok(mouse_to_value(heap, m)),
            _ => Ok(Value::nil()),
        }
    } else {
        Ok(Value::nil())
    }
}

/// Encode a Brood mouse event as the vector `[:mouse action button row col mods]`
/// — the shared shape both frontends yield, so the observer (and any future UI)
/// reads one form. `action` is a keyword, `button` a keyword or nil, `row`/`col`
/// 0-based cell coordinates, `mods` a vector of the held modifier keywords (in a
/// stable `[:ctrl :alt :shift]` order, `[]` when none) so an app can bind
/// Ctrl+wheel etc.
pub(super) fn mouse_value(
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
pub(super) fn mouse_to_value(heap: &mut Heap, m: crossterm::event::MouseEvent) -> Value {
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
pub(super) fn key_to_value(heap: &mut Heap, k: crossterm::event::KeyEvent) -> Value {
    use crossterm::event::{KeyCode, KeyModifiers};
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    match k.code {
        // Ctrl+Alt (Emacs C-M-… — structural sexp motion C-M-f/b/u/d, mark-sexp
        // C-M-SPC). Must precede the ctrl-only / alt-only arms so the second modifier
        // isn't dropped (the `ctrl-meta-` spelling the keymaps bind).
        KeyCode::Char(c) if ctrl && alt => Value::keyword(value::intern(&format!(
            "ctrl-meta-{}",
            c.to_ascii_lowercase()
        ))),
        KeyCode::Char(c) if ctrl => {
            Value::keyword(value::intern(&format!("ctrl-{}", c.to_ascii_lowercase())))
        }
        // Alt/Meta combos (M-f, M-b, … — emacs word motion). Some terminals send
        // these as an Esc prefix; crossterm normalises them to the ALT modifier.
        KeyCode::Char(c) if alt => {
            // Meta is case-SENSITIVE (`M-O` ≠ `M-o`): keep a shifted letter upper-case so
            // the two are distinct; an unshifted chord lower-cases (Caps Lock / a stray
            // Shift can't change the binding). Mirrors the GUI frontend
            // (`gui::backend::translate_key`); Control chords above stay case-insensitive.
            let ch = if k.modifiers.contains(KeyModifiers::SHIFT) {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            };
            Value::keyword(value::intern(&format!("alt-{ch}")))
        }
        KeyCode::Char(c) => heap.alloc_string(&c.to_string()),
        KeyCode::Up => value::kw("up"),
        KeyCode::Down => value::kw("down"),
        KeyCode::Left => value::kw("left"),
        KeyCode::Right => value::kw("right"),
        KeyCode::Enter => value::kw("enter"),
        KeyCode::Esc => value::kw("escape"),
        KeyCode::Backspace => value::kw("backspace"),
        KeyCode::Tab => value::kw("tab"),
        KeyCode::BackTab => value::kw("back-tab"),
        KeyCode::Delete => value::kw("delete"),
        KeyCode::Home => value::kw("home"),
        KeyCode::End => value::kw("end"),
        KeyCode::PageUp => value::kw("page-up"),
        KeyCode::PageDown => value::kw("page-down"),
        _ => Value::nil(),
    }
}

/// `(term-draw frame)` — paint a frame: a vector of op vectors `[:clear]`,
/// `[:text row col str]`, `[:text row col str face]`, `[:cursor row col]`.
/// Unknown ops are skipped (forward-compatible protocol). Queues all ops then
/// flushes once, so a frame paints without intermediate tearing.
/// Write rendered terminal bytes (escape sequences) to stdout — unless an MCP
/// stdout-capture is active on this thread, in which case divert them into the
/// capture buffer instead. During a `nest mcp` `tools/call`, stdout *is* the
/// JSON-RPC channel, so a `term-draw` / `term-emit` writing raw escapes there would
/// corrupt the protocol and wedge the client (the `print` capture only catches
/// Brood `print`, not these direct crossterm writes). Diverting keeps the channel
/// pure and rides the rendered bytes back in the result envelope, so an agent can
/// still inspect what a frame produced. Mirrors `print`'s capture check.
pub(super) fn write_term_bytes(bytes: &[u8]) -> std::io::Result<()> {
    if !capture_write(&String::from_utf8_lossy(bytes)) {
        use std::io::Write;
        let mut real = std::io::stdout();
        real.write_all(bytes)?;
        real.flush()?;
    }
    Ok(())
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
pub(super) fn frame_ops(
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

pub(super) fn term_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crossterm::cursor::MoveTo;
    use crossterm::style::{Attribute, Print, ResetColor, SetAttribute};
    use crossterm::terminal::{Clear, ClearType};

    let parsed = frame_ops(heap, arg(args, 0), "%term-draw", "vector (a frame)")?;
    let clear_t = value::intern("clear");
    let text_t = value::intern("text");
    let cursor_t = value::intern("cursor");
    let rect_t = value::intern("rect");
    let scroll_region_t = value::intern("scroll-region");
    // Use a deque so scroll-region inner ops can be prepended and processed in order.
    let mut queue: std::collections::VecDeque<(value::Symbol, Vec<Value>)> = parsed.into();
    let mut out: Vec<u8> = Vec::new();
    while let Some((tag, parts)) = queue.pop_front() {
        let (tag, parts) = (tag, parts);
        if tag == clear_t {
            crossterm::queue!(out, Clear(ClearType::All)).map_err(term_err)?;
        } else if tag == rect_t {
            // [:rect row col w h face] — fill the block by printing `w` spaces in the
            // face on each of the `h` rows, so the face `:bg` (or `:reverse`) shows.
            let row = expect_int(heap, "%term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "%term-draw", arg(&parts, 2))?;
            // `w`/`h` are attacker-reachable ints that size an allocation and a loop,
            // so they get `clamp_extent`, not a bare `.max(0)`: `" ".repeat(w)` with a
            // huge `w` is an allocator abort, which no `try` can catch (see MAX_EXTENT).
            let w = clamp_extent(expect_int(heap, "%term-draw", arg(&parts, 3))?) as usize;
            let h = clamp_extent(expect_int(heap, "%term-draw", arg(&parts, 4))?) as i64;
            let face = parts.get(5).copied().unwrap_or(Value::nil());
            let fill = " ".repeat(w);
            for i in 0..h {
                // `saturating_add`: `row` and `i` are both bounded but `row` is a raw
                // caller int, so `row + i` near i64::MAX panics under debug-assertions
                // (the build this repo's own debug guidance recommends). Clamp after a
                // saturating add, never before it.
                crossterm::queue!(
                    out,
                    MoveTo(clamp_u16(col), clamp_u16(row.saturating_add(i)))
                )
                .map_err(term_err)?;
                apply_face(&mut out, heap, face)?;
                crossterm::queue!(
                    out,
                    Print(&fill),
                    SetAttribute(Attribute::Reset),
                    ResetColor
                )
                .map_err(term_err)?;
            }
        } else if tag == cursor_t {
            use crate::gui::CursorStyle;
            use crossterm::cursor::SetCursorStyle;
            let row = expect_int(heap, "%term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "%term-draw", arg(&parts, 2))?;
            crossterm::queue!(out, MoveTo(clamp_u16(col), clamp_u16(row))).map_err(term_err)?;
            // honour the optional style keyword so the caret shape matches the GUI
            match cursor_style_from(parts.get(3).copied().unwrap_or(Value::nil())) {
                CursorStyle::Bar => {
                    crossterm::queue!(out, SetCursorStyle::SteadyBar).map_err(term_err)?
                }
                CursorStyle::Underline => {
                    crossterm::queue!(out, SetCursorStyle::SteadyUnderScore).map_err(term_err)?
                }
                CursorStyle::Block => {
                    crossterm::queue!(out, SetCursorStyle::SteadyBlock).map_err(term_err)?
                }
            }
        } else if tag == text_t {
            let row = expect_int(heap, "%term-draw", arg(&parts, 1))?;
            let col = expect_int(heap, "%term-draw", arg(&parts, 2))?;
            let s = expect_string(heap, "%term-draw", arg(&parts, 3))?;
            crossterm::queue!(out, MoveTo(clamp_u16(col), clamp_u16(row))).map_err(term_err)?;
            apply_face(
                &mut out,
                heap,
                parts.get(4).copied().unwrap_or(Value::nil()),
            )?;
            crossterm::queue!(out, Print(s), SetAttribute(Attribute::Reset), ResetColor)
                .map_err(term_err)?;
        } else if tag == scroll_region_t {
            // terminal can't do sub-pixel offsets; flatten by prepending inner ops to the queue.
            let inner_val = arg(&parts, 2);
            if let Ok(inner_parsed) = frame_ops(heap, inner_val, "%term-draw", "scroll-region ops")
            {
                for item in inner_parsed.into_iter().rev() {
                    queue.push_front(item);
                }
            }
        }
    }
    write_term_bytes(&out).map_err(term_err)?;
    Ok(Value::nil())
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
pub(super) fn term_raw_enter(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use crossterm::cursor::Show;
    use crossterm::event::DisableMouseCapture;
    crossterm::terminal::enable_raw_mode().map_err(term_err)?;
    crossterm::execute!(std::io::stdout(), Show, DisableMouseCapture).map_err(term_err)?;
    Ok(Value::nil())
}

/// `(term-raw-leave)` — leave raw mode (the teardown for `term-raw-enter`).
/// Idempotent with the panic-path `restore_terminal`.
pub(super) fn term_raw_leave(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
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
pub(super) fn term_emit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
});

/// Apply a face map (`{:fg :red :bg :blue :bold true :reverse true}`) as
/// crossterm style commands. A non-map (or nil) face is a no-op. Unknown colour
/// names are skipped. Callers reset attributes after the text.
pub(super) fn apply_face<W: std::io::Write>(
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
    if heap.map_get(id, k.underline).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Underlined)).map_err(term_err)?;
    }
    if heap.map_get(id, k.reverse).is_some_and(face_truthy) {
        crossterm::queue!(out, SetAttribute(Attribute::Reverse)).map_err(term_err)?;
    }
    Ok(())
}

/// Brood truthiness for a face flag: only `nil`/`false` are falsy.
pub(super) fn face_truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// A face colour value to a crossterm `Color`. A palette keyword (`:red`,
/// `:dark-grey`, …) maps to the terminal's *named* colour, so it honours the
/// user's terminal theme; an explicit `[r g b]` vector or `"#rrggbb"` string maps
/// to a true-colour cell (`Color::Rgb`) — the same RGB the GUI frontend paints, so
/// a curated palette renders identically in both.
pub(super) fn color_of(heap: &Heap, v: Value) -> Option<crossterm::style::Color> {
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
pub(super) fn clamp_u16(n: i64) -> u16 {
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
pub(super) fn clamp_extent(n: i64) -> u16 {
    n.clamp(0, MAX_EXTENT) as u16
}

// ---- the GUI frontend (ADR-046, feature "gui") ------------------------------
//
// `gui-*` mirror `term-*`: a second frontend that paints the *same* render-op
// protocol (a frame is the same Brood data) to a native window and reads keys
// back in the same encoding. The window/loop machinery lives in `crate::gui`
// (behind the `gui` feature); these primitives just translate Brood `Value`s ⇄
// the plain `gui::Op`/`gui::Key`/`gui::Face` the backend speaks. A composite
// "broadcast" display in std/tool/observer.blsp drives term + gui (+ remote later)
// from one frame — so the frontends can't drift. Without `--features gui` the
// backend functions return a clear "rebuild with --features gui" error.

/// A face colour keyword (`:red`, `:dark-grey`, …) to an RGB triple for the GUI
/// framebuffer. The same palette `color_of` maps to crossterm `Color`s, so the
/// two frontends agree on what `:red` looks like.
pub(super) fn color_rgb(v: Value) -> Option<[u8; 3]> {
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
pub(super) fn face_rgb(heap: &Heap, v: Value) -> Option<[u8; 3]> {
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
        Value::Str(id) => parse_hex_color(&heap.string(id)),
        _ => None,
    }
}

/// Parse a `"#rgb"` or `"#rrggbb"` hex colour to an RGB triple. `None` for any
/// other shape (no leading `#`, a bad length, or a non-hex digit). The 3-digit
/// shorthand expands each nibble (`#f0a` → `[0xff 0x00 0xaa]`).
pub(super) fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
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
pub(super) fn gui_face(heap: &Heap, face: Value) -> crate::gui::Face {
    let mut f = crate::gui::Face::default();
    let Value::Map(id) = face else { return f };
    let k = &*FACE_KEYS;
    f.fg = heap.map_get(id, k.fg).and_then(|v| face_rgb(heap, v));
    f.bg = heap.map_get(id, k.bg).and_then(|v| face_rgb(heap, v));
    f.bold = heap.map_get(id, k.bold).is_some_and(face_truthy);
    f.italic = heap.map_get(id, k.italic).is_some_and(face_truthy);
    f.underline = heap.map_get(id, k.underline).is_some_and(face_truthy);
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
pub(super) fn gui_window_id(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    Ok(expect_int(heap, who, v)?.max(0) as u64)
}

/// `(gui-open)` / `(gui-open title)` — open a new native window and return its integer
/// id, optionally with a title-bar string (else a default `brood observer #id`). Its
/// key/mouse input is delivered to the **calling process's mailbox** (ADR-058), so the
/// observer parks in `(receive)` rather than pinning a worker in a blocking poll.
/// Starts the GUI thread on the first call; each call is an independent window.
pub(super) fn gui_open(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
                    "gui-open: :app-id must not be empty — it names the .desktop entry \
                     the desktop matches this window to (pass nil for no app id)",
                ));
            }
            Some(name)
        }
    };
    let spec = crate::gui::WindowSpec {
        title,
        size,
        decorations,
        app_id,
    };
    let id = crate::gui::open(crate::process::self_pid(), spec).map_err(LispError::runtime)?;
    Ok(Value::int(id as i64))
}

/// `(audio-beep freq-hz ms [vol])` — play a short tone of `freq-hz` for `ms`
/// milliseconds at peak amplitude `vol` (0..1, default ~0.18). Fire-and-forget
/// (never blocks); overlapping beeps mix. A no-op without `--features audio`,
/// when there's no audio device, or when muted (`BROOD_AUDIO=0` /
/// `BROOD_GUI_HEADLESS`). Returns nil.
pub(super) fn audio_beep(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let freq = num_to_f64(heap, "audio-beep", arg(args, 0))?;
    let ms = num_to_f64(heap, "audio-beep", arg(args, 1))?;
    // Optional 3rd arg is peak amplitude; 0.0 (also the default) means "use the
    // backend's default volume".
    let vol = if args.len() >= 3 {
        num_to_f64(heap, "audio-beep", arg(args, 2))? as f32
    } else {
        0.0
    };
    crate::audio::beep(freq as f32, ms.max(0.0) as u64, vol);
    Ok(Value::Nil)
}

/// `(gui-close id)` — close window `id` (the teardown for `gui-open`; idempotent).
pub(super) fn gui_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-close", arg(args, 0))?;
    crate::gui::close(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-title! id text)` — set window `id`'s OS title-bar text at runtime.
pub(super) fn gui_title(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-title!", arg(args, 0))?;
    let title = expect_string(heap, "%gui-title!", arg(args, 1))?;
    crate::gui::title(id, title).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-icon! id rgba w h)` — set window `id`'s taskbar/title-bar icon from raw RGBA
/// pixels (a vector of `w*h*4` byte ints, row-major).
pub(super) fn gui_icon(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
                "gui-icon!: rgba must be a vector of bytes".to_string(),
            ))
        }
    };
    // Validate BEFORE the narrowing cast, and against the buffer we actually have:
    // `expect_int(..)? as u32` truncates silently (4294967296 arrives as 0), and the
    // backend reads `w * h * 4` bytes out of `rgba`, so a dimension that doesn't match
    // the pixel count is a malformed icon at best. A caller mistake must be a clean
    // catchable error, never a cast that lies about the size.
    let (w, h) = match (u32::try_from(w_i), u32::try_from(h_i)) {
        (Ok(w), Ok(h)) if (w as u64) * (h as u64) * 4 == rgba.len() as u64 => (w, h),
        _ => {
            return Err(LispError::runtime(format!(
                "gui-icon!: w*h*4 must equal the rgba byte count (w={w_i}, h={h_i}, {} bytes)",
                rgba.len()
            )))
        }
    };
    crate::gui::icon(id, rgba, w, h).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-focus id)` — raise window `id` and give it OS keyboard focus (un-minimising
/// it). Lets an app surface an already-open singleton window instead of opening a
/// duplicate. Errors only if `id` isn't a live window.
pub(super) fn gui_focus(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-focus", arg(args, 0))?;
    crate::gui::focus(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-grab-cursor id on)` — confine the pointer to window `id` while `on` is
/// truthy, release it otherwise.
pub(super) fn gui_grab_cursor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-grab-cursor", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::gui::grab(id, on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-fullscreen! id on)` — make window `id` borderless-fullscreen (`on` truthy)
/// or restore it to a normal window.
pub(super) fn gui_fullscreen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-fullscreen!", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::gui::fullscreen(id, on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-minimize! id)` — iconify window `id`. The counterpart of `gui-maximize!`
/// for an app that draws its own window controls, which a borderless window
/// (`gui-open` with `{:decorations false}`) must.
pub(super) fn gui_minimize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-minimize!", arg(args, 0))?;
    crate::gui::minimize(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-drag-move id)` — hand window `id` to the window manager for an interactive
/// move, for the rest of the currently-held press.
///
/// This is what a borderless window needs to stay movable: with no OS title bar
/// there is nothing to grab, so the app nominates a region of its own chrome (a
/// browser's tab strip) and calls this when a press lands there.
pub(super) fn gui_drag_move(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-drag-move", arg(args, 0))?;
    crate::gui::drag_move(id).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-drag-resize id dir)` — hand window `id` to the window manager for an
/// interactive resize from `dir`: `:north` `:south` `:east` `:west` `:north-east`
/// `:north-west` `:south-east` `:south-west`. The window-frame counterpart of
/// `gui-drag-move`, for a borderless window that draws its own edges.
pub(super) fn gui_drag_resize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-drag-resize", arg(args, 0))?;
    let dir = match arg(args, 1) {
        Value::Keyword(sym) => value::symbol_name(sym),
        v => expect_string(heap, "%gui-drag-resize", v)?,
    };
    crate::gui::drag_resize(id, &dir).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-maximize! id on)` — maximise window `id` (`on` truthy) or restore it,
/// keeping the title bar / decorations (unlike fullscreen).
pub(super) fn gui_maximize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-maximize!", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::gui::maximize(id, on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-size id)` — window `id`'s size as `[cols rows]` (character cells), same
/// shape as `term-size`.
pub(super) fn gui_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-size", arg(args, 0))?;
    let (cols, rows) = crate::gui::size(id).map_err(LispError::runtime)?;
    Ok(heap.alloc_vector(vec![Value::int(cols as i64), Value::int(rows as i64)]))
}

/// A held `gui::Key` as the same Brood value `gui-open` delivers for that press —
/// a 1-char string, else a `:ctrl-…` / `:alt-…` / `:ctrl-meta-…` / named keyword —
/// so an app can compare `(gui-held-key id)` directly against the key it last saw.
pub(super) fn gui_key_to_value(heap: &mut Heap, k: crate::gui::Key) -> Value {
    use crate::gui::Key;
    match k {
        Key::Char(c) => heap.alloc_string(&c.to_string()),
        Key::Ctrl(c) => Value::keyword(value::intern(&format!("ctrl-{c}"))),
        Key::Alt(c) => Value::keyword(value::intern(&format!("alt-{c}"))),
        Key::CtrlAlt(c) => Value::keyword(value::intern(&format!("ctrl-meta-{c}"))),
        Key::Named(s) => Value::keyword(value::intern(s)),
    }
}

/// `(gui-held-key id)` — the key window `id` currently sees as physically held (the
/// same value its press delivered), or nil when none. Tracked from press/release
/// transitions, not winit's unreliable `ke.repeat`, so it's the source of truth a
/// consumer-paced key repeat polls to stop the instant the key is up — making a
/// missed key-up unable to cause runaway repeat (ADR-086).
pub(super) fn gui_held_key(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = gui_window_id(heap, "%gui-held-key", arg(args, 0))?;
    match crate::gui::held_key(id).map_err(LispError::runtime)? {
        Some(k) => Ok(gui_key_to_value(heap, k)),
        None => Ok(Value::nil()),
    }
}

/// `(gui-draw id frame)` — paint a frame (the same op vector `term-draw` takes) to
/// window `id`. Parses the ops into plain `gui::Op`s (it has heap access) and ships
/// them to the GUI thread. Unknown ops are skipped (forward-compatible).
/// Interned tag symbols shared between `gui_draw` and `parse_gui_ops`.
struct GuiOpTags {
    clear_t: value::Symbol,
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
    rect_t: value::Symbol,
    frect_t: value::Symbol,
}
impl GuiOpTags {
    fn new() -> Self {
        Self {
            clear_t: value::intern("clear"),
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
            rect_t: value::intern("rect"),
            frect_t: value::intern("frect"),
        }
    }
}

/// Parse a sequence of already-tagged op tuples into `gui::Op`s. Called recursively
/// for the inner ops of a `ScrollRegion`. Malformed or unknown ops are skipped.
fn parse_gui_ops(
    heap: &Heap,
    parsed: Vec<(value::Symbol, Vec<Value>)>,
    tags: &GuiOpTags,
) -> Vec<crate::gui::Op> {
    let num = |v: Value| -> f32 {
        match v {
            Value::Int(n) => n as f32,
            Value::Float(f) => f as f32,
            _ => 0.0,
        }
    };
    let mut ops = Vec::with_capacity(parsed.len());
    for (tag, parts) in parsed {
        if tag == tags.clear_t {
            ops.push(crate::gui::Op::Clear);
        } else if tag == tags.cursor_t {
            let Ok(row_i) = expect_int(heap, "%gui-draw", arg(&parts, 1)) else {
                continue;
            };
            let Ok(col_i) = expect_int(heap, "%gui-draw", arg(&parts, 2)) else {
                continue;
            };
            let style = cursor_style_from(parts.get(3).copied().unwrap_or(Value::nil()));
            ops.push(crate::gui::Op::Cursor {
                row: clamp_u16(row_i),
                col: clamp_u16(col_i),
                style,
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
            ops.push(crate::gui::Op::Rect {
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
            ops.push(crate::gui::Op::FRect {
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
            ops.push(crate::gui::Op::Text {
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
                    Some(crate::gui::CursorShape::ColResize)
                }
                Some(Value::Keyword(s)) if *s == tags.row_resize_t => {
                    Some(crate::gui::CursorShape::RowResize)
                }
                Some(Value::Keyword(s)) if *s == tags.pointer_t => {
                    Some(crate::gui::CursorShape::Pointer)
                }
                _ => None,
            };
            if let Some(shape) = shape {
                ops.push(crate::gui::Op::CursorZone {
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
            ops.push(crate::gui::Op::VSpans {
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
            ops.push(crate::gui::Op::Cells {
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
            ops.push(crate::gui::Op::CellsRgb {
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
                let inner_ops = parse_gui_ops(heap, inner_parsed, tags);
                ops.push(crate::gui::Op::ScrollRegion {
                    dy_frac,
                    ops: inner_ops,
                });
            }
        }
    }
    ops
}

pub(super) fn gui_draw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let win = gui_window_id(heap, "%gui-draw", arg(args, 0))?;
    let parsed = frame_ops(heap, arg(args, 1), "%gui-draw", "vector (a frame)")?;
    let tags = GuiOpTags::new();
    let ops = parse_gui_ops(heap, parsed, &tags);
    crate::gui::draw(win, ops).map_err(LispError::runtime)?;
    Ok(Value::nil())
}
/// A `:vspans` segment colour: a face colour keyword (`:red` → the GUI palette),
/// an explicit `[r g b]` triple, or a `"#rrggbb"` hex string (all via the shared
/// `face_rgb`); anything else is `None` — "transparent", leaving the background
/// showing.
pub(super) fn span_color(heap: &Heap, v: Value) -> Option<[u8; 3]> {
    face_rgb(heap, v)
}

/// The cursor style from a `[:cursor row col style]` op's optional 4th element: a
/// `:bar` / `:underline` keyword, else (`:block`, nil, or anything unknown) the
/// default `Block`. Shared by both frontends so the caret shape agrees.
pub(super) fn cursor_style_from(v: Value) -> crate::gui::CursorStyle {
    use crate::gui::CursorStyle;
    match v {
        Value::Keyword(s) => match value::symbol_name(s).as_str() {
            "bar" => CursorStyle::Bar,
            "underline" => CursorStyle::Underline,
            _ => CursorStyle::Block,
        },
        _ => CursorStyle::Block,
    }
}

/// Read a `:height` value from a font spec as a pixel size (int or float), or None.
pub(super) fn font_px(heap: &Heap, id: crate::core::value::MapId) -> Option<f32> {
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
pub(super) fn gui_font(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
    crate::gui::font(win, family, font_px(heap, m)).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-inset! px)` — set the window content inset (logical pixels): a blank margin
/// before the cell grid on every edge, so text doesn't sit flush against the window
/// frame. Applies to every open window + the default for ones opened later. The grid
/// loses `2*px` per axis (fewer cells) and re-renders. GUI only.
pub(super) fn gui_inset(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let px = match arg(args, 0) {
        Value::Int(n) => n.max(0) as f32,
        Value::Float(f) => f.max(0.0) as f32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-inset!",
                "a number (pixels)",
                other,
            ))
        }
    };
    crate::gui::inset(px).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-bg! color)` — set the window background: the fill for `:clear`, the pre-clear,
/// and (being outside every cell) the inset margin + the cell-grid snap remainder. So a
/// GUI app's padding matches its own theme instead of the hardcoded default. `color` is
/// a keyword (`:base`-style named colour), an `[r g b]` vector, or a `"#rrggbb"` hex
/// string; `nil` restores the default. Applies to every open window + ones opened later.
/// GUI only; returns nil.
pub(super) fn gui_bg(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
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
    crate::gui::bg(rgb).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(gui-font-register name styles)` — register font family `name` (a keyword) from
/// `styles`, a map of style → TTF file path: `{:regular "…" :bold "…" :italic "…"
/// :bold-italic "…"}`. Only `:regular` is required; a missing style reuses the
/// regular file (so a single-file family works). The fonts are read here and parsed
/// on the GUI thread; afterwards a face's `:family <name>` selects them. Returns
/// `name`.
pub(super) fn gui_font_register(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Keyword(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-font-register",
                "keyword",
                other,
            ))
        }
    };
    let Value::Map(id) = arg(args, 1) else {
        return Err(LispError::wrong_type(
            heap,
            "%gui-font-register",
            "map (style → path)",
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
    let read = |p: &str| -> Result<Vec<u8>, LispError> {
        std::fs::read(p).map_err(|e| LispError::runtime(format!("gui-font-register: {p}: {e}")))
    };
    let regular_path = path("regular")?
        .ok_or_else(|| LispError::runtime("gui-font-register: a :regular path is required"))?;
    let regular = read(&regular_path)?;
    // each missing style falls back to the regular file's bytes
    let style = |key: &str| -> Result<Vec<u8>, LispError> {
        match path(key)? {
            Some(p) => read(&p),
            None => Ok(regular.clone()),
        }
    };
    let bold = style("bold")?;
    let italic = style("italic")?;
    let bold_italic = style("bold-italic")?;
    crate::gui::register_family(name, regular, bold, italic, bold_italic)
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
    use crate::gui::CursorStyle;

    #[test]
    fn maps_keywords_with_block_default() {
        assert_eq!(cursor_style_from(value::kw("bar")), CursorStyle::Bar);
        assert_eq!(
            cursor_style_from(value::kw("underline")),
            CursorStyle::Underline
        );
        assert_eq!(cursor_style_from(value::kw("block")), CursorStyle::Block);
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
