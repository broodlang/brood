//! The collector's policy constants and their env overrides — floors, strides, the
//! nursery ceiling, the deep-walker stack guard, tenure and trace defaults. Every
//! `BROOD_GC_*` knob the runtime reads is parsed here.

use super::*;

// The collector's policy constants and their env overrides — floors, strides, the nursery
// ceiling, the deep-walker stack guard, tenure and trace defaults. Moved here from `heap.rs`
// on 2026-09-08 (handoff item 1, move f); `heap.rs` re-imports them so its own users and the
// sibling children (`gc_runtime`, `equality`, `promote`) resolve them exactly as before.

/// Parse a GC threshold override (an *object count*, with an optional `K`/`M`
/// suffix — `64K` = 65536, `1M` = 1048576) from env var `key`. `None` if unset;
/// a malformed value warns and is ignored (so the caller's default stands).
/// Mirrors the `BROOD_MEM_LIMIT` size-parse style in `core/alloc.rs`, but counts
/// objects rather than bytes.
pub(in crate::core::heap) fn gc_count_env(key: &str) -> Option<usize> {
    let v = std::env::var(key).ok()?;
    let s = v.trim();
    let (num, mult) = match s.chars().last() {
        Some(c @ ('K' | 'k')) => (&s[..s.len() - c.len_utf8()], 1024usize),
        Some(c @ ('M' | 'm')) => (&s[..s.len() - c.len_utf8()], 1024 * 1024),
        _ => (s, 1usize),
    };
    match num.trim().parse::<usize>() {
        Ok(n) => n.checked_mul(mult),
        Err(_) => {
            eprintln!("[gc] ignoring malformed {key}={v:?} (try e.g. 65536 or 64K)");
            None
        }
    }
}

/// Object-count budget a process may accumulate before its **first** GC (the
/// initial/minimum value of the adaptive `gc_threshold`, which after each GC
/// becomes `max(gc_floor, live*2)` — so a genuinely large live set keeps its own
/// `live*2` threshold and the floor is irrelevant to it). The floor therefore
/// only bites churny processes whose working set stays *below* it.
///
/// **Process-count-aware** (the fix for the `pfib` 1-GB blowup): a fixed object
/// budget is divided among the live processes, so fanning out N short-lived
/// churny processes doesn't have each one climb to the single-process ceiling
/// before collecting. A lone process is unchanged at `FLOOR_MAX`; the
/// per-process floor scales down toward `FLOOR_MIN` as concurrency rises
/// (e.g. 100-way `pfib`: ~64K each → ~4K each, ~990 MB → ~90 MB peak, no
/// throughput cost — the churn GCs were happening regardless, just later).
///
/// Read only at process creation and after each GC (never on the allocation hot
/// path), so the relaxed atomic load is free. `BROOD_GC_FLOOR` / `BROOD_GC_STRESS`
/// still pin a fixed value and opt out of the adaptive policy.
pub(in crate::core::heap) fn gc_floor() -> usize {
    // An explicit override pins a fixed floor and bypasses the adaptive policy —
    // used by the GC stress tests and honoured by the "non-default GC config"
    // benchmark guard. Cached: the env is read once.
    static OVERRIDE: OnceLock<Option<usize>> = OnceLock::new();
    let fixed = *OVERRIDE.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            Some(0)
        } else {
            // Overridable for tuning via `BROOD_GC_FLOOR` (object count, K/M ok).
            gc_count_env("BROOD_GC_FLOOR")
        }
    });
    if let Some(n) = fixed {
        return n;
    }
    // ~64K objects for a lone process (well above per-call working sets, trivial
    // vs the GBs a long-running process leaks); ~4K is the floor under heavy
    // fan-out (below this, GC churn starts to cost more than the memory saved).
    const FLOOR_MAX: usize = 64 * 1024;
    const FLOOR_MIN: usize = 4 * 1024;
    let live = live_process_count().max(1);
    (FLOOR_MAX / live).clamp(FLOOR_MIN, FLOOR_MAX)
}

/// RUNTIME-closure count at or above which the eval safepoint auto-runs a
/// **RUNTIME** compaction ([`Heap::maybe_runtime_collect`]) — the shared-code
/// analog of [`gc_floor`]. The region only grows on `def`/hot-reload (≈1 KB per
/// superseded closure), so a default of 4096 (~4 MB of churn) keeps a normal
/// program — which defines each global once, far below this — from ever
/// auto-collecting, while a sustained redefinition session is bounded. Like
/// [`major_floor`] (and unlike [`gc_floor`]) this stays **nonzero under
/// `BROOD_GC_STRESS`**: recompacting the whole RUNTIME region at *every*
/// safepoint would be O(region) per step, so stress keeps it periodic (still
/// exercised) at a small floor rather than literally every safepoint.
/// Overridable via `BROOD_RT_GC_FLOOR` (object count, K/M ok).
pub(in crate::core::heap) fn rt_gc_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            gc_count_env("BROOD_RT_GC_FLOOR").unwrap_or(256)
        } else {
            gc_count_env("BROOD_RT_GC_FLOOR").unwrap_or(4096)
        }
    })
}

/// How often a process runs the multigen drain free-attempt (the O·live-process
/// `report_parked_liveness` registry scan) at its RUNTIME safepoint while a drain is
/// armed: once every `RT_DRAIN_SCAN_STRIDE` safepoints, not every frame. See the
/// `rt_drain_tick` field for why (the pinned-generation scan storm).
pub(in crate::core::heap) const RT_DRAIN_SCAN_STRIDE: u32 = 64;

/// How often a process runs its **per-safepoint drain self-report** while a generation drain
/// is armed: once every `DRAIN_REPORT_STRIDE` safepoints, not every frame. During a `spawn`
/// fan-out the drain lingers (workers pin it until they exit), so every worker reaches the
/// report on nearly every safepoint — ~9 M calls for 10 k workers, 99.9 % of them no-op
/// re-confirmations by an already-acked process. Each call re-reads the shared `drain_epoch`
/// (periodically written as drains re-arm → its cache line bounces → a coherence miss), so at
/// that volume it dominated the residual `spawn` collector overhead. The throttle is a
/// per-heap `Cell` tick (no shared read), so a skipped frame costs nothing. Sound: a process
/// that turns clean acks within a stride (drain completes ≤ stride safepoints later) and an
/// exiting one is accounted at once by `drain_note_exit`. To keep completion prompt, the
/// process that *arms* a drain resets its own tick (see [`Heap::begin_gen_drain`]) so it
/// reports on its very next frame. Throttles ONLY the safepoint path
/// ([`crate::process::report_drain_liveness`]); the parked-process inspector and the
/// drain-completion tests call `report_gen_liveness` directly and stay unthrottled.
pub(in crate::core::heap) const DRAIN_REPORT_STRIDE: u32 = 64;

/// How often a process already found **dirty via Phase 2** (a RUNTIME handle embedded in
/// its LOCAL heap data) re-runs that O(heap) walk in the drain self-report, vs. reporting
/// its cached stale-dirty verdict: once every `P2_REVALIDATE_STRIDE` safepoints of the
/// current drain epoch. Bounds a data-pinned process's per-safepoint report to 1/stride of
/// the full-heap walk — without it a big-heap pinning process (e.g. the root over a growing
/// message backlog) re-walks its whole heap every safepoint, quadratic (the ~300× `spawn`
/// fan-out regression). Sound: a stale-dirty verdict only delays completion, and a process
/// that turns clean re-validates within a stride. See `runtime_gen_referenced_private`.
pub(in crate::core::heap) const P2_REVALIDATE_STRIDE: u32 = 64;

/// The Phase-1 counterpart of [`P2_REVALIDATE_STRIDE`], and the seed size above which it
/// applies.
///
/// Phase 1 (private roots + live arms) is the *cheap* probe and is deliberately
/// unthrottled, so a process that stops running draining-generation code acks on its very
/// next safepoint — the promptness the drain-completion tests rely on. "Cheap" holds only
/// while the seed is small, and one term in it is not bounded: `roots` is the VM operand /
/// env stack, so it grows with **recursion depth**. A process 100 000 frames deep seeds
/// hundreds of thousands of values per probe, and while it stays dirty it pays that on
/// every reporting safepoint — quadratic in depth, and in practice a run that stops making
/// progress (KI-14: one worker pinned at 100% CPU, the suite never finishing).
///
/// So throttle Phase 1 the same way Phase 2 already is, but **only for a large seed**: a
/// shallow process (every drain-completion test, and the overwhelming majority of real
/// ones) is below the threshold and keeps reporting on every safepoint, unchanged. Sound
/// for the same reason as Phase 2 — a stale-dirty verdict only delays drain completion, it
/// can never fabricate a clean ack, and a process that turns clean re-validates within a
/// stride. See `runtime_gen_referenced_private`.
pub(in crate::core::heap) const P1_REVALIDATE_STRIDE: u32 = 64;

/// Seed size (roots + env roots + dynamics + live arms) above which a dirty Phase-1 verdict
/// starts being cached between re-validations. See [`P1_REVALIDATE_STRIDE`].
pub(in crate::core::heap) const P1_LARGE_SEED: usize = 4096;

/// Live old-gen object count below which a **major** collection never fires —
/// the old-gen counterpart of [`gc_floor`]. Crucially this is **not** zeroed by
/// `BROOD_GC_STRESS`: stress makes *minor* collection fire at every safepoint
/// (its purpose), but a major every safepoint would recompact the whole old
/// generation on an incremental large-structure build — O(n²). Keeping a nonzero
/// floor makes majors periodic under stress (still exercised) and rare in normal
/// operation (the old gen grows to a few MB before a compaction reclaims tenured
/// garbage, so live tenured data isn't recopied often).
/// Growth factor for the major-collection threshold: after a major, the next one
/// fires when the old gen has grown this many× (was 2×). A larger factor makes
/// majors geometrically rarer during a large-structure build — where the old gen
/// is nearly all-live and a compaction copies everything for almost no reclaim —
/// at the cost of retaining more tenured garbage between majors (memory for speed).
/// Ceiling for the adaptive nursery threshold (see `collect`): the young gen may
/// grow to at most this many objects before a minor GC, regardless of old-gen size.
/// Bounds the transient young-garbage buffer for a large-heap churny process while
/// sitting far above real build working sets (~8M objects ≈ a few hundred MB).
pub(in crate::core::heap) const NURSERY_MAX: usize = 8 * 1024 * 1024;

/// Deep-value guard for the recursive heap walkers (`promote_in`, the GC
/// `flush_value`, `equal`, `hash_value_into`): each recurses per **car**-nesting
/// level (their cdr spines are already iterative), so a deep-but-legal immutable
/// value — a 60k-deep nested list is just data — overflowed the native stack
/// (found 2026-07-19/20 by the iolist deep-nesting test; CI SIGABRT). Each
/// recursion entry checks the remaining stack and, inside the red zone, grows in
/// a heap-backed segment (`stacker::maybe_grow`, rustc's own approach) instead
/// of overflowing. Cost when not growing: one thread-local read + compare per
/// level. The alternative — rewriting four bottom-up builders as explicit
/// two-phase stack machines — was rejected as far more complexity for the same
/// guarantee.
pub(in crate::core::heap) const WALKER_RED_ZONE: usize = 64 * 1024;

/// Segment size for a deep-walker stack grow — large enough that even a
/// million-deep value grows a handful of times, small enough to stay cheap.
pub(in crate::core::heap) const WALKER_STACK_CHUNK: usize = 1024 * 1024;

pub(in crate::core::heap) fn major_growth() -> usize {
    static G: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *G.get_or_init(|| {
        std::env::var("BROOD_MAJOR_GROWTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n >= 2)
            .unwrap_or(4)
    })
}

pub(in crate::core::heap) fn major_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            8192
        } else {
            // Overridable for tuning via `BROOD_GC_MAJOR` (object count, K/M ok).
            gc_count_env("BROOD_GC_MAJOR").unwrap_or(256 * 1024)
        }
    })
}

/// Nursery-pressure threshold (live object count) at or above which a minor
/// collection **tenures** survivors into the old generation; below it the minor
/// does a young **semi-space flip** (survivors stay in a fresh nursery) instead.
/// This is the *aging* policy: an object tenures only when it survives a
/// collection that followed real allocation pressure — never a premature one.
/// Stress-independent (unlike [`gc_floor`]) so that `BROOD_GC_STRESS=1`, which
/// fires a minor at *every* safepoint with a tiny nursery, always flips and so
/// never tenures transient garbage (which would otherwise bloat the old gen and
/// make majors recopy it — the adversarial-under-stress regression). A
/// long-lived structure still tenures once the nursery genuinely grows past this.
pub(in crate::core::heap) fn min_tenure() -> usize {
    static T: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    // Overridable for tuning via `BROOD_GC_TENURE` (object count, K/M ok).
    *T.get_or_init(|| gc_count_env("BROOD_GC_TENURE").unwrap_or(16 * 1024))
}

/// Default for the per-process GC **trace** flag, from the `BROOD_GC_TRACE` env
/// var (set it to trace the whole run — including the root process, which the
/// `(gc-trace …)` builtin can't reach before user code runs). Read once and
/// cached; `(gc-trace on/off)` overrides it per process at runtime.
pub(in crate::core::heap) fn gc_trace_default() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_GC_TRACE").is_some())
}
