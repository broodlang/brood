// OS / environment / subprocess builtins — extracted from io.rs (file-organization split).
use super::args::{arg, expect_int, expect_string};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value, kw};
use crate::error::{LispError, LispResult};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // system / environment
    primitives.def(
        "%getenv",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        &["name"],
        "The value of environment variable name, or nil if unset.",
        getenv,
    );
    primitives.def(
        "%user-home",
        Arity::exact(1),
        Sig::new(vec![string], string.union(nil_ty)),
        &["name"],
        "The home directory of the user called name (the password database, so NSS users too), or nil when there is no such user. What a `~name/` path names.",
        user_home,
    );
    primitives.def(
        "%hostname",
        Arity::exact(0),
        Sig::nullary(string),
        &[],
        "This machine's short hostname (no domain). Used to qualify a node name as name@host.",
        hostname,
    );
    primitives.def(
        "%install-interrupt-handler",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "Take over SIGINT so Ctrl-C records a request instead of terminating the runtime; returns true when installed (false with no Unix signals). Idempotent, and clears any pending request. Opt-in, so a script keeps dying on Ctrl-C: the REPL installs it, nothing else does.",
        install_interrupt_handler);
    primitives.def(
        "%restore-interrupt-handler",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "Restore the default SIGINT disposition (Ctrl-C terminates again) and clear any pending request — the uninstall half of %install-interrupt-handler, so a transient REPL (pry) inside a script gives the script its Ctrl-C back. Returns true when restored.",
        restore_interrupt_handler);
    primitives.def(
        "%interrupt-taken?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "True if an interrupt arrived since the last call, clearing it (read-and-clear, so one Ctrl-C is acted on once). Poll this while a spawned evaluation runs and (exit pid :kill) it.",
        interrupt_taken);
    primitives.def(
        "%trap-signal",
        Arity::exact(2),
        Sig::new(vec![super::signature_types::kw, pid_or_nil], pid_or_nil),
        &["sig", "pid"],
        "Deliver signal sig (:term, :int or :hup) to local process pid as the message [:signal sig] instead of its default action; a nil pid restores the default. Returns the previous subscriber or nil. If the subscriber has exited when the signal arrives, the default action is restored and the signal re-raised, so a trap never leaves the runtime unkillable.",
        trap_signal);
    primitives.def(
        "%run-process",
        Arity::exact(2),
        Sig::new(vec![string, seq], int),
        &["prog", "args"],
        "Run external program prog with an args list, inheriting stdio; returns its exit code.",
        run_process,
    );
    primitives.def(
        "%env-all",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "All environment variables as a map of string→string.",
        env_all,
    );
    primitives.def(
        "%argv",
        Arity::exact(0),
        Sig::nullary(seq),
        &[],
        "Command-line arguments as a vector of strings (including argv[0]).",
        argv_builtin,
    );
    primitives.def(
        "%script-args",
        Arity::exact(0),
        Sig::nullary(seq),
        &[],
        "Arguments meant for this program, without the host CLI's own — everything after \
         `--` in `brood file.blsp -- a b`. For a bundled app (no host CLI), everything \
         after argv[0].",
        script_args_builtin,
    );
    primitives.def(
        "%os-type",
        Arity::exact(0),
        Sig::nullary(kw),
        &[],
        "The host OS as a keyword: :linux, :macos, or :windows.",
        os_type_builtin,
    );
    primitives.def(
        "%os-cmd",
        Arity::at_least(1),
        Sig::new(vec![string, seq, map_ty], map_ty),
        &["prog", "&", "args", "opts"],
        "Run prog (with optional args list and an opts map {:cwd :env :stdin :timeout-ms}) to completion; returns {:stdout s :stderr s :exit n}, plus :timed-out true when the timeout ended the call. The timeout bounds the WHOLE call — a grandchild still holding the output pipes is killed with the child's process group.",
        os_cmd);
    primitives.def(
        "%halt",
        Arity::exact(1),
        Sig::new(vec![int], nil_ty),
        &["code"],
        "Terminate the process with exit code. Never returns.",
        halt_builtin,
    );
    // time
    primitives.def(
        "%now",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "Wall-clock milliseconds since the Unix epoch.",
        now,
    );
    primitives.def(
        "%now-ns",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "Wall-clock nanoseconds since the Unix epoch (finer-grained than now).",
        now_ns,
    );
}

/// `(%getenv name)` — the value of environment variable `name` as a string, or nil
/// if it is unset. Lets Brood locate things like the user config directory.
pub(super) fn getenv(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_string(heap, "os/env", arg(args, 0))?;
    match std::env::var(&name) {
        Ok(val) => Ok(heap.alloc_string(&val)),
        Err(_) => Ok(Value::nil()),
    }
}

/// `(%user-home name)` — the home directory of the user called `name`, from the password
/// database (`getpwnam`, so an NSS/LDAP user resolves too), or nil when there is no such
/// user. The other half of `~`: `~/` is `$HOME`, `~name/` is this. The reentrant
/// `getpwnam_r` writes into a buffer of ours, so no lock is shared with `file/stat`'s
/// `getpwuid` (green processes run on several OS threads). Nil off unix.
pub(super) fn user_home(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_string(heap, "%user-home", arg(args, 0))?;
    match lookup_user_home(&name) {
        Some(dir) => Ok(heap.alloc_string(&dir)),
        None => Ok(Value::nil()),
    }
}

#[cfg(unix)]
fn lookup_user_home(name: &str) -> Option<String> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let mut buffer: Vec<libc::c_char> = vec![0; 1024];
    loop {
        // SAFETY: `getpwnam_r` fills `entry` with pointers into `buffer`, which outlives
        // every read of them below; `found` is null or `&mut entry`.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwnam_r(
                c_name.as_ptr(),
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut found,
            )
        };
        // the entry did not fit: try a bigger buffer, up to a bound no real entry reaches
        if status == libc::ERANGE && buffer.len() < (1 << 20) {
            buffer.resize(buffer.len() * 4, 0);
            continue;
        }
        if status != 0 || found.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        // SAFETY: `pw_dir` is a NUL-terminated string inside `buffer`, still alive here.
        let dir = unsafe { std::ffi::CStr::from_ptr(entry.pw_dir) };
        return dir
            .to_str()
            .ok()
            .filter(|dir| !dir.is_empty())
            .map(|dir| dir.to_string());
    }
}

#[cfg(not(unix))]
fn lookup_user_home(_name: &str) -> Option<String> {
    None
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

/// The arguments a host CLI decided belong to the program, not to itself.
///
/// `brood file.blsp -- --verbose x` has to route `--verbose x` somewhere: they are not
/// files to run, and a script reading raw `%argv` would have to know where its host's
/// own arguments stop — i.e. reimplement the host's parser to find the `--`. The host
/// already knows, so it says.
///
/// Unset means nobody told us, which is the honest state for a **bundled** app
/// (`nest release`, ADR-038): it boots before any CLI parsing and owns its whole argv.
/// `script_args_builtin` falls back to everything after argv[0] there.
static SCRIPT_ARGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Record the arguments intended for the program being run, for `(%script-args)`.
///
/// Called once by a host CLI before it runs anything. Later calls are ignored rather
/// than an error: the value is a fact about this process's invocation, so the first
/// writer — the one that parsed the real argv — is the authority.
pub fn set_script_args(args: Vec<String>) {
    let _ = SCRIPT_ARGS.set(args);
}

/// `(%script-args)` — the arguments meant for this program, without the host CLI's own.
///
/// For `brood file.blsp -- a b` that is `["a" "b"]`. When no host recorded anything (a
/// bundled app, which boots before any CLI), it is everything after argv[0], so a bundled
/// program and a script see the same shape.
pub(super) fn script_args_builtin(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let args: Vec<String> = match SCRIPT_ARGS.get() {
        Some(recorded) => recorded.clone(),
        None => std::env::args_os()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
    };
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

/// `(%os-cmd prog args opts)` — run `prog` with `args` (list or vector of strings) to
/// completion, capturing stdout and stderr: `{:stdout s :stderr s :exit n}`, plus
/// `:timed-out true` when a timeout ended it. `opts` (a map, or absent) takes:
///
/// - `:cwd` — the working directory, passed to the child so this process's cwd is never
///   touched and two calls in different directories cannot race (the alternative, `cd` in
///   a `sh -c` string, re-introduces a shell and its quoting purely to change directory).
/// - `:env` — `{"NAME" "value"}` added to the inherited environment; a nil value UNSETS
///   the name. What makes a non-interactive child possible: `GIT_TERMINAL_PROMPT=0` keeps
///   a background `git fetch` from asking for a password nobody can see.
/// - `:stdin` — a string written to the child's stdin, then EOF. Without it the child's
///   stdin is `/dev/null`, never ours.
/// - `:timeout-ms` — a deadline over the WHOLE call: the child exiting AND both output
///   pipes reaching end-of-file. Past it the child's process group is killed and the call
///   returns at once with the output read so far, `:timed-out true`, and `:exit` the
///   child's own status (`-1` when the kill ended it; its real code when it had already
///   exited and only a grandchild — `sleep` under `sh -c "sleep 9 &"`, `ssh` under `git` —
///   was still holding the pipes). The child runs in its own process group, so the kill
///   reaches such a grandchild too.
///
/// Every pipe is serviced together (one `poll` loop on Unix, a thread per pipe
/// elsewhere), so neither direction can fill and wedge the other: writing all of stdin
/// before reading any output deadlocks the moment the child emits more than one pipe
/// buffer (~64 KiB) while still being fed.
pub(super) fn os_cmd(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::process::{Command, Stdio};
    let prog = expect_string(heap, "%os-cmd", arg(args, 0))?;
    let mut cmd = Command::new(&prog);
    let mut argv = Vec::new();
    if args.len() > 1 && !matches!(arg(args, 1), Value::Nil) {
        for a in heap.seq_items(arg(args, 1))? {
            argv.push(expect_string(heap, "%os-cmd", a)?);
        }
    }
    cmd.args(&argv);
    // a responsive process is told it waited, or refused the wait (ADR-517)
    let blocking = crate::process::responsive::enter_blocking(heap, "os/cmd", || {
        crate::process::responsive::command_detail(&prog, &argv)
    })?;
    let mut stdin_text: Option<String> = None;
    let mut timeout: Option<std::time::Duration> = None;
    let (cwd, env) = super::subprocesses::child_options(heap, "os/cmd", arg(args, 2))?;
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    crate::host::subprocess::apply_env(&mut cmd, &env);
    crate::host::subprocess::reset_inherited_signals(&mut cmd);
    if let Value::Map(opts) = arg(args, 2) {
        if let Some(v) = heap.map_get(opts, kw("stdin"))
            && !matches!(v, Value::Nil)
        {
            stdin_text = Some(expect_string(heap, "os/cmd :stdin", v)?);
        }
        if let Some(v) = heap.map_get(opts, kw("timeout-ms"))
            && !matches!(v, Value::Nil)
        {
            let ms = expect_int(heap, "os/cmd :timeout-ms", v)?;
            timeout = Some(std::time::Duration::from_millis(ms.max(0) as u64));
        }
    }
    cmd.stdin(if stdin_text.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let fail = |e: std::io::Error| {
        LispError::runtime(format!("%os-cmd: {prog}: {e}"))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
    };
    // The child can run for as long as it likes, and this worker runs nothing else until it
    // finishes: mark it dirty-blocked (ADR-100 §7.4) for the wait, so its queue is moved to
    // a live worker and a process woken meanwhile is not routed to it. Without this, two
    // `os/cmd` calls — or one beside a tree-walked `receive` — on a two-worker pool left no
    // worker to run anything else until a command returned (KI-280).
    //
    // The child dies with its caller: the wait checks the calling process's exit signal
    // between polls, and a killed caller kills the child's group and dies itself, as it
    // would in a `receive`. Without it a cancelled task (`task/cancel`) left its command
    // running to the end on a worker nothing would read from again.
    let (status, stdout_bytes, stderr_bytes, stop) = {
        let _dirty = crate::process::dirty_block();
        let child = cmd.spawn().map_err(fail)?;
        os_cmd_collect(
            child,
            stdin_text,
            timeout,
            crate::process::capture_kill_pending,
        )
        .map_err(fail)?
    };
    if let Some(call) = blocking {
        call.report(heap);
    }
    if stop == Some(Stop::Killed) {
        return Err(LispError::kill_signal());
    }
    let stdout = heap.alloc_string(&String::from_utf8_lossy(&stdout_bytes));
    let stderr = heap.alloc_string(&String::from_utf8_lossy(&stderr_bytes));
    let exit_code = status.code().unwrap_or(-1) as i64;
    let mut pairs = vec![
        (kw("stdout"), stdout),
        (kw("stderr"), stderr),
        (kw("exit"), Value::int(exit_code)),
    ];
    if stop == Some(Stop::Deadline) {
        pairs.push((kw("timed-out"), Value::Bool(true)));
    }
    Ok(heap.map_from_pairs(pairs))
}

/// Why `%os-cmd` stopped waiting before its child finished, and killed it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stop {
    /// `:timeout-ms` ran out.
    Deadline,
    /// The calling process was sent an exit signal: the child dies with it.
    Killed,
}

/// The longest one wait of `%os-cmd` runs before it looks for its caller's exit signal
/// again — how long a killed caller's child can outlive it.
const KILL_CHECK: std::time::Duration = std::time::Duration::from_millis(20);

/// What `%os-cmd` collects from a finished (or killed) child: its status, stdout,
/// stderr, and what stopped the call early, if anything did.
type OsCmdOutcome = (std::process::ExitStatus, Vec<u8>, Vec<u8>, Option<Stop>);

/// What, if anything, stops the wait now: the deadline passed, or `killed` says the caller
/// is dying.
fn os_cmd_stop(deadline: Option<std::time::Instant>, killed: fn() -> bool) -> Option<Stop> {
    if killed() {
        Some(Stop::Killed)
    } else if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        Some(Stop::Deadline)
    } else {
        None
    }
}

/// Feed `%os-cmd`'s child its stdin and read both output pipes in ONE `poll` loop
/// bounded by the deadline and by `killed` (asked at least every [`KILL_CHECK`]), so no
/// helper thread exists to outlive the call: a grandchild that keeps a pipe open past
/// either is killed with the group, and one that escaped the group (`setsid`) only finds
/// our ends closed.
///
/// The child is reaped LAST. Until then its pid — the process-group id — cannot be
/// reused, so the `killpg` below can only ever reach the group we created.
#[cfg(unix)]
fn os_cmd_collect(
    mut child: std::process::Child,
    stdin_text: Option<String>,
    timeout: Option<std::time::Duration>,
    killed: fn() -> bool,
) -> std::io::Result<OsCmdOutcome> {
    use std::io::{ErrorKind, Read, Write};
    use std::os::fd::AsRawFd;
    use std::time::Instant;
    fn set_nonblocking(fd: libc::c_int) {
        // SAFETY: fcntl on a pipe descriptor this function owns for its whole duration.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }
    let deadline = timeout.map(|limit| Instant::now() + limit);
    let stdin_bytes = stdin_text.unwrap_or_default().into_bytes();
    let mut stdin_written = 0usize;
    // An empty `:stdin` is EOF at once: dropping the pipe here closes it.
    let mut stdin_pipe = child.stdin.take().filter(|_| !stdin_bytes.is_empty());
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    if let Some(pipe) = &stdin_pipe {
        set_nonblocking(pipe.as_raw_fd());
    }
    if let Some(pipe) = &stdout_pipe {
        set_nonblocking(pipe.as_raw_fd());
    }
    if let Some(pipe) = &stderr_pipe {
        set_nonblocking(pipe.as_raw_fd());
    }
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stop = None;
    let mut chunk = vec![0u8; 64 * 1024];
    // Read until the pipe would block; `None` it out at end-of-file or on an error.
    fn drain_ready<R: Read>(pipe: &mut Option<R>, into: &mut Vec<u8>, chunk: &mut [u8]) {
        while let Some(reader) = pipe.as_mut() {
            match reader.read(chunk) {
                Ok(0) => *pipe = None,
                Ok(count) => into.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => return,
                Err(_) => *pipe = None,
            }
        }
    }
    while stdin_pipe.is_some() || stdout_pipe.is_some() || stderr_pipe.is_some() {
        stop = os_cmd_stop(deadline, killed);
        if stop.is_some() {
            break;
        }
        // Round up, so a sub-millisecond remainder sleeps rather than spins.
        let wait = deadline.map_or(KILL_CHECK, |deadline| {
            KILL_CHECK.min(deadline.saturating_duration_since(Instant::now()))
        });
        let wait_ms = (wait.as_millis() + 1) as libc::c_int;
        let mut descriptors = Vec::with_capacity(3);
        if let Some(pipe) = &stdin_pipe {
            descriptors.push(libc::pollfd {
                fd: pipe.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            });
        }
        for fd in [
            stdout_pipe.as_ref().map(|pipe| pipe.as_raw_fd()),
            stderr_pipe.as_ref().map(|pipe| pipe.as_raw_fd()),
        ]
        .into_iter()
        .flatten()
        {
            descriptors.push(libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            });
        }
        // SAFETY: `descriptors` is a live, correctly sized array of pollfd for the call.
        let ready = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                wait_ms,
            )
        };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            // SAFETY: as below — the group is ours until the child is reaped.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.wait();
            return Err(error);
        }
        if ready == 0 {
            continue; // the stop check at the top of the loop decides
        }
        for descriptor in &descriptors {
            if descriptor.revents == 0 {
                continue;
            }
            let fd = descriptor.fd;
            if stdin_pipe.as_ref().map(|pipe| pipe.as_raw_fd()) == Some(fd) {
                let writer = stdin_pipe.as_mut().expect("checked above");
                match writer.write(&stdin_bytes[stdin_written..]) {
                    Ok(count) => {
                        stdin_written += count;
                        if stdin_written == stdin_bytes.len() {
                            stdin_pipe = None; // EOF for the child
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            ErrorKind::WouldBlock | ErrorKind::Interrupted
                        ) => {}
                    // EPIPE from a child that exits early just ends the write
                    Err(_) => stdin_pipe = None,
                }
            } else if stdout_pipe.as_ref().map(|pipe| pipe.as_raw_fd()) == Some(fd) {
                drain_ready(&mut stdout_pipe, &mut stdout_bytes, &mut chunk);
            } else if stderr_pipe.as_ref().map(|pipe| pipe.as_raw_fd()) == Some(fd) {
                drain_ready(&mut stderr_pipe, &mut stderr_bytes, &mut chunk);
            }
        }
    }
    // Close our ends before waiting: a child blocked writing to us must see EPIPE.
    drop(stdin_pipe);
    drop(stdout_pipe);
    drop(stderr_pipe);
    // The pipes are closed but the child may still run (it closed its own output): it is
    // waited for under the same two stops, polled, since a blocking `wait` would hear
    // neither.
    let status = loop {
        if stop.is_some() {
            // SAFETY: killpg on the group `process_group(0)` made for this child; the
            // child is not reaped yet, so its pid (the group id) cannot have been reused.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            break child.wait()?;
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        stop = os_cmd_stop(deadline, killed);
        if stop.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    Ok((status, stdout_bytes, stderr_bytes, stop))
}

/// The portable fallback: a thread per pipe, the child killed at the deadline or when
/// `killed` says the caller is dying. Without process groups a grandchild holding a pipe
/// can still outlast either here.
#[cfg(not(unix))]
fn os_cmd_collect(
    mut child: std::process::Child,
    stdin_text: Option<String>,
    timeout: Option<std::time::Duration>,
    killed: fn() -> bool,
) -> std::io::Result<OsCmdOutcome> {
    use std::io::{Read, Write};
    // `Builder::spawn`, not `thread::spawn`: the OS can refuse a thread (KI-241), and the
    // child is already running, so a refusal kills and reaps it and is the call's error.
    let stdin_pipe = child.stdin.take();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let started_threads = (|| {
        let writer = match (stdin_pipe, stdin_text) {
            (Some(mut pipe), Some(text)) => Some(std::thread::Builder::new().spawn(move || {
                // EPIPE from a child that exits early just ends the write
                let _ = pipe.write_all(text.as_bytes());
            })?),
            _ => None,
        };
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::Builder::new().spawn(move || {
                let mut buffer = Vec::new();
                if let Some(mut reader) = pipe {
                    let _ = reader.read_to_end(&mut buffer);
                }
                buffer
            })
        };
        let stdout_reader = drain(stdout_pipe.map(|pipe| Box::new(pipe) as Box<dyn Read + Send>))?;
        let stderr_reader = drain(stderr_pipe.map(|pipe| Box::new(pipe) as Box<dyn Read + Send>))?;
        Ok::<_, std::io::Error>((writer, stdout_reader, stderr_reader))
    })();
    let (writer, stdout_reader, stderr_reader) = match started_threads {
        Ok(threads) => threads,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let deadline = timeout.map(|limit| std::time::Instant::now() + limit);
    let mut stop = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        stop = os_cmd_stop(deadline, killed);
        if stop.is_some() {
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stdout_bytes = stdout_reader.join().unwrap_or_default();
    let stderr_bytes = stderr_reader.join().unwrap_or_default();
    Ok((status, stdout_bytes, stderr_bytes, stop))
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

// A long-running program has to stop cleanly when its supervisor (systemd, a container
// runtime, a shell's `kill`) asks it to, and that request arrives as SIGTERM. The
// interrupt seam above is a flag a poller reads; a shutdown wants a MESSAGE, so the
// process that owns the shutdown can sit in `receive` like any other process (ADR-435).
//
// A signal handler can do almost nothing safely, and delivering to a mailbox locks. So
// the handler writes the signal number to a self-pipe — `write` is async-signal-safe —
// and one non-worker thread reads the pipe and delivers `[:signal sig]` to whoever
// trapped it (ADR-059's blocking-work-delivers-to-a-mailbox rule). Policy — what a
// shutdown stops, in what order — is Brood (`std/proc/application.blsp`).

/// The signals a Brood process may trap, by the keyword it names them with.
#[cfg(unix)]
const TRAPPABLE_SIGNALS: [(&str, libc::c_int); 5] = [
    ("term", libc::SIGTERM),
    ("int", libc::SIGINT),
    ("hup", libc::SIGHUP),
    // job control, for a full-screen terminal app that must give the terminal back before it
    // stops and repaint after it continues (ADR-586)
    ("tstp", libc::SIGTSTP),
    ("cont", libc::SIGCONT),
];

/// One trappable signal's trap.
#[cfg(unix)]
struct Trap {
    /// The pid its `[:signal sig]` goes to; 0 = not trapped.
    subscriber: u64,
    /// What the signal did before it was trapped — `SIG_DFL`, the `SIG_IGN` a `nohup`
    /// leaves on SIGHUP, the REPL's Ctrl-C handler — put back when the trap is lifted or
    /// its subscriber has exited.
    prior: Option<libc::sigaction>,
}

#[cfg(unix)]
impl Trap {
    /// Lift the trap: no subscriber, and the signal does what it did before.
    fn lift(&mut self, signum: libc::c_int) {
        self.subscriber = 0;
        if let Some(action) = self.prior.take() {
            unsafe {
                libc::sigaction(signum, &action, std::ptr::null_mut());
            }
        }
    }
}

/// Each trappable signal's trap, by its index in [`TRAPPABLE_SIGNALS`]. The handler never
/// reads it (it only writes the pipe), so a lock is safe, and it orders a trap against the
/// reader thread lifting one.
#[cfg(unix)]
static SIGNAL_TRAPS: std::sync::Mutex<[Trap; 5]> = std::sync::Mutex::new(
    [const {
        Trap {
            subscriber: 0,
            prior: None,
        }
    }; 5],
);

#[cfg(unix)]
fn signal_traps() -> std::sync::MutexGuard<'static, [Trap; 5]> {
    SIGNAL_TRAPS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The self-pipe's write end, or -1 before the first trap.
#[cfg(unix)]
static SIGNAL_PIPE_WRITE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// The pipe and its reader thread, made once; an error says why the OS refused either.
#[cfg(unix)]
static SIGNAL_PIPE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

/// The handler for a trapped signal: one `write` of the signal number, nothing else.
#[cfg(unix)]
extern "C" fn brood_forward_signal(signum: libc::c_int) {
    let fd = SIGNAL_PIPE_WRITE.load(std::sync::atomic::Ordering::Relaxed);
    if fd >= 0 {
        let byte = signum as u8;
        unsafe {
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
        }
    }
}

/// Create the self-pipe and start the thread that turns its bytes into messages.
#[cfg(unix)]
fn start_signal_pipe() -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let mut fds = [0 as libc::c_int; 2];
    // `pipe` then `FD_CLOEXEC`, not `pipe2`: macOS has no `pipe2`. The window between the
    // two only lets a child spawned at that instant inherit two idle descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    for fd in fds {
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    let [read_fd, write_fd] = fds;
    let reader = crate::process::reserve_io_thread("brood-signals").map_err(|e| e.to_string())?;
    SIGNAL_PIPE_WRITE.store(write_fd, Ordering::Relaxed);
    reader.run(move || {
        loop {
            let mut byte = 0u8;
            let read = unsafe { libc::read(read_fd, &mut byte as *mut u8 as *mut libc::c_void, 1) };
            if read < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
                continue;
            }
            if read <= 0 {
                return;
            }
            let signum = byte as libc::c_int;
            let Some(index) = TRAPPABLE_SIGNALS.iter().position(|(_, s)| *s == signum) else {
                continue;
            };
            let mut traps = signal_traps();
            let subscriber = traps[index].subscriber;
            if subscriber != 0 && crate::process::is_alive(subscriber) {
                crate::process::deliver(
                    subscriber,
                    crate::process::Message::Vector(vec![
                        crate::process::Message::Keyword(value::intern("signal")),
                        crate::process::Message::Keyword(value::intern(TRAPPABLE_SIGNALS[index].0)),
                    ]),
                );
            } else {
                // Nobody is left to act on it: the signal does what it did before it was
                // trapped, so a dead subscriber cannot make the runtime unkillable.
                traps[index].lift(signum);
                drop(traps);
                unsafe {
                    libc::raise(signum);
                }
            }
        }
    });
    Ok(())
}

/// `(%trap-signal sig pid)` — route signal `sig` to local process `pid` as `[:signal sig]`,
/// or give it back what it did before the trap when `pid` is nil. Returns the previous
/// subscriber.
pub(super) fn trap_signal(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    #[cfg(unix)]
    {
        let name = match arg(args, 0) {
            Value::Keyword(s) => value::symbol_name(s),
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "%trap-signal",
                    "a signal keyword",
                    other,
                ));
            }
        };
        let Some(index) = TRAPPABLE_SIGNALS
            .iter()
            .position(|(n, _)| *n == name.as_str())
        else {
            return Err(LispError::runtime(format!(
                "%trap-signal: cannot trap :{name} (expected :term, :int, :hup, :tstp or :cont)"
            )));
        };
        let subscriber = match arg(args, 1) {
            Value::Nil => 0,
            Value::Pid { node, id } if crate::dist::is_local(node) => id,
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "%trap-signal",
                    "a local pid or nil",
                    other,
                ));
            }
        };
        let signum = TRAPPABLE_SIGNALS[index].1;
        if subscriber != 0 {
            SIGNAL_PIPE
                .get_or_init(start_signal_pipe)
                .clone()
                .map_err(|why| {
                    LispError::runtime(format!("%trap-signal: no signal pipe: {why}"))
                })?;
        }
        let mut traps = signal_traps();
        let trap = &mut traps[index];
        let previous = trap.subscriber;
        if subscriber == 0 {
            trap.lift(signum);
        } else {
            trap.subscriber = subscriber;
            // Installed on every trap (something may have replaced the handler since), but
            // the disposition to restore is the one from before the FIRST trap.
            unsafe {
                let handler: extern "C" fn(libc::c_int) = brood_forward_signal;
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler as usize as libc::sighandler_t;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                let mut replaced: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signum, &action, &mut replaced) == 0 && trap.prior.is_none() {
                    trap.prior = Some(replaced);
                }
            }
        }
        Ok(if previous == 0 {
            Value::nil()
        } else {
            crate::process::pid_value(previous)
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (args, heap);
        Err(LispError::runtime(
            "%trap-signal: this platform has no Unix signals",
        ))
    }
}

/// `(run-process prog args)` — run external program `prog` with `args` (a list or
/// vector of strings), inheriting stdout/stderr, and return its exit code as an integer
/// (-1 if killed by a signal). The Emacs `call-process` analogue: the general
/// subprocess mechanism (used by the project scaffolder's `git init`).
///
/// **stdin is `/dev/null`, deliberately** (KI-97). `Command::status()` inherits all three
/// streams, and an inherited stdin is an unbounded, *uncatchable* block on a scheduler
/// worker: a child that reads it — `git` hitting a credential prompt is the realistic
/// case, and `std/tool/workspace.blsp` runs `git` across sibling repos — waits forever on
/// a terminal nobody is typing at. The scheduler cannot preempt a thread parked in a
/// syscall (ADR-059), so a handful of those wedge the whole ~nproc pool, and no timeout
/// or `try` can recover them. With `/dev/null` the child reads EOF and fails fast, which
/// is a diagnosable error instead of a hung runtime.
///
/// This also matches the analogue rather than departing from it: Emacs `call-process`
/// takes an INFILE and uses `/dev/null` when it is nil. A genuinely interactive child
/// needs a different primitive (one that does not run on a worker), not this one.
pub(super) fn run_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "os/run-process", arg(args, 0))?;
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        match a {
            Value::Str(id) => argv.push(heap.string(id).to_string()),
            _ => {
                return Err(LispError::type_err(
                    "os/run-process: arguments must be strings",
                ));
            }
        }
    }
    // a responsive process is told it waited, or refused the wait (ADR-517)
    let blocking = crate::process::responsive::enter_blocking(heap, "run-process", || {
        crate::process::responsive::command_detail(&prog, &argv)
    })?;
    let status = std::process::Command::new(&prog)
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .status();
    if let Some(call) = blocking {
        call.report(heap);
    }
    match status {
        Ok(status) => Ok(Value::int(status.code().unwrap_or(-1) as i64)),
        Err(e) => Err(
            LispError::runtime(format!("os/run-process: {}: {}", prog, e))
                .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
                .with_hint("check that the program is on PATH and the args are well-formed"),
        ),
    }
}

// ---------- time ----------

/// `(%now)` — wall-clock milliseconds since the Unix epoch, as an integer.
/// Subtract two readings to measure elapsed time (see `std/tool/test.blsp`).
pub(super) fn now(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ms = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::int(ms))
}

/// `(now-ns)` — wall-clock nanoseconds since the Unix epoch, as an integer.
/// The fine-grained partner to `now`; subtract two readings to time sub-
/// millisecond work that `now`'s resolution would round to zero. (i64
/// nanoseconds since 1970 stays in range until the year 2262.)
pub(super) fn now_ns(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ns = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(Value::int(ns))
}
