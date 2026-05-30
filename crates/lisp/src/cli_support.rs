//! Tiny CLI helpers shared between the `brood` (language) and `nest` (project)
//! binaries. Lives in the brood lib so neither crate depends on the other —
//! both already embed `brood::Interp`.
//!
//! What goes here is *mechanism* the two binaries genuinely share (error
//! formatting, common flag parsing). Anything binary-specific stays in
//! `crates/cli/` or `crates/nest/`.

use crate::error::LispError;

/// Install a panic hook that appends a full crash report (message + location +
/// backtrace) to `.brood_crash_dump` in the working directory, *in addition* to
/// the normal stderr output. A Rust panic in these binaries is almost always a
/// kernel-level fault — a use-after-GC tripwire, a heap index, a runtime invariant
/// — and the one-line stderr message often scrolls past (especially under a TUI /
/// `nest run` animation). The dump captures it durably with a backtrace
/// (`force_capture`, so it works even without `RUST_BACKTRACE`). Appends, so a
/// burst of worker-thread panics all land. Best-effort: a write failure is
/// swallowed (we never want the crash handler to itself panic).
///
/// **Caveat:** this catches Rust *panics*, not `SIGSEGV` (e.g. a coroutine
/// stack overflow) — a signal handler writing from an async-signal context is a
/// separate, much hairier mechanism, deliberately not done here.
pub fn install_crash_dump() {
    let prior = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Preserve the normal behaviour first (stderr message / default trace).
        prior(info);
        use std::io::Write;
        let bt = std::backtrace::Backtrace::force_capture();
        let when = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let thread = std::thread::current();
        let mut body = String::new();
        body.push_str("\n=== brood crash dump ===\n");
        body.push_str(&format!("when:    {when} ms since epoch\n"));
        body.push_str(&format!("thread:  {}\n", thread.name().unwrap_or("<unnamed>")));
        body.push_str(&format!("panic:   {info}\n"));
        body.push_str(&format!("backtrace:\n{bt}\n"));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(".brood_crash_dump")
        {
            if f.write_all(body.as_bytes()).is_ok() {
                // Point the user at the dump (the panic message itself already
                // went to stderr via `prior`).
                eprintln!("brood: crash report appended to .brood_crash_dump");
            }
        }
    }));
}

/// Print a one-line stderr notice if any non-default GC env knob is active, so a
/// performance benchmark can't silently measure a stressed or retuned heap. Prints
/// nothing in the normal (all-default) case, so there's zero noise on a real run.
/// Called once at binary startup. `BROOD_GC_STRESS` is the dangerous one (collect
/// at *every* safepoint — order-of-magnitude slower); the tuning knobs and trace
/// just change behaviour from the shipped defaults. (`BROOD_GC_VERIFY` is debug-only
/// and inert in a release build, but it's still worth flagging that it's set.)
pub fn warn_nondefault_gc_env() {
    const KNOBS: &[&str] = &[
        "BROOD_GC_STRESS",
        "BROOD_GC_VERIFY",
        "BROOD_GC_TRACE",
        "BROOD_GC_FLOOR",
        "BROOD_GC_TENURE",
        "BROOD_GC_MAJOR",
    ];
    let active: Vec<String> = KNOBS
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| format!("{k}={v}")))
        .collect();
    if !active.is_empty() {
        eprintln!(
            "brood: note — non-default GC config active ({}); not representative for benchmarking",
            active.join(", ")
        );
    }
}

/// Print an error as a GNU `FILE:LINE:COL: message` line (editor-parseable),
/// followed — when the file and position are known — by the offending source
/// line and a caret under the column. See `docs/tooling.md`.
pub fn report_error(e: &LispError) {
    eprintln!("{}", e.located());
    if let (Some(file), Some(pos)) = (&e.file, e.pos) {
        if let Ok(src) = std::fs::read_to_string(file) {
            if let Some(line) = src.lines().nth(pos.line.saturating_sub(1) as usize) {
                eprintln!("    {}", line);
                let pad = " ".repeat(pos.col.saturating_sub(1) as usize);
                eprintln!("    {}^", pad);
            }
        }
    }
    // Surface the fix hint (the C-style-call nudge, the deep-recursion suggestion,
    // …) — it's the actionable half of the error, and the CLI is where a human / LLM
    // most often reads it. Structured consumers (MCP / LSP) get it via `to_value_map`.
    if let Some(hint) = &e.hint {
        eprintln!("    hint: {hint}");
    }
}

/// Split CLI args into file paths and an optional concurrency cap. Accepts
/// `-j N`, `--jobs N`, `--max-parallel N`, and the `=`/joined forms (`-jN`,
/// `--max-parallel=N`). A bad value calls `exit_with(prog)` so a typo never
/// silently runs unbounded — letting each caller decide its exit message.
///
/// Callers pass `mode_flags` (e.g. `["--test", "--check", "--watch"]`) for the
/// boolean / consumed-elsewhere flags they've already stripped: appearing here
/// would mean a duplicate, which we accept as a no-op (the caller saw it
/// first). Any *other* `-*` argument at this point is a typo — we hard-error
/// with the offending token so the user sees "unknown option" instead of
/// "cannot read --foo: No such file or directory".
pub fn parse_jobs_args(prog: &str, args: Vec<String>) -> (Vec<String>, Option<usize>) {
    let mut files = Vec::new();
    let mut max_parallel = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let value = if a == "-j" || a == "--jobs" || a == "--max-parallel" {
            i += 1;
            args.get(i).cloned()
        } else if let Some(v) = a
            .strip_prefix("--max-parallel=")
            .or_else(|| a.strip_prefix("--jobs="))
        {
            Some(v.to_string())
        } else if let Some(v) = a
            .strip_prefix("-j")
            // Only the joined `-jN` form; otherwise a file like `-justfile` would
            // be misread as a flag. The explicit `=`/spaced forms still error on
            // a bad value above.
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
        {
            Some(v.to_string())
        } else if a.starts_with('-') && a != "-" {
            // Unknown `-*` arg at this point — caller's recognised flags
            // (--test/--check/--watch/etc.) have already been stripped, so
            // anything dash-prefixed left over is a typo. Reject rather than
            // treat as a file path (which surfaces as a confusing "cannot
            // read --foo" later).
            eprintln!("{prog}: unknown option {a:?}");
            std::process::exit(2);
        } else {
            files.push(a.clone());
            None
        };
        if let Some(v) = value {
            match v.parse::<usize>() {
                Ok(n) => max_parallel = Some(n),
                Err(_) => {
                    eprintln!("{prog}: {a} expects a number, got {v:?}");
                    std::process::exit(2);
                }
            }
        }
        i += 1;
    }
    (files, max_parallel)
}
