// OS / environment / subprocess builtins — extracted from io.rs (file-organization split).
#![allow(unused_imports)]
use super::io::*;
use super::numeric::{arg, expect_int, expect_string};
use super::*;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

/// `(%getenv name)` — the value of environment variable `name` as a string, or nil
/// if it is unset. Lets Brood locate things like the user config directory.
pub(super) fn getenv(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_string(heap, "getenv", arg(args, 0))?;
    match std::env::var(&name) {
        Ok(val) => Ok(heap.alloc_string(&val)),
        Err(_) => Ok(Value::nil()),
    }
}

/// `(hostname)` — this machine's short hostname (no domain), used to qualify a
/// node name as `name@host` (ADR-073). Reads `/proc/sys/kernel/hostname`,
/// falling back to `$HOSTNAME` then `"localhost"` — never errors, since a node
/// must always get *some* identity. Long/FQDN names are had by passing an
/// already-qualified name to `node-start` (`:foo@my.fqdn`), so we don't resolve
/// the FQDN here.
pub(super) fn hostname(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let h = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "localhost".to_string());
    Ok(heap.alloc_string(&h))
}

/// `(%env-all)` — all environment variables as a `{string → string}` map.
///
/// Non-UTF-8 names and values are **lossily decoded** (invalid bytes become U+FFFD)
/// rather than skipped, so a hostile or merely unusual environment still reports the
/// variable's presence. `std::env::vars()` would *panic* on such an entry, and a
/// panic on a scheduler worker is not a Brood error: `try`/`catch` cannot see it, the
/// worker dies, and the runtime hangs. `vars_os` is the only version of this that a
/// Brood program can survive.
pub(super) fn env_all(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let env: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let pairs: Vec<(Value, Value)> = env
        .iter()
        .map(|(k, v)| (heap.alloc_string(k), heap.alloc_string(v)))
        .collect();
    Ok(heap.map_from_pairs(pairs))
}

/// `(%argv)` — command-line arguments as a vector of strings, including argv[0].
///
/// Lossy like `%env-all`, and for the same reason: `std::env::args()` panics on a
/// non-UTF-8 argument. The plain `brood`/`nest` CLIs happen to be shielded (clap's
/// `parse()` rejects non-UTF-8 argv before any Brood code runs), but a **bundled**
/// app (`nest release`, ADR-038) boots *before* clap and never runs it at all — so
/// this primitive cannot rely on someone else having validated argv.
pub(super) fn argv_builtin(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let vals: Vec<Value> = args.iter().map(|a| heap.alloc_string(a)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(%os-type)` — the current OS as a keyword: `:linux`, `:macos`, or `:windows`.
pub(super) fn os_type_builtin(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    #[cfg(target_os = "linux")]
    return Ok(Value::keyword(value::intern("linux")));
    #[cfg(target_os = "macos")]
    return Ok(Value::keyword(value::intern("macos")));
    #[cfg(target_os = "windows")]
    return Ok(Value::keyword(value::intern("windows")));
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return Ok(Value::keyword(value::intern("unknown")));
}

/// `(%os-cmd prog args)` — run `prog` with `args` (list or vector of strings),
/// capturing stdout and stderr. Returns `{:stdout s :stderr s :exit n}`.
pub(super) fn os_cmd(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "%os-cmd", arg(args, 0))?;
    let mut cmd = std::process::Command::new(&prog);
    if args.len() > 1 {
        let raw = heap.seq_items(arg(args, 1))?;
        for a in &raw {
            cmd.arg(expect_string(heap, "%os-cmd", *a)?);
        }
    }
    let output = cmd.output().map_err(|e| {
        LispError::runtime(format!("%os-cmd: {prog}: {e}"))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
    })?;
    let stdout = heap.alloc_string(&String::from_utf8_lossy(&output.stdout));
    let stderr = heap.alloc_string(&String::from_utf8_lossy(&output.stderr));
    let exit_code = output.status.code().unwrap_or(-1) as i64;
    let kw = |k: &'static str| Value::keyword(value::intern(k));
    Ok(heap.map_from_pairs(vec![
        (kw("stdout"), stdout),
        (kw("stderr"), stderr),
        (kw("exit"), Value::int(exit_code)),
    ]))
}

/// `(%os-cmd-stdin prog args stdin-str)` — like `%os-cmd` but writes `stdin-str` to the
/// child's stdin (pipe closed after writing → EOF); used by the git porcelain to pipe
/// patch text to `git apply -` instead of writing a temp file.
///
/// The write runs on its own thread so **both directions make progress**. Writing the
/// whole input before reading any output deadlocks the moment the child emits more than
/// one pipe buffer (~64 KiB) while still being fed: the child blocks writing stdout,
/// we block writing stdin, and neither ever moves. That is not a slow call — it is a
/// permanently pinned scheduler worker that no timeout or `try` can recover. `stdin-str`
/// is therefore unbounded in size, and so is the child's output.
pub(super) fn os_cmd_stdin(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write;
    let prog = expect_string(heap, "%os-cmd-stdin", arg(args, 0))?;
    let mut cmd = std::process::Command::new(&prog);
    if args.len() > 1 {
        let raw = heap.seq_items(arg(args, 1))?;
        for a in &raw {
            cmd.arg(expect_string(heap, "%os-cmd-stdin", *a)?);
        }
    }
    let stdin_str = expect_string(heap, "%os-cmd-stdin", arg(args, 2))?.to_string();
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        LispError::runtime(format!("%os-cmd-stdin: {prog}: {e}"))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
    })?;
    // Feed stdin from a separate thread; `wait_with_output` drains stdout and stderr
    // concurrently, so all three pipes move at once. A child that exits early makes
    // the write fail with EPIPE, which ends the thread — the same "we tried" outcome
    // the inline write had.
    let writer = child.stdin.take().map(|mut stdin_pipe| {
        std::thread::spawn(move || {
            let _ = stdin_pipe.write_all(stdin_str.as_bytes());
            // stdin_pipe dropped here → EOF sent to child
        })
    });
    let output = child.wait_with_output().map_err(|e| {
        LispError::runtime(format!("%os-cmd-stdin: {prog}: {e}"))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
    })?;
    // The child is gone, so the write has either finished or hit EPIPE; joining is a
    // formality that keeps the thread from outliving the call.
    if let Some(handle) = writer {
        let _ = handle.join();
    }
    let stdout = heap.alloc_string(&String::from_utf8_lossy(&output.stdout));
    let stderr = heap.alloc_string(&String::from_utf8_lossy(&output.stderr));
    let exit_code = output.status.code().unwrap_or(-1) as i64;
    let kw = |k: &'static str| Value::keyword(value::intern(k));
    Ok(heap.map_from_pairs(vec![
        (kw("stdout"), stdout),
        (kw("stderr"), stderr),
        (kw("exit"), Value::int(exit_code)),
    ]))
}

/// `(%halt code)` — terminate the process immediately with `code`, which must be a
/// POSIX exit status (0–255).
///
/// Anything outside that range is a **clean catchable error**, not a silent
/// truncation: `code as i32` turned `(%halt 4294967296)` into `exit(0)`, so a script
/// reporting failure reported success instead — the worst possible way for this to be
/// wrong, since every caller (CI, a shell, a supervisor) trusts the status.
pub(super) fn halt_builtin(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let code = expect_int(heap, "%halt", arg(args, 0))?;
    if !(0..=255).contains(&code) {
        return Err(
            LispError::runtime(format!("%halt: exit code {code} is out of range (0-255)"))
                .with_hint("a POSIX exit status is a single byte; pick a code in 0-255"),
        );
    }
    std::process::exit(code as i32);
}

// ---- interrupt (SIGINT) -----------------------------------------------------
// A REPL has to survive Ctrl-C. The default SIGINT disposition terminates the
// runtime, which at a prompt means losing the whole live image — every definition,
// every spawned process — in order to interrupt one runaway expression.
//
// Signal handling is mechanism Brood cannot express, so the kernel offers the
// smallest seam that makes it expressible and nothing more: a handler that only
// *records* that a request arrived, plus a read-and-clear accessor. Every policy
// question — who gets interrupted, what it costs, when to give up — stays in Brood
// (`std/tool/repl.blsp` runs each eval in a spawned process and `(exit pid :kill)`s
// it when the flag comes up), which is the whole point of ADR-006.
//
// Installed only on request, never by default: `brood script.blsp` must keep dying
// on Ctrl-C like any other Unix program, and a library embedding the interpreter
// must not have its host's signal disposition rewritten out from under it.

#[cfg(unix)]
static INTERRUPT_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The SIGINT handler. Runs on whichever scheduler thread the kernel picks, so it
/// does the only thing that is async-signal-safe: one relaxed atomic store. No
/// allocation, no locks, no I/O, no heap access.
#[cfg(unix)]
extern "C" fn brood_handle_sigint(_signum: libc::c_int) {
    INTERRUPT_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// `(%install-interrupt-handler)` — take over SIGINT so Ctrl-C sets a flag instead
/// of killing the runtime. Returns true when a handler was installed (false on a
/// platform without Unix signals, where the caller should keep its old behaviour).
/// Idempotent, and clears any pending flag so a stale interrupt can't fire into the
/// next thing that polls.
pub(super) fn install_interrupt_handler(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    #[cfg(unix)]
    {
        INTERRUPT_REQUESTED.store(false, std::sync::atomic::Ordering::Relaxed);
        // Via an explicit fn *pointer*: casting a fn *item* straight to an integer is
        // what `function_casts_as_integer` warns about.
        let handler: extern "C" fn(libc::c_int) = brood_handle_sigint;
        unsafe {
            libc::signal(libc::SIGINT, handler as usize as libc::sighandler_t);
        }
        Ok(Value::boolean(true))
    }
    #[cfg(not(unix))]
    {
        Ok(Value::boolean(false))
    }
}

/// `(%restore-interrupt-handler)` — restore the default SIGINT disposition (Ctrl-C
/// terminates the runtime again) and clear any pending flag. The uninstall half of
/// `%install-interrupt-handler`, for a *transient* REPL inside a longer run — a
/// script that drops into `pry` must get its normal Ctrl-C back when the pry exits,
/// or every later Ctrl-C sets a flag nobody polls and the script becomes
/// uninterruptible. Returns true when restored (false with no Unix signals).
pub(super) fn restore_interrupt_handler(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    #[cfg(unix)]
    {
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
        }
        INTERRUPT_REQUESTED.store(false, std::sync::atomic::Ordering::Relaxed);
        Ok(Value::boolean(true))
    }
    #[cfg(not(unix))]
    {
        Ok(Value::boolean(false))
    }
}

/// `(%interrupt-taken?)` — true if an interrupt has arrived since the last call,
/// **clearing** it. Read-and-clear (rather than a plain read plus a separate reset)
/// so two pollers can never both act on one Ctrl-C.
pub(super) fn interrupt_taken(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    #[cfg(unix)]
    {
        Ok(Value::boolean(
            INTERRUPT_REQUESTED.swap(false, std::sync::atomic::Ordering::Relaxed),
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(Value::boolean(false))
    }
}

/// `(run-process prog args)` — run external program `prog` with `args` (a list or
/// vector of strings), inheriting stdio, and return its exit code as an integer
/// (-1 if killed by a signal). The Emacs `call-process` analogue: the general
/// subprocess mechanism (used by the project scaffolder's `git init`).
pub(super) fn run_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let prog = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "run-process",
                "string program",
                pv,
            ))
        }
    };
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        match a {
            Value::Str(id) => argv.push(heap.string(id).to_string()),
            _ => {
                return Err(LispError::type_err(
                    "run-process: arguments must be strings",
                ))
            }
        }
    }
    match std::process::Command::new(&prog).args(&argv).status() {
        Ok(status) => Ok(Value::int(status.code().unwrap_or(-1) as i64)),
        Err(e) => Err(LispError::runtime(format!("run-process: {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
            .with_hint("check that the program is on PATH and the args are well-formed")),
    }
}
