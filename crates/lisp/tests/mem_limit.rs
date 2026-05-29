//! Memory-limit backstop (ADR-043): the soft ceiling turns a runaway allocation
//! into a clean, catchable `E0043` at the eval safepoint instead of letting a
//! program exhaust host RAM. This is its **own test binary** on purpose — the
//! limit is a process-wide global, so setting it here can't leak into the other
//! integration tests (each `cargo test` binary is a separate process).

use brood::core::alloc;
use brood::error::ErrorKind;
use brood::Interp;

#[test]
fn parse_size_handles_suffixes() {
    assert_eq!(alloc::parse_size("1024"), Some(1024));
    assert_eq!(alloc::parse_size("512K"), Some(512 * 1024));
    assert_eq!(alloc::parse_size("64M"), Some(64 * 1024 * 1024));
    assert_eq!(alloc::parse_size("2G"), Some(2 * 1024 * 1024 * 1024));
    // A trailing `B` / `iB` is ignored, case-insensitively.
    assert_eq!(alloc::parse_size("2GiB"), Some(2 * 1024 * 1024 * 1024));
    assert_eq!(alloc::parse_size("2gb"), Some(2 * 1024 * 1024 * 1024));
    assert_eq!(alloc::parse_size("  8M  "), Some(8 * 1024 * 1024));
    assert_eq!(alloc::parse_size("0"), Some(0)); // valid → unlimited
                                                 // Garbage parses to None so the caller warns and falls back.
    assert_eq!(alloc::parse_size(""), None);
    assert_eq!(alloc::parse_size("notanumber"), None);
    assert_eq!(alloc::parse_size("12X"), None);
}

/// A runaway allocation fails with a clean, catchable `E0043` once it crosses
/// the soft limit — instead of growing unbounded and OOMing the host. The
/// interpreter stays usable afterwards (it's an ordinary error, not a crash).
///
/// `#[ignore]`d by default: this is the *only* test that deliberately drives an
/// unbounded allocation, so if the soft-limit safepoint ever regresses it OOMs
/// the host instead of failing cleanly — not something to hit unattended during
/// a routine `cargo test`. Run it deliberately, when you can watch it, with
/// `cargo test --test mem_limit -- --ignored`.
#[test]
#[ignore = "drives an unbounded allocation; run with --ignored when you can watch it (see doc comment)"]
fn soft_limit_turns_runaway_into_catchable_error() {
    // Build the prelude with no limit, *then* cap just above current usage so
    // the next chunk of allocation trips it. (GC is a no-op today, so a build
    // loop's live bytes only grow — the safepoint check is what stops it.)
    let mut interp = Interp::new();
    let headroom = 4 * 1024 * 1024; // 4 MiB
    alloc::set_soft_limit(alloc::live_bytes() + headroom);

    let runaway = "(let (build (fn (n acc) (if (= n 0) acc (build (- n 1) (cons n acc))))) \
                     (build 100000000 nil))";
    let err = interp
        .eval_str(runaway)
        .expect_err("runaway allocation should hit the soft memory limit");
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.code, Some("E0043"));

    // Clear the limit so nothing after this point is affected.
    alloc::set_soft_limit(0);

    // The same error is catchable from *within* Brood (it's a normal runtime
    // error): re-arm a tight limit, catch the runaway, and prove we recover.
    alloc::set_soft_limit(alloc::live_bytes() + headroom);
    let caught = interp
        .eval_str(
            "(try (let (build (fn (n acc) (if (= n 0) acc (build (- n 1) (cons n acc))))) \
                    (build 100000000 nil)) \
               (catch e :caught))",
        )
        .expect("memory error must be catchable");
    assert_eq!(interp.print(caught), ":caught");
    alloc::set_soft_limit(0);

    // Interpreter is still healthy after the caught memory error.
    let v = interp
        .eval_str("(+ 1 2)")
        .expect("interp usable after limit error");
    assert_eq!(interp.print(v), "3");
}
