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
//! you want. The `(%)` / `(%)` primitives in `builtins.rs` read
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
//!   the whole brood process, after one stderr line naming the limit
//!   (`report_hard_limit`). Ungraceful (kills every green process) but it is
//!   the backstop that guarantees the *host* survives any allocation pattern,
//!   including a single huge allocation between eval safepoints.
//! - **Soft limit** ([`soft_limit_state`]): *not* enforced here — checked at the
//!   eval safepoint (`eval.rs`). Crossing it stops the process holding the most
//!   memory with `E0043` — raised in place, catchably, when that is the process
//!   that noticed (`process/pressure.rs`, ADR-399). Set below the hard limit so a
//!   runaway fails gracefully long before the hard abort fires. The single-shot
//!   giant allocation is the only case that reaches the hard limit.
//!
//! Normal `brood file.blsp` runs and the REPL stay unlimited unless
//! `BROOD_MEM_LIMIT` is set ([`init_limits_from_env`]) or they run in a
//! memory-capped container, whose cap sets both ([`container_memory_cap`]); the
//! test runners default both on ([`init_limits_with_default`]) so an adversarial
//! test can't take the machine down.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

/// The real allocator the [`Counting`] wrapper delegates to. **mimalloc**, not
/// the system malloc: Brood is a long-running runtime (editors, web servers) whose
/// immutable data path-copies on every update (a CHAMP `assoc` clones each node on
/// the root→leaf path; a fresh `Value` per builtin), so allocation throughput is
/// load-bearing. mimalloc's per-thread heaps + size-segregated free lists turn that
/// churn into ~bump-speed alloc/free. It holds freed pages for reuse rather than returning them to
/// the OS — a deliberate memory-for-speed trade, the right one for a long-running
/// app. The byte-counting in [`Counting`] (ADR-043 `BROOD_MEM_LIMIT`) is unaffected:
/// it tallies the requested `Layout` size around this backend.
#[cfg(all(not(feature = "system-alloc"), not(target_arch = "wasm32")))]
const BACKEND: mimalloc::MiMalloc = mimalloc::MiMalloc;
/// Sanitizer builds (`--features system-alloc`): ThreadSanitizer can't see the
/// synchronization inside mimalloc's un-instrumented C internals, so every
/// cross-thread memory-block reuse reports as a phantom data race. The system
/// allocator is TSAN-interceptable, making real races stand out.
// wasm32 has no mimalloc C build (and no threads for its per-thread heaps), so the
// browser/headless wasm runtime falls back to the system allocator.
#[cfg(any(feature = "system-alloc", target_arch = "wasm32"))]
const BACKEND: std::alloc::System = std::alloc::System;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

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

/// Default hard ceiling the *test runners* apply when neither env var is set
/// (`brood --test`, `nest test`, the Rust-hosted Brood suite). A **host-survival
/// backstop, not a working-set budget**: set below a typical dev machine's RAM so an
/// allocation that grows without bound fails cleanly long before the OS OOM-killer
/// fires, and well above the suite's legitimate peak (~2.6× at last measurement, KI-47)
/// so parallel load never trips it. The collector is the reclamation path; opt out per
/// run via `BROOD_MEM_LIMIT`.
///
/// **Never default this to `0`/unlimited** — a non-collecting path (or the prelude
/// *builder*, where GC is off) could still eat host RAM; an unlimited default once
/// OOM-froze the machine.
pub const TEST_DEFAULT_HARD: usize = 3 * 1024 * 1024 * 1024; // 3 GiB
/// Soft default for the test runners: a runaway fails with the catchable `E0043`
/// before the hard abort. It is a process-wide total, so the cases that die on it are
/// whichever happen to be running when the suite crosses it — the tree-walker arm,
/// which allocates the most, crosses first.
pub const TEST_DEFAULT_SOFT: usize = 2 * 1024 * 1024 * 1024; // 2 GiB

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

/// Would committing `size` more bytes cross the hard limit? When no limit is set (the
/// common case) it's a single load of a zero and an early return. A small allocation far
/// from the limit is judged by the lazily refreshed `APPROX_LIVE` snapshot (one relaxed
/// load): it lags by up to [`PEAK_SAMPLE`] allocations a thread, and an overshoot that size
/// is fine for a backstop. A LARGE allocation, or any allocation the snapshot puts near the
/// limit, is judged by the exact sum: the snapshot's lag is counted in allocations, not
/// bytes, so it can miss earlier large blocks entirely — a 100 MB string made since the last
/// sample was invisible, and a 500 MB one built on top of it passed a 512 MiB limit (KI-248).
#[inline]
fn would_exceed_hard(size: usize) -> bool {
    hard_verdict(
        size,
        HARD_LIMIT.load(Ordering::Relaxed),
        APPROX_LIVE.load(Ordering::Relaxed),
        exact_live,
    )
}

/// The hard-limit decision, apart from the process-wide counters so a test can drive it:
/// `approx` is the snapshot, `exact` the exact sum, consulted only for a large or near-limit
/// allocation. Limit 0 means none.
#[inline]
fn hard_verdict(size: usize, limit: usize, approx: usize, exact: impl FnOnce() -> usize) -> bool {
    if limit == 0 {
        return false;
    }
    if size < LARGE_ALLOCATION && approx.saturating_add(size) <= limit - limit / 8 {
        return false;
    }
    exact().saturating_add(size) > limit
}

/// The exact live sum, which it also leaves in the snapshot (so a refusal reports the true
/// live figure). Large or near-limit allocations only.
#[cold]
fn exact_live() -> usize {
    let live = live_bytes();
    APPROX_LIVE.store(live, Ordering::Relaxed);
    live
}

/// An allocation at least this large checks the soft limit itself.
const LARGE_ALLOCATION: usize = 64 * 1024;

thread_local! {
    /// Large-allocation bytes this thread may still make near the soft limit before its
    /// quantum is ended again. A `const` thread-local with no destructor, so the allocator
    /// may touch it.
    static NEAR_ALLOWANCE: Cell<usize> = const { Cell::new(0) };
    /// Set with every quantum ended here; taken by a JIT'd arm's builtin call
    /// ([`take_soft_attention`]), which polls its reductions from a register every 128
    /// iterations and so never sees the ended quantum in time.
    static SOFT_ATTENTION: Cell<bool> = const { Cell::new(false) };
}

/// Did a large allocation on this thread end its quantum for the soft limit since the last
/// call? One thread-local read; clears it.
#[inline]
pub fn take_soft_attention() -> bool {
    // Read first: this runs on every builtin call from native code, and is almost always
    // false, so the common case stores nothing.
    SOFT_ATTENTION.with(|attention| attention.get() && attention.replace(false))
}

/// End this thread's quantum for the soft limit, and flag it for a JIT'd caller.
fn soft_limit_attention() {
    SOFT_ATTENTION.with(|attention| attention.set(true));
    crate::process::expire_quantum();
}

/// A large allocation of `size` bytes just landed. Over the soft limit, end this thread's
/// quantum, so the once-per-quantum safepoint check runs at the next reduction tick:
/// without it, a loop allocating a megabyte an iteration covers hundreds of megabytes
/// inside one quantum and reaches the hard limit — an abort — before the soft limit is
/// ever checked. Within the last eighth, do the same once per 1/64 of the limit, so a fast
/// allocator measures itself on the way up and the choice of whom to stop at the crossing
/// sees it (`process/pressure.rs`). Sums the shards (exact, not the lagging snapshot);
/// only large allocations pay, and only while a soft limit is set.
#[cold]
fn large_allocation_made(size: usize) {
    let soft = SOFT_LIMIT.load(Ordering::Relaxed);
    if soft == 0 {
        return;
    }
    let live = live_bytes();
    if live > soft {
        soft_limit_attention();
    } else if live > soft - soft / 8 {
        NEAR_ALLOWANCE.with(|allowance| {
            if allowance.get() <= size {
                allowance.set(soft / 64);
                soft_limit_attention();
            } else {
                allowance.set(allowance.get() - size);
            }
        });
    }
}

/// Say why the process is about to abort, once. Rust's handler for the null that follows
/// prints only `memory allocation of N bytes failed`, which names neither the limit nor
/// that one was set. This runs inside the allocator, so it must not allocate: the line is
/// formatted into a stack buffer and written straight to the stderr descriptor.
#[cold]
fn report_hard_limit(size: usize) {
    use std::fmt::Write;
    static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    struct StackLine {
        bytes: [u8; 400],
        len: usize,
    }
    impl Write for StackLine {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            let room = self.bytes.len() - self.len;
            let taken = text.len().min(room);
            self.bytes[self.len..self.len + taken].copy_from_slice(&text.as_bytes()[..taken]);
            self.len += taken;
            Ok(())
        }
    }
    let mut line = StackLine {
        bytes: [0; 400],
        len: 0,
    };
    let limit = HARD_LIMIT.load(Ordering::Relaxed);
    let cause = if size >= limit / 16 {
        "one value this large is being built in a single step"
    } else {
        "memory grew faster than the soft limit could stop it"
    };
    let _ = writeln!(
        line,
        "brood: hard memory limit reached ({} MiB, BROOD_MEM_LIMIT): an allocation of {} KiB \
         on top of {} MiB live was refused, and the runtime aborts; {cause}.",
        limit >> 20,
        size >> 10,
        APPROX_LIVE.load(Ordering::Relaxed) >> 20,
    );
    #[cfg(unix)]
    // SAFETY: writes initialised bytes from a live stack buffer to the stderr descriptor.
    unsafe {
        libc::write(2, line.bytes.as_ptr().cast(), line.len);
    }
}

// SAFETY: every method forwards to `BACKEND` with the caller's layout and pointer unchanged,
// so `BACKEND` upholds the `GlobalAlloc` contract. The bookkeeping around it never allocates
// (atomics, and a stderr write from a stack buffer), and a refusal returns null, which the
// contract allows.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwards the caller's `GlobalAlloc::alloc` contract (non-zero-size
        // `layout`) unchanged to `BACKEND`; the bookkeeping around it does not allocate.
        unsafe {
            if would_exceed_hard(layout.size()) {
                // Null tells Rust the allocation failed; the default handler then
                // aborts the process. The host survives; the brood process doesn't.
                report_hard_limit(layout.size());
                return std::ptr::null_mut();
            }
            let ptr = BACKEND.alloc(layout);
            if !ptr.is_null() {
                record_alloc(layout.size());
                if layout.size() >= LARGE_ALLOCATION {
                    large_allocation_made(layout.size());
                }
            }
            ptr
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller guarantees `ptr` came from this allocator with `layout`, and
        // every block this allocator hands out came from `BACKEND`, so it is `BACKEND`'s.
        unsafe {
            BACKEND.dealloc(ptr, layout);
            record_dealloc(layout.size());
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's `GlobalAlloc::realloc` contract (`ptr` from this allocator
        // with `layout`, valid non-zero `new_size`) is forwarded unchanged to `BACKEND`,
        // which allocated `ptr`; refusing a growth returns null and leaves `ptr` intact.
        unsafe {
            // Only a *growth* can cross the limit; a shrink is always allowed.
            if new_size > layout.size() && would_exceed_hard(new_size - layout.size()) {
                report_hard_limit(new_size - layout.size());
                return std::ptr::null_mut();
            }
            let new_ptr = BACKEND.realloc(ptr, layout, new_size);
            if !new_ptr.is_null() {
                let old = layout.size();
                if new_size >= old {
                    record_alloc(new_size - old);
                    if new_size - old >= LARGE_ALLOCATION {
                        large_allocation_made(new_size - old);
                    }
                } else {
                    record_dealloc(old - new_size);
                }
            }
            new_ptr
        }
    }
}

/// Bytes currently allocated across the whole process (the wrapping sum of the
/// per-thread shards — see `LIVE`).
pub fn live_bytes() -> usize {
    let sum = LIVE.iter().fold(0usize, |acc, s| {
        acc.wrapping_add(s.0.load(Ordering::Relaxed))
    });
    settle_live_sum(sum, APPROX_LIVE.load(Ordering::Relaxed))
}

/// Reject a torn shard sum. The shards are read one at a time, not as a snapshot, so
/// a block allocated on one thread and freed on another can be seen freed (its
/// shard wrapped below zero) before it is seen allocated: the wrapping sum then reads
/// ≈2^64 — a bogus hard-limit abort under `BROOD_MEM_LIMIT` and a garbage peak. No
/// real live figure exceeds `isize::MAX` (no allocation can), so a sum past it is
/// stale by construction and the previous settled figure stands in.
fn settle_live_sum(sum: usize, previous: usize) -> usize {
    if sum > isize::MAX as usize {
        previous
    } else {
        sum
    }
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

/// Set the soft ceiling (`0` disables). Polled at the safepoints via
/// [`soft_limit_state`]; crossing it raises a catchable `LispError`.
pub fn set_soft_limit(bytes: usize) {
    SOFT_LIMIT.store(bytes, Ordering::Relaxed);
}

/// Where live memory stands against the soft ceiling, for the eval safepoint.
pub enum SoftLimit {
    /// Below 7/8 of it, or no ceiling set.
    Clear,
    /// Within the last eighth: processes measure themselves, so the choice of whom to stop
    /// at the crossing has current figures (`process/pressure.rs`).
    Near,
    /// Crossed, with the live byte count.
    Over(usize),
}

/// Whether the soft ceiling is crossed (with the live byte count) or approached — the
/// safepoints' probe. Cheap when no ceiling is set: one relaxed load of a zero; otherwise
/// it sums the shards.
#[inline]
pub fn soft_limit_state() -> SoftLimit {
    let limit = SOFT_LIMIT.load(Ordering::Relaxed);
    if limit == 0 {
        return SoftLimit::Clear;
    }
    let live = live_bytes();
    if live > limit {
        SoftLimit::Over(live)
    } else if live > limit - limit / 8 {
        SoftLimit::Near
    } else {
        SoftLimit::Clear
    }
}

/// Is live memory within the last eighth of the soft ceiling, by the sampled figure? Two
/// relaxed loads, for a hot path ([`soft_limit_state`] sums every shard).
#[inline]
pub fn near_soft_limit_approx() -> bool {
    let limit = SOFT_LIMIT.load(Ordering::Relaxed);
    limit != 0 && APPROX_LIVE.load(Ordering::Relaxed) > limit - limit / 8
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
/// and apply them. When only the hard limit is given, the soft limit is derived as
/// 3/4 of it (so the graceful path fires first). A malformed value is warned about
/// and ignored. Called by every entry point. With neither variable set, a process
/// inside a memory-capped container takes its limits from the cap
/// ([`container_memory_cap`]); anywhere else plain runs and the REPL stay unlimited.
pub fn init_limits_from_env() {
    init_limits_inner(None);
}

/// Like [`init_limits_from_env`], but when an env var is *absent* fall back to
/// the given defaults instead of leaving the limit off. The test runners use
/// this so an adversarial test can't OOM the host; an explicit env var still
/// wins, and a container cap lower than a default wins over it. Pass `(0, 0)` to
/// behave exactly like [`init_limits_from_env`].
pub fn init_limits_with_default(default_hard: usize, default_soft: usize) {
    init_limits_inner(Some((default_hard, default_soft)).filter(|&(hard, _)| hard != 0));
}

/// The limits a container cap implies: the hard limit a quarter below the cap, and
/// the soft three quarters of that. Well below the cap because the container counts
/// resident memory and the allocator counts live bytes: pages the allocator keeps
/// after a free, thread stacks and mapped code are the difference, and a leak making
/// garbage as it grows measured resident at about 1.45 times live — under a 600 MB
/// cap, a hard limit an eighth below it was out-of-memory killed before the soft
/// limit tripped. The container's kill names nothing, where the limits stop or name
/// the process holding the memory (ADR-399).
fn limits_for_cap(cap: usize) -> (usize, usize) {
    let hard = cap / 4 * 3;
    (hard, hard / 4 * 3)
}

/// The memory cap of the container this process runs in, if it has one: the
/// tightest `memory.max` on its cgroup's path (cgroup v2), else the tightest
/// `memory.limit_in_bytes` (v1). `None` with no cap, no cgroup filesystem, or off
/// Linux.
pub fn container_memory_cap() -> Option<usize> {
    let membership = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    cap_from_cgroups(&membership, |path| std::fs::read_to_string(path).ok())
}

/// [`container_memory_cap`] over a `/proc/self/cgroup` text and a file reader, so the
/// walk is testable without a container. A cap may be set on any ancestor (a systemd
/// slice, a container runtime's parent), so every level counts and the lowest wins.
fn cap_from_cgroups(membership: &str, read: impl Fn(&str) -> Option<String>) -> Option<usize> {
    // v1 reports a "practically unlimited" cap as a page-rounded i64::MAX. Read as `u64`
    // (a cgroup counts in 64 bits whatever the target's `usize`); a cap this address
    // space cannot hold is no cap for it.
    const UNLIMITED_V1: u64 = 1 << 62;
    let tightest = |root: &str, file: &str, path: &str| {
        let mut tightest: Option<usize> = None;
        let mut level = path.trim_end_matches('/');
        loop {
            let cap = read(&format!("{root}{level}/{file}"))
                .and_then(|text| text.trim().parse::<u64>().ok())
                .filter(|&cap| cap < UNLIMITED_V1)
                .and_then(|cap| usize::try_from(cap).ok());
            if let Some(cap) = cap {
                tightest = Some(tightest.map_or(cap, |seen| seen.min(cap)));
            }
            match level.rfind('/') {
                Some(cut) => level = &level[..cut],
                None => return tightest,
            }
        }
    };
    let mut v1_path = None;
    for line in membership.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(_), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if controllers.is_empty() {
            if let Some(cap) = tightest("/sys/fs/cgroup", "memory.max", path) {
                return Some(cap);
            }
        } else if controllers
            .split(',')
            .any(|controller| controller == "memory")
        {
            v1_path = Some(path);
        }
    }
    v1_path.and_then(|path| tightest("/sys/fs/cgroup/memory", "memory.limit_in_bytes", path))
}

/// The limits a caller's defaults and a container cap give together: the cap's limits
/// bound the defaults, and stand in for them when there are none.
fn limits_with_cap(defaults: Option<(usize, usize)>, cap: Option<usize>) -> Option<(usize, usize)> {
    match (defaults, cap.map(limits_for_cap)) {
        (Some((hard, soft)), Some((cap_hard, cap_soft))) => {
            Some((hard.min(cap_hard), soft.min(cap_soft)))
        }
        (defaults, container) => defaults.or(container),
    }
}

/// Whether the limits follow the container's cap — no env var set them.
static FOLLOW_CONTAINER: AtomicBool = AtomicBool::new(false);
/// The caller's default hard and soft limits (`0` = none), for re-deriving them.
static CALLER_DEFAULTS: AtomicUsize = AtomicUsize::new(0);
static CALLER_DEFAULT_SOFT: AtomicUsize = AtomicUsize::new(0);
/// When the cap was last read, in milliseconds since [`CAP_CLOCK`] began.
static CAP_READ_AT: AtomicU64 = AtomicU64::new(0);
static CAP_CLOCK: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);
/// How often a running runtime re-reads its container's cap.
const CAP_REREAD_MS: u64 = 5000;

/// Re-derive the limits from the container's cap if it changed — a container can be
/// resized while it runs (a pod's in-place resize), and limits read once at startup would
/// then sit above a lowered cap, where the kernel's kill comes first. Called from the
/// once-per-quantum checks; does nothing unless the limits follow the cap, and reads the
/// cgroup file at most every [`CAP_REREAD_MS`], one caller at a time.
pub fn refresh_container_limits() {
    if !FOLLOW_CONTAINER.load(Ordering::Relaxed) {
        return;
    }
    let now = CAP_CLOCK.elapsed().as_millis() as u64 + CAP_REREAD_MS;
    let last = CAP_READ_AT.load(Ordering::Relaxed);
    if now < last + CAP_REREAD_MS
        || CAP_READ_AT
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    let defaults = match CALLER_DEFAULTS.load(Ordering::Relaxed) {
        0 => None,
        hard => Some((hard, CALLER_DEFAULT_SOFT.load(Ordering::Relaxed))),
    };
    let (hard, soft) = limits_with_cap(defaults, container_memory_cap()).unwrap_or((0, 0));
    if hard != HARD_LIMIT.load(Ordering::Relaxed) || soft != SOFT_LIMIT.load(Ordering::Relaxed) {
        set_hard_limit(hard);
        set_soft_limit(soft);
    }
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
    // Inside a capped container the cap is a ceiling the caller's defaults cannot exceed,
    // and the default when there are none. Not consulted when the user set either limit —
    // and then not followed either ([`refresh_container_limits`]).
    let defaults = match (hard_env, soft_env) {
        (None, None) => {
            CALLER_DEFAULTS.store(defaults.map_or(0, |(hard, _)| hard), Ordering::Relaxed);
            CALLER_DEFAULT_SOFT.store(defaults.map_or(0, |(_, soft)| soft), Ordering::Relaxed);
            FOLLOW_CONTAINER.store(true, Ordering::Relaxed);
            limits_with_cap(defaults, container_memory_cap())
        }
        _ => defaults,
    };

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A torn read of the shards — one seen with a free whose allocation it has not
    /// seen yet — must not surface as a ≈2^64 live figure (a bogus hard-limit abort).
    /// Driven through `live_bytes` itself: a shard is pushed below zero exactly as a
    /// cross-thread free does, read, and restored.
    #[test]
    fn a_torn_shard_sum_does_not_read_as_a_huge_live_figure() {
        let torn = 1usize << 40;
        LIVE[0].0.fetch_sub(torn, Ordering::Relaxed);
        let observed = live_bytes();
        let peak = peak_bytes();
        LIVE[0].0.fetch_add(torn, Ordering::Relaxed);
        assert!(
            observed <= isize::MAX as usize,
            "torn sum leaked out: {observed}"
        );
        assert!(
            peak <= isize::MAX as usize,
            "torn sum reached the peak: {peak}"
        );
        // A sane sum passes through untouched; a torn one keeps the previous figure.
        assert_eq!(settle_live_sum(4096, 7), 4096);
        assert_eq!(settle_live_sum(usize::MAX - 10, 7), 7);
    }

    /// KI-248: the snapshot lags by a count of allocations, so a large block made since the
    /// last sample is invisible to it. A large allocation on top of one must still be refused:
    /// 100 MB live that the snapshot has not seen, then a 500 MB request under a 512 MiB
    /// limit. Sabotage: judge every size by the snapshot again and the first assert fails.
    ///
    /// Driven through `hard_verdict`, not the process-wide counters: this test used to store
    /// a real 512 MiB `HARD_LIMIT`, and under `cargo test` — every lib test a thread of one
    /// process, as the nightly ASAN job runs them — a sibling holding 1 GiB live had its next
    /// allocation refused and the whole process aborted (2026-10-17).
    #[test]
    fn a_large_allocation_sees_the_large_blocks_the_snapshot_has_not() {
        let limit = 512usize << 20;
        let stale = 10usize << 20;
        let exact = stale + 100_000_000;
        assert!(
            hard_verdict(500_000_000, limit, stale, || exact),
            "500 MB on top of an unseen 100 MB passed a 512 MiB hard limit"
        );
        assert!(
            !hard_verdict(64, limit, stale, || exact),
            "a small allocation far from the limit was refused"
        );
        assert!(
            !hard_verdict(usize::MAX / 2, 0, stale, || exact),
            "limit 0 means none"
        );
    }

    fn files(entries: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let entries: Vec<(String, String)> = entries
            .iter()
            .map(|&(path, text)| (path.to_string(), text.to_string()))
            .collect();
        move |path| {
            entries
                .iter()
                .find(|(known, _)| known == path)
                .map(|(_, text)| text.clone())
        }
    }

    /// The cap may sit on any ancestor of the process's own cgroup — a systemd slice, a
    /// container runtime's parent — and the tightest one is what the kernel enforces.
    #[test]
    fn the_tightest_v2_cap_on_the_path_wins() {
        let membership = "0::/machine.slice/app.scope\n";
        let read = files(&[
            ("/sys/fs/cgroup/machine.slice/app.scope/memory.max", "max\n"),
            ("/sys/fs/cgroup/machine.slice/memory.max", "1073741824\n"),
        ]);
        assert_eq!(cap_from_cgroups(membership, read), Some(1 << 30));
        let read = files(&[
            (
                "/sys/fs/cgroup/machine.slice/app.scope/memory.max",
                "536870912\n",
            ),
            ("/sys/fs/cgroup/machine.slice/memory.max", "1073741824\n"),
        ]);
        assert_eq!(cap_from_cgroups(membership, read), Some(1 << 29));
    }

    #[test]
    fn no_cap_anywhere_is_none() {
        let read = files(&[("/sys/fs/cgroup/user.slice/memory.max", "max\n")]);
        assert_eq!(cap_from_cgroups("0::/user.slice\n", read), None);
        // A container whose cgroup is the namespace root, with no file to read.
        assert_eq!(cap_from_cgroups("0::/\n", files(&[])), None);
    }

    /// cgroup v1 names the memory controller on its own line, and reports "unlimited" as
    /// a page-rounded `i64::MAX`.
    #[test]
    fn a_v1_memory_controller_cap_is_read_and_its_unlimited_is_none() {
        let membership = "12:pids:/docker/abc\n4:memory:/docker/abc\n1:name=systemd:/docker/abc\n";
        let read = files(&[(
            "/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes",
            "268435456\n",
        )]);
        assert_eq!(cap_from_cgroups(membership, read), Some(1 << 28));
        let read = files(&[(
            "/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes",
            "9223372036854771712\n",
        )]);
        assert_eq!(cap_from_cgroups(membership, read), None);
    }

    /// The cap bounds a caller's defaults and stands in for none; no cap leaves the
    /// defaults as they were — the rule a resized container is re-derived by.
    #[test]
    fn a_cap_bounds_the_defaults_and_replaces_none() {
        let gib = 1 << 30;
        let (cap_hard, cap_soft) = limits_for_cap(gib);
        assert_eq!(
            limits_with_cap(Some((3 * gib, 2 * gib)), Some(gib)),
            Some((cap_hard, cap_soft))
        );
        assert_eq!(limits_with_cap(Some((100, 75)), Some(gib)), Some((100, 75)));
        assert_eq!(limits_with_cap(None, Some(gib)), Some((cap_hard, cap_soft)));
        assert_eq!(limits_with_cap(Some((100, 75)), None), Some((100, 75)));
        assert_eq!(limits_with_cap(None, None), None);
    }

    #[test]
    fn a_cap_leaves_room_below_it_and_soft_below_hard() {
        let (hard, soft) = limits_for_cap(1 << 30);
        assert_eq!(hard, (1 << 30) / 4 * 3);
        assert_eq!(soft, hard / 4 * 3);
    }
}
