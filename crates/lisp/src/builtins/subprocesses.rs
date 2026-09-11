//! Persistent child OS processes (ADR-104): spawn (plain or on a pty), write to stdin,
//! resize, close. Output arrives as messages in the owning mailbox; the mechanism is
//! `crate::host::subprocess`. Distinct from `processes.rs`, which is green processes.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::bytes::send_payload;
use super::numeric::{arg, expect_int, expect_string};

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
        "Spawn prog (a string) with args (a list/vector of strings) as a persistent child process with piped stdio. An optional opts map tunes the child: :cwd (a string) sets its working directory, :env (a map of string->string) adds environment variables on top of the inherited environment. Its stdout/stderr arrive at the calling process as [:proc handle data] / [:proc-err handle data] messages, and [:proc-closed handle code] on exit (code is the exit status, or nil if signalled). Returns a subprocess handle. Throws if prog can't be spawned.",
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
        "Terminate subprocess p: kill it if still running and close its stdin. Idempotent; returns nil. The final [:proc-closed handle code] still arrives at the owner.",
        proc_close);
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

pub(super) fn proc_spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "proc-spawn", arg(args, 0))?;
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        argv.push(expect_string(heap, "proc-spawn", a)?);
    }
    // Optional 3rd argument: an options map `{:cwd "dir" :env {"K" "V" …}}`.
    let mut cwd: Option<String> = None;
    let mut env: Vec<(String, String)> = Vec::new();
    if let Value::Map(opts) = arg(args, 2) {
        if let Some(v) = heap.map_get(opts, Value::keyword(value::intern("cwd"))) {
            if !matches!(v, Value::Nil) {
                cwd = Some(expect_string(heap, "proc-spawn :cwd", v)?);
            }
        }
        if let Some(Value::Map(e)) = heap.map_get(opts, Value::keyword(value::intern("env"))) {
            for (k, v) in heap.map_entries(e) {
                env.push((
                    expect_string(heap, "proc-spawn :env key", k)?,
                    expect_string(heap, "proc-spawn :env value", v)?,
                ));
            }
        }
    }
    let owner = crate::process::self_pid();
    match crate::host::subprocess::spawn(&prog, &argv, cwd.as_deref(), &env, owner) {
        Ok(id) => Ok(Value::subprocess(id)),
        Err(e) => Err(LispError::runtime(format!("proc-spawn {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)),
    }
}

/// `(pty-spawn prog args opts)` — like `proc-spawn`, but the child runs under a
/// pseudo-terminal, so it behaves the way it does in a terminal instead of the way it
/// does on a pipe. Same options plus `:cols` / `:rows` (default 80×24).
pub(super) fn pty_spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "pty-spawn", arg(args, 0))?;
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        argv.push(expect_string(heap, "pty-spawn", a)?);
    }
    let mut cwd: Option<String> = None;
    let mut env: Vec<(String, String)> = Vec::new();
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    if let Value::Map(opts) = arg(args, 2) {
        if let Some(v) = heap.map_get(opts, Value::keyword(value::intern("cwd"))) {
            if !matches!(v, Value::Nil) {
                cwd = Some(expect_string(heap, "pty-spawn :cwd", v)?);
            }
        }
        if let Some(Value::Map(e)) = heap.map_get(opts, Value::keyword(value::intern("env"))) {
            for (k, v) in heap.map_entries(e) {
                env.push((
                    expect_string(heap, "pty-spawn :env key", k)?,
                    expect_string(heap, "pty-spawn :env value", v)?,
                ));
            }
        }
        if let Some(v) = heap.map_get(opts, Value::keyword(value::intern("cols"))) {
            if !matches!(v, Value::Nil) {
                cols = expect_int(heap, "pty-spawn :cols", v)?.clamp(1, 10_000) as u16;
            }
        }
        if let Some(v) = heap.map_get(opts, Value::keyword(value::intern("rows"))) {
            if !matches!(v, Value::Nil) {
                rows = expect_int(heap, "pty-spawn :rows", v)?.clamp(1, 10_000) as u16;
            }
        }
    }
    let owner = crate::process::self_pid();
    match crate::host::subprocess::spawn_pty(&prog, &argv, cwd.as_deref(), &env, owner, cols, rows)
    {
        Ok(id) => Ok(Value::subprocess(id)),
        Err(e) => Err(LispError::runtime(format!("pty-spawn {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)),
    }
}

/// `(pty-resize handle cols rows)` — tell a pty child its window changed size, which
/// is how a full-screen program learns to redraw (it sees `SIGWINCH`).
pub(super) fn pty_resize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "pty-resize", arg(args, 0))?;
    let cols = expect_int(heap, "pty-resize", arg(args, 1))?.clamp(1, 10_000) as u16;
    let rows = expect_int(heap, "pty-resize", arg(args, 2))?.clamp(1, 10_000) as u16;
    crate::host::subprocess::pty_resize(id, cols, rows)
        .map_err(|e| LispError::runtime(format!("pty-resize: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-send", arg(args, 0))?;
    let out = send_payload(heap, "proc-send", arg(args, 1))?;
    crate::host::subprocess::send(id, &out)
        .map_err(|e| LispError::runtime(format!("proc-send: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-set-binary", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::host::subprocess::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("proc-set-binary: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-close", arg(args, 0))?;
    crate::host::subprocess::close(id);
    Ok(Value::nil())
}
