//! Tiny CLI helpers shared between the `brood` (language) and `nest` (project)
//! binaries. Lives in the brood lib so neither crate depends on the other —
//! both already embed `brood::Interp`.
//!
//! What goes here is *mechanism* the two binaries genuinely share (error
//! formatting, common flag parsing). Anything binary-specific stays in
//! `crates/cli/` or `crates/nest/`.

use crate::error::LispError;
use crate::Interp;

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
pub fn parse_jobs_args(
    prog: &str,
    args: Vec<String>,
) -> (Vec<String>, Option<usize>) {
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

// ---- REPL stubs (interim) --------------------------------------------------
//
// `nest repl` (per the in-flight refactor) calls these from the lib, but the
// real REPL code still lives privately in `crates/cli/src/main.rs`. Completing
// the move means pulling `repl_interactive` (rustyline) + `repl_plain` (plain
// stdio) up here, plus their helpers (`history_path`, `is_balanced`), and
// adding `rustyline` to the lib's `Cargo.toml`. That's a real architectural
// choice — `rustyline` is currently a "CLI-only UX dep" per `CLAUDE.md` — so
// we stub here for now to keep the build green; the cli's REPL keeps working
// from its private copies.

/// Stub: interactive REPL not yet moved into `cli_support`. Tells the caller
/// to use `brood` directly. Returns `Ok(())` so the caller's error path
/// doesn't fire.
pub fn repl_interactive(_interp: &mut Interp) -> Result<(), std::io::Error> {
    eprintln!(
        "nest repl: the shared REPL has not been moved into `brood::cli_support` yet — \
         run `brood` directly for an interactive REPL (the cli crate still holds its \
         own copy)."
    );
    Ok(())
}

/// Stub: plain (non-terminal) REPL not yet moved. Prints the same pointer.
pub fn repl_plain(_interp: &mut Interp) {
    eprintln!(
        "nest repl: the shared REPL has not been moved into `brood::cli_support` yet — \
         pipe Brood source through `brood` instead."
    );
}
