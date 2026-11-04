//! Persistent child OS processes (ADR-104): spawn (plain or on a pty), write to stdin,
//! resize, close. Output arrives as messages in the owning mailbox; the mechanism is
//! `crate::host::subprocess`. Distinct from `processes.rs`, which is green processes.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::host::subprocess::EnvEntry;

use super::args::{arg, expect_int, expect_string};
use super::bytes::send_payload;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // Persistent child processes (ADR-104): spawn a co-process with piped stdio,
    // write its stdin, receive its stdout/stderr as `[:proc …]` mailbox messages.
    // A `Value::Subprocess` handle, local to this runtime, never sent across nodes.
    primitives.def(
        "%proc-spawn",
        Arity::range(2, 3),
        Sig::with_rest(vec![string, list_ty.union(vec_ty)], map_ty, subprocess_ty),
        &["prog", "args", "opts"],
        "Spawn prog (a string) with args (a list/vector of strings) as a persistent child process with piped stdio. An optional opts map tunes the child: :cwd (a string) sets its working directory, :env (a map of string->string) adds environment variables on top of the inherited environment, a nil value removing one; :raw-args true (Windows) puts the arguments into the command line unquoted, as cmd.exe /c needs. Its stdout/stderr arrive at the calling process as [:proc handle data] / [:proc-err handle data] messages; [:proc-exit handle code] arrives the moment the child itself exits, after the output it wrote before exiting (code is the exit status, or nil if signalled), even while a background job it started still holds its stdout; and [:proc-closed handle code] is the last message, once the child has exited AND its output is at end of file (or os/close ended it). Returns a subprocess handle. Throws if prog can't be spawned.",
        proc_spawn);
    // The same seam under a pseudo-terminal, for a child that expects to BE in a
    // terminal — a REPL, a shell, anything that asks `isatty` and drops its prompt,
    // its line editing and its unbuffered output when told no. One fd carries both
    // directions, so a pty child has no separate `[:proc-err …]` stream.
    primitives.def(
        "%pty-spawn",
        Arity::range(2, 3),
        Sig::with_rest(vec![string, list_ty.union(vec_ty)], map_ty, subprocess_ty),
        &["prog", "args", "opts"],
        "Spawn prog under a pseudo-terminal, so it behaves as it would in a terminal (prompt, line editing, unbuffered output) rather than as it does on a pipe. Same handle, messages and options as %proc-spawn, plus :cols/:rows for the initial window size (default 80x24) — except that a terminal has ONE channel, so the child's stderr arrives as [:proc handle data] too and there is no [:proc-err …]. The child gets its own session and controlling terminal, so job control and ^C reach IT. Unix only.",
        pty_spawn);
    primitives.def(
        "%pty-resize",
        Arity::exact(3),
        Sig::new(vec![subprocess_ty, int, int], nil_ty),
        &["p", "cols", "rows"],
        "Tell pty child p its terminal is now cols x rows; it sees SIGWINCH and redraws. Returns nil; throws if p is unknown, closed, or was spawned with pipes rather than a pty.",
        pty_resize);
    // How a pty child's terminal reads input (termios ECHO and ICANON, ADR-570): a
    // program reading a password turns echo off and keeps line mode, so a host that shows
    // the child's input itself — an editor's line-mode REPL — knows to read that line
    // masked instead.
    primitives.def(
        "%pty-mode",
        Arity::exact(1),
        Sig::new(vec![subprocess_ty], map_or_nil),
        &["p"],
        "How pty child p's terminal is reading input right now: {:echo bool :canonical bool} — its termios ECHO flag (the line discipline echoes what is typed) and ICANON flag (input is read a line at a time). A program reading a password (sudo, ssh, getpass) turns echo off and keeps canonical mode for that read; a program with its own line editor (readline, a REPL in raw mode) turns both off. Returns nil when p is unknown or already closed, or off Unix, where nothing is known. Throws if p was spawned with pipes rather than a pty.",
        pty_mode);
    primitives.def(
        "%os-detach",
        Arity::range(2, 3),
        Sig::with_rest(vec![string, list_ty.union(vec_ty)], map_ty, nil_ty),
        &["prog", "args", "opts"],
        "Start prog (a string) with args and let it go: its stdio is the null device, no handle is returned, and it is not stopped when the caller dies — not in the caller's process group, nor its Job Object on Windows. For handing something to another application (a browser, a file manager) that must outlive the caller. Options as %proc-spawn, plus :raw-args. Returns nil; throws if prog can't be started.",
        os_detach);
    primitives.def(
        "%proc-send",
        Arity::exact(2),
        // data is any iolist (ADR-139); string leaves are UTF-8 in text mode,
        // 0–255 codepoints in binary mode; bytes leaves go verbatim.
        Sig::new(vec![subprocess_ty, iolist], nil_ty),
        &["p", "data"],
        "Write data to subprocess p's stdin (blocking) and flush. data is any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139); a string leaf is always its UTF-8 bytes, whatever the child's mode (ADR-141). Returns nil; throws if p is unknown/closed.",
        proc_send);
    primitives.def(
        "%proc-set-binary",
        Arity::exact(2),
        Sig::new(vec![subprocess_ty, any], nil_ty),
        &["p", "on"],
        "Switch subprocess p's INBOUND decode between text mode (default) and binary mode (mirrors tcp/set-binary; outbound os/write is unaffected, ADR-141). In binary mode inbound [:proc …]/[:proc-err …] delivers data as a byte-faithful `bytes` value (not a string) — for a child speaking a binary protocol over stdio. Returns nil; throws if p is unknown/closed.",
        proc_set_binary);
    primitives.def(
        "%proc-close",
        Arity::exact(1),
        Sig::new(vec![subprocess_ty], nil_ty),
        &["p"],
        "Terminate subprocess p: kill it if still running and close its stdin. Idempotent; returns nil. [:proc-exit handle code] (if not already sent) and the final [:proc-closed handle code] still arrive at the owner, promptly: close also stops reading, so a descendant that escaped the kill cannot hold the handle open.",
        proc_close);
    primitives.def(
        "%proc-signal",
        Arity::exact(2),
        Sig::new(vec![subprocess_ty, kw], nil_ty),
        &["p", "sig"],
        "Send signal sig (:int, :term, :hup, :quit or :kill) to subprocess p's whole process group — the child and everything it started — and leave it running if it chooses to survive. :int is Ctrl-C. If it exits, [:proc-exit handle code] arrives as for any exit; until [:proc-closed …] the group can still be signalled, which reaches descendants the child left running. Returns nil; throws if p is unknown/closed or sig is not one of those. Unix only.",
        proc_signal);
}

// ----- persistent child processes (ADR-104) ----------------------------------
//
// Thin mechanism over `crate::host::subprocess`: spawn a long-lived child with piped stdio,
// write its stdin, and receive its output as `[:proc …]` mailbox messages. The
// framing/protocol policy (e.g. JSON-RPC for an LSP client) is Brood. A child is
// `Value::Subprocess(id)`. Contrast `%os-cmd`/`run-process`, which run to exit.

pub(super) fn expect_subprocess(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "subprocess",
        Value::Subprocess(id) => id,
    )
}

/// What both spawn primitives read from `(prog args opts)`: the program, its string
/// arguments, and the `:cwd` / `:env {"K" "V" …}` of the optional options map.
struct SpawnSpec {
    prog: String,
    argv: Vec<String>,
    cwd: Option<String>,
    env: Vec<EnvEntry>,
    raw_args: bool,
}

/// The `:cwd` and `:env` of a child-process options map — read the one way `os/spawn`,
/// `os/spawn-pty` and `os/cmd` share. `:env` entries go on top of the inherited
/// environment, and a nil value removes the variable. (`os/cmd` removed and `os/spawn`
/// raised on a nil, from two copies of this.)
pub(super) fn child_options(
    heap: &Heap,
    who: &str,
    opts: Value,
) -> Result<ChildOptions, LispError> {
    let Value::Map(opts) = opts else {
        return Ok(ChildOptions::default());
    };
    let cwd = match heap.map_get(opts, value::kw("cwd")) {
        Some(Value::Nil) | None => None,
        Some(v) => Some(expect_string(heap, &format!("{who} :cwd"), v)?),
    };
    let mut env = Vec::new();
    if let Some(Value::Map(e)) = heap.map_get(opts, value::kw("env")) {
        for (k, v) in heap.map_entries(e) {
            let name = expect_string(heap, &format!("{who} :env key"), k)?;
            let value = match v {
                Value::Nil => None,
                v => Some(expect_string(heap, &format!("{who} :env value"), v)?),
            };
            env.push((name, value));
        }
    }
    let raw_args = heap
        .map_get(opts, value::kw("raw-args"))
        .is_some_and(crate::eval::truthy);
    Ok(ChildOptions { cwd, env, raw_args })
}

/// A child-process options map, read: `:cwd`, `:env`, and `:raw-args` (Windows: the
/// arguments go into the command line as given, unquoted — KI-364; ignored elsewhere).
#[derive(Default)]
pub(super) struct ChildOptions {
    pub(super) cwd: Option<String>,
    pub(super) env: Vec<EnvEntry>,
    pub(super) raw_args: bool,
}

fn spawn_spec(heap: &Heap, who: &str, args: &[Value]) -> Result<SpawnSpec, LispError> {
    let prog = expect_string(heap, who, arg(args, 0))?;
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        argv.push(expect_string(heap, who, a)?);
    }
    let ChildOptions { cwd, env, raw_args } = child_options(heap, who, arg(args, 2))?;
    Ok(SpawnSpec {
        prog,
        argv,
        cwd,
        env,
        raw_args,
    })
}

pub(super) fn proc_spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let SpawnSpec {
        prog,
        argv,
        cwd,
        env,
        raw_args,
    } = spawn_spec(heap, "os/spawn", args)?;
    let owner = crate::process::self_pid();
    match crate::host::subprocess::spawn(&prog, &argv, cwd.as_deref(), &env, raw_args, owner) {
        Ok(id) => Ok(Value::subprocess(id)),
        Err(e) => Err(LispError::runtime(format!("os/spawn {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)),
    }
}

/// `(os/detach prog args opts)` — start a program and let it go (see
/// [`crate::host::subprocess::detach`]).
pub(super) fn os_detach(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let SpawnSpec {
        prog,
        argv,
        cwd,
        env,
        raw_args,
    } = spawn_spec(heap, "os/detach", args)?;
    crate::host::subprocess::detach(&prog, &argv, cwd.as_deref(), &env, raw_args).map_err(|e| {
        LispError::runtime(format!("os/detach {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
    })?;
    Ok(Value::nil())
}

/// `(pty-spawn prog args opts)` — like `proc-spawn`, but the child runs under a
/// pseudo-terminal, so it behaves the way it does in a terminal instead of the way it
/// does on a pipe. Same options plus `:cols` / `:rows` (default 80×24).
pub(super) fn pty_spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let SpawnSpec {
        prog,
        argv,
        cwd,
        env,
        raw_args: _,
    } = spawn_spec(heap, "os/spawn-pty", args)?;
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    if let Value::Map(opts) = arg(args, 2) {
        if let Some(v) = heap.map_get(opts, value::kw("cols"))
            && !matches!(v, Value::Nil)
        {
            cols = expect_int(heap, "os/spawn-pty :cols", v)?.clamp(1, 10_000) as u16;
        }
        if let Some(v) = heap.map_get(opts, value::kw("rows"))
            && !matches!(v, Value::Nil)
        {
            rows = expect_int(heap, "os/spawn-pty :rows", v)?.clamp(1, 10_000) as u16;
        }
    }
    let owner = crate::process::self_pid();
    match crate::host::subprocess::spawn_pty(&prog, &argv, cwd.as_deref(), &env, owner, cols, rows)
    {
        Ok(id) => Ok(Value::subprocess(id)),
        Err(e) => Err(LispError::runtime(format!("os/spawn-pty {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)),
    }
}

/// `(pty-resize handle cols rows)` — tell a pty child its window changed size, which
/// is how a full-screen program learns to redraw (it sees `SIGWINCH`).
pub(super) fn pty_resize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "os/resize-pty", arg(args, 0))?;
    let cols = expect_int(heap, "os/resize-pty", arg(args, 1))?.clamp(1, 10_000) as u16;
    let rows = expect_int(heap, "os/resize-pty", arg(args, 2))?.clamp(1, 10_000) as u16;
    crate::host::subprocess::pty_resize(id, cols, rows)
        .map_err(|e| LispError::runtime(format!("os/resize-pty: {}", e)))?;
    Ok(Value::nil())
}

/// `(pty-mode handle)` — `{:echo bool :canonical bool}`, how a pty child's terminal reads
/// input right now; nil when the child is gone (or off Unix), so a caller checking at a
/// prompt need not race its exit.
pub(super) fn pty_mode(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "os/pty-mode", arg(args, 0))?;
    match crate::host::subprocess::pty_input_mode(id) {
        Ok(Some(mode)) => Ok(heap.map_from_pairs(vec![
            (value::kw("echo"), Value::boolean(mode.echo)),
            (value::kw("canonical"), Value::boolean(mode.canonical)),
        ])),
        Ok(None) => Ok(Value::nil()),
        Err(e) => Err(LispError::runtime(format!("os/pty-mode: {}", e))),
    }
}

pub(super) fn proc_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "os/write", arg(args, 0))?;
    let out = send_payload(heap, "os/write", arg(args, 1))?;
    crate::host::subprocess::send(id, &out)
        .map_err(|e| LispError::runtime(format!("os/write: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "os/set-binary", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::host::subprocess::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("os/set-binary: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_signal(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "os/signal", arg(args, 0))?;
    let sig = expect!(heap, "os/signal", arg(args, 1), "keyword",
        Value::Keyword(k) => value::symbol_name_ref(k),
    )?;
    crate::host::subprocess::signal(id, sig)
        .map_err(|e| LispError::runtime(format!("os/signal: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "os/close", arg(args, 0))?;
    crate::host::subprocess::close(id);
    Ok(Value::nil())
}
