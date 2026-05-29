//! A process-wide allocator that counts bytes, so Brood can report how much
//! memory a piece of work used — and *cap* it, so a runaway (or hostile) program
//! can't exhaust host RAM and freeze the machine. It wraps the system allocator
//! and keeps two running totals — current live bytes and the high-water mark —
//! updated on every (de)allocation with relaxed atomics (cheap; ordering between
//! the two counters doesn't matter, only their individual values).
//!
//! Installed as `#[global_allocator]` in `lib.rs`, so it covers *every* Rust
//! allocation in the process (the interpreter included), not just Brood values.
//! For "how much memory did this run use," that whole-process number is the one
//! you want. The `(mem-bytes)` / `(mem-peak)` primitives in `builtins.rs` read
//! these counters. Dependency-free (std only), per ADR-005.
//!
//! ## Memory limits (ADR-043)
//!
//! Two tiers, both off by default (`0` = unlimited) and both *process-wide* (not
//! per green-process — we only account bytes process-wide today; per-process
//! limits are deferred, ADR-011):
//!
//! - **Hard limit** ([`set_hard_limit`]): enforced *here*, in `alloc`/`realloc`.
//!   An allocation that would cross it returns null, so Rust's OOM handler aborts
//!   the whole brood process. Ungraceful (kills every green process) but it is
//!   the backstop that guarantees the *host* survives any allocation pattern,
//!   including a single huge allocation between eval safepoints.
//! - **Soft limit** ([`soft_limit_hit`]): *not* enforced here — checked at the
//!   eval safepoint (`eval/mod.rs`), which raises a clean, catchable `LispError`
//!   (`E0043`). Set below the hard limit so a runaway *loop* fails gracefully
//!   (only the offending process dies / `try`-`catch` can recover) long before
//!   the hard abort fires. The single-shot giant allocation is the only case
//!   that reaches the hard limit.
//!
//! Normal `brood file.blsp` runs and the REPL stay unlimited unless
//! `BROOD_MEM_LIMIT` is set ([`init_limits_from_env`]); the test runners default
//! both on ([`init_limits_with_default`]) so an adversarial test can't take the
//! machine down.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0); // bytes currently allocated
static PEAK: AtomicUsize = AtomicUsize::new(0); // high-water mark of LIVE
static HARD_LIMIT: AtomicUsize = AtomicUsize::new(0); // 0 = unlimited; abort if crossed
static SOFT_LIMIT: AtomicUsize = AtomicUsize::new(0); // 0 = unlimited; safepoint raises if crossed

/// Default ceiling the *test runners* apply when neither env var is set
/// (`brood --test`, `nest test`, the `cargo test` Brood suite). Its job is to
/// **GUARANTEE THE HOST SURVIVES a test run** — it is *not* a precise working-set
/// budget (that's impossible while the tracing GC is still a no-op, the M1
/// migration: the bump allocator never reclaims, so even legitimate bounded work
/// accumulates). The number is chosen *below* a typical dev machine's RAM so a run
/// whose allocation grows without bound fails with a clean, catchable `E0043`
/// (then the hard abort) long before the OS OOM-killer or a hard freeze can fire.
///
/// **Never default this to `0`/unlimited.** The GC doesn't reclaim yet, so an
/// unbounded run will eat all host RAM — an unlimited default once OOM-froze the
/// machine. The cap machinery is still opt-out per run via `BROOD_MEM_LIMIT`.
///
/// **5 GiB hard / 4 GiB soft.** With the test runner now hibernating between steps
/// (`std/test.blsp`, the Stage-A block — each step flips the runner's arena so its
/// transients are reclaimed), the whole project suite peaks ~1.1 GiB (down from
/// ~4 GiB tripping this cap, and ~18 GiB before isolated units ran in droppable
/// processes / OOM-froze the host). 5/4 GiB leaves ample headroom while staying well
/// under host RAM. Tighten once automatic collection (Stage B, `docs/memory-review.md`)
/// lands and the hibernate scaffold is removed. A single `(sum-to 100000 0)` tail
/// loop still holds ~60 MiB unreclaimed without GC (every `(+ …)` is a prelude Brood
/// call allocating env frames per iteration; see `docs/devlog.md`).
pub const TEST_DEFAULT_HARD: usize = 5 * 1024 * 1024 * 1024; // 5 GiB
/// Soft default for the test runners — 4 GiB, so a runaway/accumulating run fails
/// *cleanly* (catchable `E0043`) before the hard abort and far below host RAM.
pub const TEST_DEFAULT_SOFT: usize = 4 * 1024 * 1024 * 1024; // 4 GiB

/// System allocator wrapper that tallies bytes in/out and enforces [`HARD_LIMIT`].
pub struct Counting;

fn record_alloc(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

/// Would committing `size` more bytes cross the hard limit? A relaxed load + a
/// saturating compare; when no limit is set (the common case) it's a single load
/// of a zero and an early return. The check races slightly under the worker pool
/// (another thread may commit between the load and the alloc), but the limit is a
/// safety backstop, not byte-exact accounting — a small overshoot is fine.
#[inline]
fn would_exceed_hard(size: usize) -> bool {
    let limit = HARD_LIMIT.load(Ordering::Relaxed);
    limit != 0 && LIVE.load(Ordering::Relaxed).saturating_add(size) > limit
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if would_exceed_hard(layout.size()) {
            // Null tells Rust the allocation failed; the default handler then
            // aborts the process. The host survives; the brood process doesn't.
            return std::ptr::null_mut();
        }
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Only a *growth* can cross the limit; a shrink is always allowed.
        if new_size > layout.size() && would_exceed_hard(new_size - layout.size()) {
            return std::ptr::null_mut();
        }
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            let old = layout.size();
            if new_size >= old {
                record_alloc(new_size - old);
            } else {
                LIVE.fetch_sub(old - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

/// Bytes currently allocated across the whole process.
pub fn live_bytes() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// The largest [`live_bytes`] has ever been (since process start).
pub fn peak_bytes() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// The hard ceiling in bytes (`0` = unlimited).
pub fn hard_limit() -> usize {
    HARD_LIMIT.load(Ordering::Relaxed)
}

/// The soft ceiling in bytes (`0` = unlimited).
pub fn soft_limit() -> usize {
    SOFT_LIMIT.load(Ordering::Relaxed)
}

/// Set the hard ceiling (`0` disables). Enforced in `alloc`/`realloc` — crossing
/// it aborts the process. See module docs.
pub fn set_hard_limit(bytes: usize) {
    HARD_LIMIT.store(bytes, Ordering::Relaxed);
}

/// Set the soft ceiling (`0` disables). Polled at the eval safepoint via
/// [`soft_limit_hit`], which raises a catchable `LispError`.
pub fn set_soft_limit(bytes: usize) {
    SOFT_LIMIT.store(bytes, Ordering::Relaxed);
}

/// `Some(live_bytes)` when the soft ceiling is set and currently exceeded, else
/// `None`. The eval safepoint calls this and, on `Some`, returns an `E0043`
/// error. Cheap when disabled: one relaxed load of a zero, early `None`.
#[inline]
pub fn soft_limit_hit() -> Option<usize> {
    let limit = SOFT_LIMIT.load(Ordering::Relaxed);
    if limit == 0 {
        return None;
    }
    let live = LIVE.load(Ordering::Relaxed);
    if live > limit {
        Some(live)
    } else {
        None
    }
}

/// Parse a human size: a plain byte count (`1048576`) or a number with a binary
/// suffix (`512K`, `64M`, `2G`, case-insensitive; an optional trailing `B`/`iB`
/// is ignored — `2GiB` == `2G`). Returns `None` on anything unparseable so the
/// caller can warn and fall back. `0` is valid and means "unlimited".
pub fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Strip a trailing `B`/`iB` (so `2GB` / `2GiB` both work) before the unit char.
    let body = s
        .strip_suffix('B')
        .or_else(|| s.strip_suffix('b'))
        .map(|t| {
            t.strip_suffix('i')
                .or_else(|| t.strip_suffix('I'))
                .unwrap_or(t)
        })
        .unwrap_or(s);
    let (num, mult) = match body.chars().last() {
        Some(c @ ('K' | 'k')) => (&body[..body.len() - c.len_utf8()], 1024usize),
        Some(c @ ('M' | 'm')) => (&body[..body.len() - c.len_utf8()], 1024 * 1024),
        Some(c @ ('G' | 'g')) => (&body[..body.len() - c.len_utf8()], 1024 * 1024 * 1024),
        _ => (body, 1usize),
    };
    let n: usize = num.trim().parse().ok()?;
    n.checked_mul(mult)
}

/// Read `BROOD_MEM_LIMIT` (hard) and `BROOD_MEM_SOFT_LIMIT` (soft) from the env
/// and apply them; absent vars leave the corresponding limit untouched. When
/// only the hard limit is given, the soft limit is derived as 3/4 of it (so the
/// graceful path fires first). A malformed value is warned about and ignored.
/// Called by every entry point — for plain runs / the REPL this is the *only*
/// way a limit gets set, so they stay unlimited unless the user opts in.
pub fn init_limits_from_env() {
    init_limits_inner(None);
}

/// Like [`init_limits_from_env`], but when an env var is *absent* fall back to
/// the given defaults instead of leaving the limit off. The test runners use
/// this so an adversarial test can't OOM the host; an explicit env var still
/// wins. Pass `(0, 0)` to behave exactly like [`init_limits_from_env`].
pub fn init_limits_with_default(default_hard: usize, default_soft: usize) {
    init_limits_inner(Some((default_hard, default_soft)));
}

fn init_limits_inner(defaults: Option<(usize, usize)>) {
    let env = |k: &str| -> Option<usize> {
        match std::env::var(k) {
            Ok(v) => match parse_size(&v) {
                Some(n) => Some(n),
                None => {
                    eprintln!("[mem] ignoring malformed {k}={v:?} (try e.g. 512M, 2G)");
                    None
                }
            },
            Err(_) => None,
        }
    };

    let hard_env = env("BROOD_MEM_LIMIT");
    let soft_env = env("BROOD_MEM_SOFT_LIMIT");

    // Hard: explicit env wins; else the provided default (if any).
    let hard = hard_env.or(defaults.map(|(h, _)| h));
    if let Some(h) = hard {
        set_hard_limit(h);
    }
    // Soft precedence, carefully ordered so a user who set only `BROOD_MEM_LIMIT`
    // gets a soft limit derived from *their* value (not clobbered by a larger
    // test default, which would invert the soft<hard ordering):
    //   1. explicit `BROOD_MEM_SOFT_LIMIT`
    //   2. user set `BROOD_MEM_LIMIT` but no soft → 3/4 of it
    //   3. the provided default soft (neither env var present)
    //   4. 3/4 of whatever hard we ended up with
    let soft = soft_env
        .or_else(|| hard_env.map(|h| h / 4 * 3))
        .or(defaults.map(|(_, s)| s))
        .or_else(|| hard.map(|h| h / 4 * 3));
    if let Some(s) = soft {
        set_soft_limit(s);
    }
}
