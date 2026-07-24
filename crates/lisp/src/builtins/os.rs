// OS / environment / subprocess builtins — extracted from io.rs (file-organization split).
#![allow(unused_imports)]
use super::*;
use super::io::*;
use super::numeric::{arg, expect_int, expect_string};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};


/// `(getenv name)` — the value of environment variable `name` as a string, or nil
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
pub(super) fn env_all(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let env: Vec<(String, String)> = std::env::vars().collect();
    let pairs: Vec<(Value, Value)> = env
        .iter()
        .map(|(k, v)| (heap.alloc_string(k), heap.alloc_string(v)))
        .collect();
    Ok(heap.map_from_pairs(pairs))
}

/// `(%argv)` — command-line arguments as a vector of strings, including argv[0].
pub(super) fn argv_builtin(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let args: Vec<String> = std::env::args().collect();
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
/// child's stdin (pipe closed after writing → EOF). Safe for inputs well under 64 KiB
/// (the OS pipe buffer); used by the git porcelain to pipe patch text to `git apply -`
/// instead of writing a temp file.
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
    if let Some(mut stdin_pipe) = child.stdin.take() {
        let _ = stdin_pipe.write_all(stdin_str.as_bytes());
        // stdin_pipe dropped here → EOF sent to child
    }
    let output = child.wait_with_output().map_err(|e| {
        LispError::runtime(format!("%os-cmd-stdin: {prog}: {e}"))
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

/// `(%halt code)` — terminate the process immediately with `code`.
pub(super) fn halt_builtin(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let code = expect_int(heap, "%halt", arg(args, 0))?;
    std::process::exit(code as i32);
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
