//! VM work-attribution counters — the `perf-stats` feature (see
//! `docs/benchmarking.md`).
//!
//! Process-global atomic counters that attribute *where the VM spends work*:
//! dispatch (closure activations, call/global inline-cache hit vs miss), env
//! lookups (calls + total chain hops walked), allocation, inlined-prim hits vs
//! native fallbacks, self-tail-calls, and tree-walker defers. They aggregate
//! across every green process and worker thread (one shared static), which is
//! what you want for whole-workload attribution.
//!
//! **Zero cost when the feature is off.** [`perf_bump!`] expands to nothing, so a
//! normal or release build — and the timing benches — carry no counter overhead.
//! Build `--features perf-stats` to arm them, then read `(vm-stats)` or run with
//! `BROOD_PERF_STATS=1`.
//!
//! This is a *counting* tool, not a *timing* one: the atomics perturb timing, so
//! read **counts** from a `perf-stats` build and **times** from the counter-free
//! benches (the load-invariant VM/tree-walker ratio — see `docs/benchmarking.md`).
//! The counts answer the bytecode-lowering gate (ADR-096): is the VM
//! dispatch-bound (high calls / IC misses), env-bound (high `env_hops`), or
//! alloc-bound (high `alloc` / collections)?

#[cfg(feature = "perf-stats")]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};

    macro_rules! declare {
        ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {
            /// One `AtomicU64` per attributed event; see [`perf_bump!`].
            pub struct Counters { $($(#[$doc])* pub $name: AtomicU64,)+ }
            /// The process-global counter set (shared across all process heaps).
            pub static C: Counters = Counters { $($name: AtomicU64::new(0),)+ };
            /// `(name, value)` for every counter, in declaration order.
            pub fn snapshot() -> Vec<(&'static str, u64)> {
                vec![ $((stringify!($name), C.$name.load(Ordering::Relaxed)),)+ ]
            }
            /// Zero every counter (for a clean measurement window).
            pub fn reset() { $(C.$name.store(0, Ordering::Relaxed);)+ }
        };
    }

    declare!(
        /// Closure activations run on the VM (`vm_apply`).
        vm_apply,
        /// Calls that fell back to the tree-walker (`eval::apply`) — the deopt
        /// surface; ideally near-zero for hot code.
        tw_defer,
        /// Call-site inline cache: served from the cache (skipped the resolve).
        call_ic_hit,
        /// Call-site inline cache: missed (resolved + installed, or disengaged
        /// for a local-capturing env).
        call_ic_miss,
        /// Global-read inline cache hit.
        global_ic_hit,
        /// Global-read inline cache miss.
        global_ic_miss,
        /// `Node::Prim2` ran the inlined fast path (no dispatch).
        prim2_inline,
        /// `Node::Prim2` fell back to the real native (non-inline operands /
        /// redefined operator).
        prim2_fallback,
        /// `Node::Prim1` (`first`/`rest`) ran inline.
        prim1_inline,
        /// `Node::Prim1` fell back to the native.
        prim1_fallback,
        /// Tail-call trampoline iterations (`Step::Tail` — a tail call into the
        /// same or another arm; the loop body of tail recursion).
        tail_call,
        /// Direct `letrec` self-tail-calls (`Step::SelfTail`).
        self_tail,
        /// `env_get` calls (a name resolution through the env chain).
        env_get,
        /// Total env-chain frames walked across all `env_get`s — the env-lookup
        /// cost signal (high ⇒ env-bound, a lexical-addressing target).
        env_hops,
        /// LOCAL heap allocations (pairs/vectors/maps/strings/closures/…).
        alloc,
    );
}

#[cfg(feature = "perf-stats")]
pub use imp::{reset, C};

/// Snapshot of all counters as `(name, value)` pairs, or `None` when the
/// `perf-stats` feature is off. Read by the `(vm-stats)` builtin.
#[cfg(feature = "perf-stats")]
pub fn snapshot() -> Option<Vec<(&'static str, u64)>> {
    Some(imp::snapshot())
}

/// No-op stub: counters are compiled out without the `perf-stats` feature.
#[cfg(not(feature = "perf-stats"))]
pub fn snapshot() -> Option<Vec<(&'static str, u64)>> {
    None
}

/// Reset is a no-op without the feature.
#[cfg(not(feature = "perf-stats"))]
pub fn reset() {}

/// If `BROOD_PERF_STATS` is set (and not `0`), print the counter snapshot to
/// stderr — the whole-program profiling dump the binaries call after a run. With
/// the feature off it prints a one-line hint so the flag never silently no-ops.
pub fn dump_if_requested() {
    match std::env::var("BROOD_PERF_STATS") {
        Ok(v) if v != "0" && !v.is_empty() => {}
        _ => return,
    }
    match snapshot() {
        Some(counters) => {
            eprintln!("[vm-perf] work-attribution counters (process-global totals):");
            for (name, val) in counters {
                eprintln!("[vm-perf] {:>16}  {}", name, val);
            }
        }
        None => eprintln!(
            "[vm-perf] BROOD_PERF_STATS set, but this binary was built without \
             `--features perf-stats` — counters are compiled out (all zero)."
        ),
    }
}

/// Increment a [`perf`](self) counter — `perf_bump!(name)` by one, or
/// `perf_bump!(name, n)` by `n`. Expands to **nothing** without the `perf-stats`
/// feature, so it is free to sprinkle on the hot path. `name` is a field of
/// [`imp::Counters`].
#[cfg(feature = "perf-stats")]
#[macro_export]
macro_rules! perf_bump {
    ($field:ident) => {
        $crate::perf::C.$field.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed)
    };
    ($field:ident, $n:expr) => {
        $crate::perf::C.$field.fetch_add($n as u64, ::std::sync::atomic::Ordering::Relaxed)
    };
}

/// No-op form without the feature (drops its arguments unevaluated — callers pass
/// only already-computed cheap values).
#[cfg(not(feature = "perf-stats"))]
#[macro_export]
macro_rules! perf_bump {
    ($field:ident) => {{}};
    ($field:ident, $n:expr) => {{}};
}
