//! Console I/O: print/render/write to stdout and stderr, TTY probes, the stdout capture
//! `nest test` and the MCP server use, and the non-blocking
//! stdin reader behind `read-line`. Files, sockets, tables and child processes are their
//! own domains beside this one.

use super::args::{arg, expect_string};
use super::sequences::realize_seqviews;
use super::terminal::restore_terminal_on_exit;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::printer;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%eprint",
        Arity::any(),
        Sig::variadic(any, nil_ty),
        &["&", "xs"],
        "Write the display forms of the arguments to stderr; returns nil.",
        eprint,
    );
    // The render/write split behind the dynamic `*out*`/`*err*` ports
    // (std/prelude.blsp, std/io.blsp): `%render` produces the text `print` would
    // show, and `%write-out`/`%write-err` write a ready string to stdout/stderr.
    primitives.def(
        "%render",
        Arity::any(),
        Sig::variadic(any, string),
        &["&", "xs"],
        "The space-joined display forms of the arguments as one string (no output). The rendering half of `print`; Brood's print/println route the result through the dynamic `*out*` port.",
        render);
    primitives.def(
        "%write-out",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["s"],
        "Write the ready string `s` to the current stdout sink — the active capture buffer (`with-out-str`) if set, else real stdout. The default `*out*` port.\n\n    (with-out-str (%write-out \"hi\"))   ;=> \"hi\"",
        write_out);
    primitives.def(
        "%write-err",
        Arity::exact(1),
        Sig::new(vec![string], nil_ty),
        &["s"],
        "Write the ready string `s` to real stderr (never captured). The default `*err*` port.\n\n    (%write-err \"careful\")   ;~> nil, with careful written to stderr",
        write_err,
    );
    primitives.def(
        "%read-line-start",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "Ask the stdin reader thread for the next line; returns a token at once, and `[:stdin token line]` (nil at end of input) or `[:stdin-error token err]` arrives in this process's mailbox. Policy is the prelude `read-line`, which parks on the token (ADR-059).",
        read_line_start,
    );
    // `println` is Brood over `print` (std/prelude.blsp).
    primitives.def(
        "%stdout-tty?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "True when stdout is an interactive terminal (false when piped or captured).",
        stdout_tty,
    );
    primitives.def(
        "%stdin-tty?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "True when stdin is an interactive terminal (false when redirected from a pipe or file). The REPL gates raw-mode line editing on this.",
        stdin_tty);
    // Output-capture surface for the `with-out-str` prelude macro: push/pop a
    // process-scoped capture buffer (captures nest; `nest mcp` wraps each handler in
    // one). Rust = mechanism, the macro = policy.
    primitives.def(
        "%capture-begin",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        &[],
        "",
        capture_begin,
    );
    primitives.def(
        "%capture-take",
        Arity::exact(0),
        Sig::new(vec![], any),
        &[],
        "",
        capture_take,
    );
}

/// Start capturing the current process's output into a fresh buffer. While active,
/// `print` / terminal output ([`write_term_bytes`]) appends there instead of real
/// stdout — and so does output from any process this one `spawn`s (the capture is
/// **process-scoped and inherited**, living in the process `Ctx`; see
/// `scheduler::begin_capture`). The in-browser playground wraps each snippet in one.
/// Pair with [`take_captured_stdout`].
pub fn begin_stdout_capture() {
    crate::process::begin_capture();
}

/// Stop capturing and return what was written since [`begin_stdout_capture`] —
/// `Some(text)` (possibly empty) if capture was active, `None` otherwise.
pub fn take_captured_stdout() -> Option<String> {
    crate::process::take_capture()
}

/// If a capture is active on the current process, append `s` to it and return
/// `true`; otherwise `false`. The single divert point shared by `print` and
/// `write_term_bytes`.
pub(super) fn capture_write(s: &str) -> bool {
    crate::process::capture_append(s)
}

/// `(%capture-begin)` — push a fresh output-capture buffer (see
/// [`begin_stdout_capture`]). The low half of the `with-out-str` macro; pairs with
/// `%capture-take`. Captures nest, so this composes with an outer MCP capture.
pub(super) fn capture_begin(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    begin_stdout_capture();
    Ok(Value::nil())
}

/// `(%capture-take)` — pop the current capture buffer and return its text as a
/// string (empty string if nothing was written), or `nil` if no capture was active
/// (see [`take_captured_stdout`]). The high half of the `with-out-str` macro.
pub(super) fn capture_take(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(match take_captured_stdout() {
        Some(s) => heap.alloc_string(&s),
        None => Value::nil(),
    })
}

/// Write `s` to real stdout the way a well-behaved Unix tool does. A **broken
/// pipe** (the downstream consumer closed — `brood … | head`) is not a program
/// error: the `print!` macro would panic on it with a Rust backtrace + crash
/// dump (every observed `failed printing to stdout: Broken pipe` crash bottoms
/// out here), so instead we restore the terminal and exit quietly, exactly as
/// the default SIGPIPE disposition would. Any other failure — a full disk under
/// `brood report.blsp > out.txt` — is the caller's error: dropping it made the run
/// exit 0 with its output missing (KI-244).
pub(super) fn write_stdout(s: &str) -> Result<(), LispError> {
    use std::io::Write;
    let mut out = std::io::stdout();
    match out.write_all(s.as_bytes()).and_then(|_| out.flush()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            restore_terminal_on_exit();
            std::process::exit(0);
        }
        Err(e) => Err(LispError::runtime(format!("cannot write to stdout: {e}"))),
    }
}

/// Write `s` to real stderr the way `write_stdout` writes stdout: a **broken
/// pipe** (the downstream consumer closed — `nest check … | head`) is not a
/// program error. The `eprint!` macro would panic on it with a Rust backtrace +
/// crash dump (every observed `failed printing to stderr: Broken pipe` crash
/// bottoms out in a bare `eprint!`/`eprintln!`), so instead we restore the
/// terminal and exit quietly, exactly as the default SIGPIPE disposition would.
/// Any other write/flush failure is dropped: unlike stdout, stderr carries no result,
/// and an error raised for it would be reported to the same broken stream.
pub(super) fn write_stderr(s: &str) {
    use std::io::Write;
    let mut err = std::io::stderr();
    if let Err(e) = err.write_all(s.as_bytes()).and_then(|_| err.flush())
        && e.kind() == std::io::ErrorKind::BrokenPipe
    {
        restore_terminal_on_exit();
        std::process::exit(0);
    }
    // Other errors: nothing useful to do from a print primitive; drop it.
}

pub(super) fn eprint(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let args = realize_seqviews(heap, env, args)?;
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    write_stderr(&parts.join(" "));
    Ok(Value::nil())
}

/// `(%render & xs)` — the space-joined display forms of the arguments as a single
/// string (no output). The rendering half of `print`, split out so Brood's
/// `print`/`println` — which route the result through the dynamic `*out*` port —
/// hand a non-stdout sink (a buffer, a process) the exact text stdout would show.
pub(super) fn render(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    // One buffer for every display form (no per-argument `String`, no join copy), handed
    // to the heap without another copy.
    let args = realize_seqviews(heap, env, args)?;
    let mut text = String::new();
    for (index, &value) in args.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        printer::display_into(&mut text, heap, value);
    }
    Ok(heap.alloc_string_owned(text))
}

/// `(%write-out s)` — write the ready string `s` to the current stdout sink: the
/// active capture buffer if one is set (`with-out-str`, the MCP channel), else
/// real stdout. The write half of `print` and the default value of the `*out*`
/// port — keeping it the default is what lets `with-out-str` still capture
/// un-redirected output.
pub(super) fn write_out(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "%write-out", arg(args, 0))?;
    if !capture_write(&s) {
        write_stdout(&s)?;
    }
    Ok(Value::nil())
}

/// `(%write-err s)` — write the ready string `s` to real stderr (never captured,
/// matching `eprint`). The default value of the `*err*` port.
pub(super) fn write_err(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "%write-err", arg(args, 0))?;
    write_stderr(&s);
    Ok(Value::nil())
}

/// `(stdout-tty?)` — true when stdout is an interactive terminal, false when it's
/// captured (a pipe, a file, `cargo test`). The test framework uses this to emit
/// ANSI colour only when a human is watching, so captured output (what an LLM or
/// CI reads) stays clean plain text.
pub(super) fn stdout_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::boolean(std::io::stdout().is_terminal()))
}

/// `(stdin-tty?)` — true when stdin is an interactive terminal, false when it's
/// redirected (a pipe, a file). The REPL gates raw-mode line editing on this:
/// `echo … | brood` has a piped stdin (even with a TTY stdout), so it must take
/// the plain `read-line` path, not the interactive editor.
pub(super) fn stdin_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::boolean(std::io::stdin().is_terminal()))
}

/// `(%read-line-start)` — ask the stdin reader thread for the next line and return a
/// token at once; the thread later delivers `[:stdin token line]` (`line` is `nil` at EOF)
/// or `[:stdin-error token err]` to the calling process's mailbox. The Brood `read-line`
/// (`std/prelude/process.blsp`) parks on that token in a selective receive, so a process
/// waiting for terminal input holds **no scheduler worker** — ADR-059 Phase 2, and the
/// last of KI-97's untimed blocking calls.
///
/// Before this, `read-line` took the global stdin lock on the calling worker. A process
/// waiting for a line that never came (an interactive terminal nobody typed into, a pipe
/// the parent never wrote) pinned that worker for good, and a handful of them pinned the
/// pool: every other process starved with nothing amiss on their side. One reader thread
/// serialises requests in arrival order, which is also the only sensible sharing rule for
/// a single line-oriented stream.
pub(super) fn read_line_start(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let token = STDIN_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (sink, _cell) = crate::process::sink_pair(crate::process::self_pid());
    stdin_reader_send(StdinReq { token, sink }).map_err(|e| {
        LispError::runtime(format!("io/read-line: cannot start the stdin reader: {e}"))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::Int(token))
}

struct StdinReq {
    token: i64,
    sink: crate::process::MailboxSink,
}

static STDIN_TOKEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// The reader thread's request queue. `None` until the first `read-line`, and reset to
/// `None` if the thread is ever gone, so a refused spawn (EAGAIN under thread pressure —
/// KI-97 item 3) is retried by the next call rather than latched for the process's life.
static STDIN_READER: std::sync::Mutex<Option<std::sync::mpsc::Sender<StdinReq>>> =
    std::sync::Mutex::new(None);

fn stdin_reader_send(req: StdinReq) -> std::io::Result<()> {
    let mut slot = crate::core::sync::lock(&STDIN_READER);
    if let Some(tx) = slot.as_ref() {
        match tx.send(req) {
            Ok(()) => return Ok(()),
            // The thread is gone (it only exits when every sender is dropped, which
            // cannot happen while the slot holds one — but a panic would). Restart it.
            Err(std::sync::mpsc::SendError(back)) => {
                *slot = None;
                return start_stdin_reader(&mut slot, back);
            }
        }
    }
    start_stdin_reader(&mut slot, req)
}

fn start_stdin_reader(
    slot: &mut Option<std::sync::mpsc::Sender<StdinReq>>,
    first: StdinReq,
) -> std::io::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<StdinReq>();
    std::thread::Builder::new()
        .name("brood-stdin".into())
        .spawn(move || stdin_reader_loop(rx))?;
    let _ = tx.send(first);
    *slot = Some(tx);
    Ok(())
}

/// The reader thread: one blocking `read_line` per request, in request order. The thread
/// owns the stdin lock for exactly one line at a time and no worker ever touches it.
fn stdin_reader_loop(rx: std::sync::mpsc::Receiver<StdinReq>) {
    use crate::process::Message;
    use std::io::BufRead;
    for StdinReq { token, sink } in rx {
        let mut line = String::new();
        let msg = match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) => stdin_msg("stdin", token, Message::Nil),
            Ok(_) => {
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                stdin_msg("stdin", token, Message::Str(line))
            }
            Err(e) => {
                let err = LispError::runtime(format!("io/read-line: {}", e))
                    .with_code(crate::error::error_codes::FILE_IO);
                stdin_msg("stdin-error", token, crate::process::error_reason(&err))
            }
        };
        sink.emit(msg);
    }
}

fn stdin_msg(tag: &str, token: i64, payload: crate::process::Message) -> crate::process::Message {
    use crate::process::Message;
    Message::Vector(vec![
        Message::Keyword(value::intern(tag)),
        Message::Int(token),
        payload,
    ])
}
