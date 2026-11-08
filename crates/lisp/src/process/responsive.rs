//! A RESPONSIVE process (ADR-517): one whose job is to keep answering — an editor's loop,
//! a REPL's reader, an observer's window — marks itself `(proc/flag :responsive mode)`, and
//! a native call that waits on the outside world (a subprocess) is then reported or refused
//! instead of silently freezing it.
//!
//! Brood can always move such a wait off the process: a task, a port, a spawned worker. The
//! mistake is not knowing it happened. A `git` or `mix help` run inside an editor keypress
//! looks like any other call in the source and costs seconds only on someone's large
//! repository, so it is found as a frozen window, if at all. With the flag the call says so
//! itself: `:report` sends the process a message naming the call and how long it held the
//! process, after it returns; `:refuse` raises before it starts, so a test can make every
//! such call a failure.

use crate::core::heap::Heap;
use crate::core::value::{Value, kw};
use crate::error::LispError;
use std::time::Instant;

/// How a process treats a blocking native call (`(proc/flag :responsive …)`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Responsive {
    /// Not a responsive process: blocking calls are ordinary (the default).
    #[default]
    Off,
    /// Report each blocking call, after it returns, as `[:blocking-call {…}]` to the process.
    Report,
    /// Refuse each blocking call: raise `E0048` before it starts.
    Refuse,
}

/// A blocking call in a process that reports them: what it was and when it started, so its
/// report can say how long the process was held.
pub(crate) struct BlockingCall {
    native: &'static str,
    detail: String,
    started: Instant,
}

/// Called by a native that is about to wait on the world (`native` its name, `detail` a
/// short description: the program and its arguments). `Ok(None)` for an ordinary process,
/// `Ok(Some(call))` to be passed to [`BlockingCall::report`] once the wait is over, and an
/// `E0048` error for a process that refuses blocking calls. `detail` is only built when
/// the process is responsive.
pub(crate) fn enter_blocking(
    heap: &Heap,
    native: &'static str,
    detail: impl FnOnce() -> String,
) -> Result<Option<BlockingCall>, LispError> {
    match heap.proc_responsive() {
        Responsive::Off => Ok(None),
        Responsive::Report => Ok(Some(BlockingCall {
            native,
            detail: detail(),
            started: Instant::now(),
        })),
        Responsive::Refuse => Err(LispError::runtime(format!(
            "{native} would block a responsive process (proc/flag :responsive): {}",
            detail()
        ))
        .with_code(crate::error::error_codes::BLOCKED_RESPONSIVE)),
    }
}

impl BlockingCall {
    /// Tell the process what held it: `[:blocking-call {:native "os/cmd" :detail "git status"
    /// :ms 41}]` in its own mailbox, behind whatever it already has. Best-effort: a process
    /// with no mailbox (the root thread) gets nothing.
    pub(crate) fn report(self, heap: &mut Heap) {
        let ms = self.started.elapsed().as_millis() as i64;
        let native = heap.alloc_string(self.native);
        let detail = heap.alloc_string_owned(self.detail);
        let facts = heap.map_from_pairs(vec![
            (kw("native"), native),
            (kw("detail"), detail),
            (kw("ms"), Value::int(ms)),
        ]);
        let message = heap.alloc_vector2(kw("blocking-call"), facts);
        let me = crate::process::pid_value(crate::process::self_pid());
        let _ = crate::process::send(heap, me, message);
    }
}

/// The detail a subprocess call reports: the program and its arguments, as a shell would
/// show them, cut to a readable length.
pub(crate) fn command_detail(program: &str, arguments: &[String]) -> String {
    cut_detail(
        std::iter::once(program)
            .chain(arguments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// A report's detail cut to a readable length.
fn cut_detail(text: String) -> String {
    const MAX: usize = 200;
    if text.chars().count() > MAX {
        text.chars().take(MAX).collect::<String>() + "…"
    } else {
        text
    }
}

/// How long a file call must hold a responsive process before it is reported: more than a
/// frame at 60 Hz. A local read or stat takes microseconds, and a responsive process makes
/// them on purpose (an editor reads a small file or its state inside a key), so reporting
/// every one would bury the slow mount among thousands of instant calls.
pub(crate) const SLOW_FILE_CALL_MS: u128 = 20;

/// Run `body`, a file native (`native` its name, its first `paths` arguments the paths it
/// touches), as a call that MAY wait on the world (ADR-517's file addendum): a network
/// mount, a disk spun down, a FIFO with no writer. Unlike a subprocess it is usually
/// instant, so it is never refused, and it is reported only when it held the process at
/// least [`SLOW_FILE_CALL_MS`]: `[:blocking-call {:native "file/slurp" :detail PATH :ms N}]`,
/// two paths reading `from -> to`. For an ordinary process the cost is one read of the
/// heap's per-process flags before `body` runs.
pub(crate) fn file_call(
    heap: &mut Heap,
    native: &'static str,
    args: &[Value],
    paths: usize,
    body: impl FnOnce(&mut Heap) -> crate::error::LispResult,
) -> crate::error::LispResult {
    if heap.proc_responsive() == Responsive::Off {
        return body(heap);
    }
    let started = Instant::now();
    let value = body(heap);
    if started.elapsed().as_millis() >= SLOW_FILE_CALL_MS {
        let detail = args
            .iter()
            .take(paths)
            .map(|path| {
                crate::builtins::args::expect_string(heap, native, *path).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" -> ");
        BlockingCall {
            native,
            detail: cut_detail(detail),
            started,
        }
        .report(heap);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_reads_as_a_shell_shows_it_and_is_cut_when_long() {
        assert_eq!(
            command_detail("git", &["status".into(), "-z".into()]),
            "git status -z"
        );
        let long = command_detail("x", &["y".repeat(300)]);
        assert_eq!(long.chars().count(), 201);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn an_ordinary_process_is_neither_reported_nor_refused() {
        let heap = Heap::new();
        assert!(matches!(
            enter_blocking(&heap, "os/cmd", || unreachable!()),
            Ok(None)
        ));
    }

    #[test]
    fn a_refusing_process_is_refused_with_e0048_naming_the_call() {
        let mut heap = Heap::new();
        heap.set_proc_responsive(Responsive::Refuse);
        let error = enter_blocking(&heap, "os/cmd", || "mix help".into())
            .err()
            .expect("a refusing process let a blocking call through");
        assert_eq!(
            error.code,
            Some(crate::error::error_codes::BLOCKED_RESPONSIVE)
        );
        assert!(error.to_string().contains("os/cmd") && error.to_string().contains("mix help"));
    }
}
