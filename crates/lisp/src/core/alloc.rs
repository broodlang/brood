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
//! these counters. The wrapper itself is std-only; its backend is **mimalloc**
//! (see [`BACKEND`]) — allocation throughput is load-bearing for a long-running,
//! immutable, path-copying runtime, so it's the one allocator dependency we take
//! (ADR-005's dependency-free rule relaxed for genuine runtime infrastructure,
//! like `boxcar`).
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

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

/// The real allocator the [`Counting`] wrapper delegates to. **mimalloc**, not
/// the system malloc: Brood is a long-running runtime (editors, web servers) whose
/// immutable data path-copies on every update (a CHAMP `assoc` clones each node on
/// the root→leaf path; a fresh `Value` per builtin), so allocation throughput is
/// load-bearing. mimalloc's per-thread heaps + size-segregated free lists turn that
/// churn into ~bump-speed alloc/free — measured ~15% on `wordcount`, ~28% on
/// `bintree`, broadly across every allocation-heavy path (alloc-light code and boot
/// time are unchanged). It holds freed pages for reuse rather than returning them to
/// the OS — a deliberate memory-for-speed trade, the right one for a long-running
/// app. The byte-counting in [`Counting`] (ADR-043 `BROOD_MEM_LIMIT`) is unaffected:
/// it tallies the requested `Layout` size around this backend exactly as before.
const BACKEND: mimalloc::MiMalloc = mimalloc::MiMalloc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Live-bytes accounting is **sharded** across cache-line-padded counters, one
// picked per thread, so allocators on different worker threads don't contend on a
// single atomic. Before this, every (de)allocation hit one `LIVE` (a `fetch_add`)
// and one `PEAK` (a `fetch_max` — a CAS *retry loop*, the worst case under
// contention). With the interner mutex fixed (the dominant serialization, see
// `value::intern`), this counter became the next contention point under
// allocation-heavy fan-out — sharding it recovered a further ~20%. `live_bytes`
// sums the shards; a shard can go negative (memory allocated on one thread, freed
// on another), but the wrapping sum is exact mod 2^64, which covers any real heap.
// Accounting stays process-wide (ADR-005/043; per-process limits deferred, ADR-011).
const SHARDS: usize = 64;

#[repr(align(64))] // own cache line — no false sharing between shards
struct Shard(AtomicUsize);

static LIVE: [Shard; SHARDS] = [const { Shard(AtomicUsize::new(0)) }; SHARDS];
static PEAK: AtomicUsize = AtomicUsize::new(0); // high-water mark of live_bytes()
                                                // A coarse, lazily-refreshed snapshot of `live_bytes()` for the per-allocation
                                                // hard-limit check, so that check stays one load instead of summing every shard on
                                                // the hot path. Refreshed on the `PEAK` sample cadence (see `record_alloc`); lags
                                                // real live bytes by at most one window — fine for a host-survival backstop that
                                                // already tolerates a small overshoot, and a single oversized allocation is still
                                                // caught directly by its own `size`. The exact soft limit (summed at the eval
                                                // safepoint) is the graceful path and trips first on gradual growth.
static APPROX_LIVE: AtomicUsize = AtomicUsize::new(0);
static HARD_LIMIT: AtomicUsize = AtomicUsize::new(0); // 0 = unlimited; abort if crossed
static SOFT_LIMIT: AtomicUsize = AtomicUsize::new(0); // 0 = unlimited; safepoint raises if crossed

// Round-robin shard assignment, one per thread. A `usize`/`Cell` thread-local has
// no destructor, so first access is native TLS with no heap allocation — safe to
// touch from inside the global allocator (no re-entrancy).
static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);
// Allocations a thread makes between high-water samples. Sampling — rather than a
// `fetch_max` per allocation — is what removes the CAS storm; the peak is an
// observability figure, so a slightly late sample is acceptable.
const PEAK_SAMPLE: u32 = 512;
thread_local! {
    static SHARD_IDX: usize = NEXT_SHARD.fetch_add(1, Ordering::Relaxed) % SHARDS;
    static SINCE_SAMPLE: Cell<u32> = const { Cell::new(0) };
}

/// Default ceiling the *test runners* apply when neither env var is set
/// (`brood --test`, `nest test`, the `cargo test` Brood suite). Its job is to
/// **GUARANTEE THE HOST SURVIVES a test run** — it is *not* a precise working-set
/// budget. The number is chosen *below* a typical dev machine's RAM so a run whose
/// allocation grows without bound fails with a clean, catchable `E0043` (then the
/// hard abort) long before the OS OOM-killer or a hard freeze can fire.
///
/// **2 GiB hard / 1 GiB soft.** Now that automatic collection at any eval depth
/// (Stage B + ADR-061, `docs/memory-review.md`) reclaims LOCAL garbage, the whole
/// project suite peaks ~240 MB *under collection* (down from ~1.1 GiB when the
/// runner merely hibernated between steps, ~4 GiB tripping the old cap, and ~18 GiB
/// before isolated units ran in droppable processes / OOM-froze the host). So these
/// were sized for an era that's gone: 1 GiB soft is ~4× the live peak — high enough
/// never to trip on legitimate parallel load, low enough to catch a genuine runaway
/// (which heads to many GB) *cleanly* via the catchable `E0043` before the hard
/// abort. This cap is a **host-survival backstop, not a working-set budget** — the
/// collector is the reclamation path. Opt out per run via `BROOD_MEM_LIMIT`.
///
/// **Never default this to `0`/unlimited** — a pathological non-collecting path (or
/// the prelude *builder*, where GC is off) could still eat host RAM; an unlimited
/// default once OOM-froze the machine.
pub const TEST_DEFAULT_HARD: usize = 2 * 1024 * 1024 * 1024; // 2 GiB
/// Soft default for the test runners — 1 GiB, so a runaway/accumulating run fails
/// *cleanly* (catchable `E0043`) before the hard abort and far below host RAM.
pub const TEST_DEFAULT_SOFT: usize = 1024 * 1024 * 1024; // 1 GiB

/// Allocator wrapper that tallies bytes in/out and enforces [`HARD_LIMIT`],
/// delegating the actual alloc/free to [`BACKEND`] (mimalloc).
pub struct Counting;

fn record_alloc(size: usize) {
    SHARD_IDX.with(|&i| LIVE[i].0.fetch_add(size, Ordering::Relaxed));
    // Refresh PEAK / APPROX_LIVE roughly every PEAK_SAMPLE allocations on this
    // thread, instead of a global `fetch_max` on every allocation (the old
    // serialization point). `live_bytes` only sums the shards here, off the
    // common path.
    SINCE_SAMPLE.with(|c| {
        let n = c.get().wrapping_add(1);
        if n >= PEAK_SAMPLE {
            c.set(0);
            let live = live_bytes();
            APPROX_LIVE.store(live, Ordering::Relaxed);
            PEAK.fetch_max(live, Ordering::Relaxed);
        } else {
            c.set(n);
        }
    });
}

fn record_dealloc(size: usize) {
    SHARD_IDX.with(|&i| LIVE[i].0.fetch_sub(size, Ordering::Relaxed));
}

/// Would committing `size` more bytes cross the hard limit? Reads the lazily
/// refreshed `APPROX_LIVE` snapshot (one relaxed load); when no limit is set (the
/// common case) it's a single load of a zero and an early return. The snapshot
/// lags slightly and the check races under the worker pool, but the limit is a
/// safety backstop, not byte-exact accounting — a small overshoot is fine, and a
/// single oversized allocation is caught by its own `size` regardless of the lag.
#[inline]
fn would_exceed_hard(size: usize) -> bool {
    let limit = HARD_LIMIT.load(Ordering::Relaxed);
    limit != 0 && APPROX_LIVE.load(Ordering::Relaxed).saturating_add(size) > limit
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if would_exceed_hard(layout.size()) {
            // Null tells Rust the allocation failed; the default handler then
            // aborts the process. The host survives; the brood process doesn't.
            return std::ptr::null_mut();
        }
        let ptr = BACKEND.alloc(layout);
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        BACKEND.dealloc(ptr, layout);
        record_dealloc(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Only a *growth* can cross the limit; a shrink is always allowed.
        if new_size > layout.size() && would_exceed_hard(new_size - layout.size()) {
            return std::ptr::null_mut();
        }
        let new_ptr = BACKEND.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            let old = layout.size();
            if new_size >= old {
                record_alloc(new_size - old);
            } else {
                record_dealloc(old - new_size);
            }
        }
        new_ptr
    }
}

/// Bytes currently allocated across the whole process (the wrapping sum of the
/// per-thread shards — see `LIVE`).
pub fn live_bytes() -> usize {
    LIVE.iter().fold(0usize, |acc, s| {
        acc.wrapping_add(s.0.load(Ordering::Relaxed))
    })
}

/// The largest [`live_bytes`] has ever been (since process start). The high-water
/// mark is sampled on a cadence (see `record_alloc`), so fold in the current live
/// figure on read to reflect any growth since the last sample.
pub fn peak_bytes() -> usize {
    let live = live_bytes();
    PEAK.fetch_max(live, Ordering::Relaxed);
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
    let live = live_bytes();
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
