//! The observability primitives: the sampling profiler, line/branch coverage
//! (ADR-148), and the debugger's per-process trace context. The mechanisms live in
//! `crate::diagnostics`; these are the Brood-facing handles on them.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::arg;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // Line-coverage readout (ADR-148 tier 2). Empty unless BROOD_COVERAGE is set.
    primitives.def(
        "%coverage-lines",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        &[],
        "Every source line recorded as EXECUTED, as a list of [file (line …)]. Empty unless the run was started with BROOD_COVERAGE=1 (`nest test --cover-lines`).",
        coverage_lines);
    primitives.def(
        "%coverage-instrumented",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        &[],
        "Every source line the compiler INSTRUMENTED, as a list of [file (line …)] — the denominator %coverage-lines is a subset of. Arms compile when defined, so a never-called function appears here and not there.",
        coverage_instrumented);
    primitives.def(
        "%coverage-branches",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        &[],
        "Every branch edge recorded as taken, as [file ([line col taken] …)]. A branch is fully covered when both edges (taken true and false) appear for one [line col]. Empty unless BROOD_COVERAGE=1 (`nest test --cover-branches`).",
        coverage_branches);
    primitives.def(
        "%coverage-branch-instrumented",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        &[],
        "Every [line col] branch point the compiler INSTRUMENTED, as [file ([line col] …)] — the branch denominator (each needs both edges taken for full coverage).",
        coverage_branch_instrumented);
    primitives.def(
        "%coverage-precompile",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["f"],
        "Compile f's body now, without calling it, so its lines count toward %coverage-instrumented. Returns true if a body was compiled.",
        coverage_precompile);
    primitives.def(
        "%coverage-reset",
        Arity::exact(0),
        Sig::new(vec![], nil_ty),
        &[],
        "Forget every line recorded by %coverage-lines, so a long-lived image can measure more than once without runs bleeding together.",
        coverage_reset);
    // The debugger's durable per-process causal context (ADR-174 send-level slice) —
    // `dev-tools` only, so a lean release registers neither (the `debug` module that
    // uses them is a DEV_MODULE too).
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%trace-context",
        Arity::exact(0),
        Sig::new(vec![], any),
        &[],
        "",
        trace_context_get,
    );
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%set-trace-context",
        Arity::exact(1),
        Sig::new(vec![any], any),
        &[],
        "",
        trace_context_set,
    );
    primitives.def(
        "%profile-start",
        Arity::range(0, 1),
        Sig::new(vec![any], nil_ty),
        &["&optional", "hz"],
        "Arm the sampling CPU profiler at hz samples/sec (default 99, clamped 1..10000), resetting the histogram. Sampling walks each process's reified call stack (named frames) at its next VM frame boundary after every tick — no signals, near-zero cost when off (one relaxed load per frame boundary). A JIT-resident loop is attributed when it yields at its reduction-budget preempt (~once a quantum); the legacy tree-walker isn't sampled. Stop and read with (dev/profile-stop).",
        profile_start);
    primitives.def(
        "%profile-stop",
        Arity::exact(0),
        Sig::nullary(list_ty),
        &[],
        "Disarm the sampling profiler and return the histogram: a list of {:stack (fn-names... innermost-first) :count n} maps, most-sampled first. Empty list if never armed. A sample whose frames were all anonymous appears with :stack (\"<anonymous>\").",
        profile_stop);
    // memory
    primitives.def(
        "%mem-bytes",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "Bytes currently allocated process-wide.",
        mem_bytes,
    );
    primitives.def(
        "%mem-peak",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "High-water mark of allocated bytes since process start.",
        mem_peak,
    );
    primitives.def(
        "%mem-limit",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "Hard memory ceiling in bytes (0 = unlimited); crossing it aborts the process. Set via BROOD_MEM_LIMIT.",
        mem_limit);
    primitives.def(
        "%mem-soft-limit",
        Arity::exact(0),
        Sig::nullary(int),
        &[],
        "Soft memory ceiling in bytes (0 = unlimited); crossing it raises a catchable E0043 at the next safepoint.",
        mem_soft_limit);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%ic-stats",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "A snapshot map of this process's four inline-cache tables — the largest single attributed item in the green-process floor (896 B/process, bigger than the whole Box<Process>). :calls, :links, :globals and :blocks each give [len capacity bytes], with bytes from live CAPACITY x the real element size, so a Vec grown past its contents reports the memory it actually holds. :call-entry-bytes is the size of one CallIcEntry slot — the figure a shrink of that struct would move — and :total-bytes their sum. Size these at the PARKED state, not at teardown: a teardown slot count read 3.5x too high (docs/runtime-frontier.md, 2026-08-18).",
        ic_stats);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%pos-stats",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "A snapshot map of the two source-position side tables: :local-forms/:local-cap/:local-bytes for this process's LOCAL `form_pos`, and :runtime-forms/:runtime-cap/:runtime-bytes for the runtime-shared `positions`. Bytes are derived from live CAPACITY, not entry count, so a table holding its high-water capacity after a collection reports the memory it actually occupies. The measurement surface for what positions cost: on the 38-module stdlib they are ~7% of load memory and ~6.5% of load time (2026-08-20), well under the 18%/24% a synthetic 1000-line-module corpus suggested on 2026-08-06.",
        pos_stats);
    // GC debug/introspection builtins — dev surface only. A lean `nest release`
    // runtime (`--no-default-features`) omits them so a shipped app carries no
    // debug instrumentation (ADR-038). Their fn defs are gated to match.
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%gc-stats",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "A snapshot map of GC activity: :collections, :copied, :reclaimed (cumulative object counts), :live, :live-bytes, :threshold (next-collection trigger), and the pause-duration trio :pause-total-us/:pause-max-us/:pause-last-us (cumulative wall time in collections, worst single pause, most recent — the timing tier) for the caller's own LOCAL heap; :runtime-closures and :runtime-threshold for the *shared* RUNTIME code region (its promoted-closure count + next auto-compact trigger — same for every process); and :debug-build (true if built with debug assertions — not a perf build). The LOCAL figures are per-process; use (dev/runtime-collect) for the RUNTIME live/reclaimable split.",
        gc_stats);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%tree-walker?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "True when this run's tier CEILING is the tree-walker (ADR-222) — the differential reference engine. Asks the runtime rather than the environment on purpose: the ceiling is selected by `BROOD_TIER=0` and by its older `BROOD_VM=0` alias, so a test that reads one env var directly silently stops guarding when the other spelling is used. `eval::compile::tier_ceiling` is the single source of truth.",
        tree_walker_p);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%vm-stats",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "A snapshot map of VM work-attribution counters (the perf-stats feature). :enabled is false unless the binary was built with --features perf-stats; when true, process-global cumulative totals: :vm-apply (closure activations), :tail-call/:self-tail (trampoline iterations), :tw-defer (tree-walker fallbacks), :call-ic-hit/:call-ic-miss, :global-ic-hit/:global-ic-miss, :prim2-inline/:prim2-fallback, :prim1-inline/:prim1-fallback, :env-get/:env-hops (lookups + chain frames walked), :alloc (LOCAL allocations). Tells you whether the VM is dispatch-, env-, or alloc-bound. A counting tool, not a timing one — read times from the benches (docs/benchmarking.md).",
        vm_stats);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%vm-stats-reset",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "Zero the VM work-attribution counters, returning :enabled (false without --features perf-stats). The counters are process-global and cumulative FROM PROCESS START, so a snapshot after a short program also counts the runtime's own boot — which is macro-expansion-heavy and defers to the tree-walker (measured: the same program read an 84% defer rate cold-cache and 0.8% warm). Zero first when measuring a region; `(perf/measure thunk)` packages that.",
        vm_stats_reset);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%gc-collect",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "Force a collection of this process's LOCAL heap now, returning the post-collection gc-stats map. An observability/test aid, not a load-bearing trigger — automatic collection at the eval safepoint already keeps memory bounded.",
        gc_collect);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%runtime-collect",
        Arity::exact(0),
        Sig::nullary(map_ty),
        &[],
        "Compact the shared RUNTIME code region, reclaiming superseded versions of redefined globals (hot-reload churn). Returns {:before N :after M :reclaimed (N-M) :ran bool} (closure counts). Runs only when this runtime is uniquely owned (no other live process) — otherwise :ran is false and nothing changes. Usually unnecessary: the eval safepoint auto-compacts once hot-reload churn crosses a threshold (single-process); this forces it now. ADR-076 follow-up / docs/runtime-collector-exploration.md.",
        runtime_collect);
    #[cfg(feature = "dev-tools")]
    primitives.def(
        "%gc-trace",
        Arity::range(0, 1),
        Sig::new(vec![any], bool_ty),
        &["on?"],
        "Query (no arg) or set (truthy arg) per-collection GC trace logging for this process; returns the resulting state. When on, each minor/major collection prints a one-line summary to stderr. Defaulted from BROOD_GC_TRACE.",
        gc_trace);
}

/// `(profile-start [hz])` — arm the sampling CPU profiler at `hz` samples/sec
/// (default 99, clamped 1..10000). Resets the histogram; see `profile-stop`.
pub(super) fn profile_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let hz = match arg(args, 0) {
        Value::Nil => 99,
        Value::Int(n) if n > 0 => n.min(10_000) as u32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "profile-start",
                "positive int (hz) or absent",
                other,
            ))
        }
    };
    crate::diagnostics::profile::start(hz);
    Ok(Value::nil())
}

/// `(%)` — disarm the sampling profiler and return the histogram: a
/// list of `{:stack (fn-names… innermost-first) :count n}` maps, most-sampled
/// first. A sample whose frames were all anonymous appears with `:stack
/// ("<anonymous>")`.
pub(super) fn profile_stop(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let entries = crate::diagnostics::profile::stop();
    let items: Vec<Value> = entries
        .iter()
        .map(|(stack, count)| {
            let names: Vec<Value> = if stack.is_empty() {
                vec![heap.alloc_string("<anonymous>")]
            } else {
                stack
                    .iter()
                    .map(|&s| heap.alloc_string(value::symbol_name_ref(s)))
                    .collect()
            };
            let stack_list = heap.list(names);
            let pairs = vec![
                (value::kw("stack"), stack_list),
                (value::kw("count"), Value::int(*count as i64)),
            ];
            heap.map_from_pairs(pairs)
        })
        .collect();
    Ok(heap.list(items))
}

/// `(%trace-context)` — the debugger's durable per-process causal context (ADR-174),
/// or nil. A settable per-process slot (unlike a `binding`): `spawn` copies it into a
/// child, `send` ships it, `receive` overwrites it on pop. `dev-tools` only.
#[cfg(feature = "dev-tools")]
pub(super) fn trace_context_get(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.trace_context().unwrap_or(Value::Nil))
}

/// `(%set-trace-context ctx)` — set (or, with nil, clear) the per-process trace
/// context. Returns nil. `dev-tools` only.
#[cfg(feature = "dev-tools")]
pub(super) fn trace_context_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Set from Brood (`with-debugger`/`span`) → this is the process's OWN context,
    // so `spawn` propagates it. Message-adoption uses `own = false` (in the mailbox).
    heap.set_trace_context(Some(arg(args, 0)), true);
    Ok(Value::Nil)
}

/// `[file (line …)]` pairs, the shape both coverage readouts return.
fn coverage_pairs(entries: Vec<(String, Vec<u32>)>, heap: &mut Heap) -> LispResult {
    let mut out = Vec::new();
    for (file, lines) in entries {
        let file_val = heap.alloc_string(&file);
        let line_vals: Vec<Value> = lines.iter().map(|l| Value::int(i64::from(*l))).collect();
        let lines_val = heap.list(line_vals);
        out.push(heap.alloc_vector2(file_val, lines_val));
    }
    Ok(heap.list(out))
}

/// `(%coverage-lines)` — every line recorded as EXECUTED, as a list of
/// `[file (line …)]`. Empty unless the run was started with `BROOD_COVERAGE=1`
/// (which `nest test --cover-lines` sets before building the prelude).
pub(super) fn coverage_lines(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    coverage_pairs(crate::diagnostics::coverage::snapshot(), heap)
}

/// `(%coverage-instrumented)` — every line the compiler INSTRUMENTED, same shape. The
/// denominator for a percentage: without it the two halves of the ratio would come
/// from different populations (see `coverage.rs`). A never-called function is present
/// here and absent from `%coverage-lines` — provided it was forced through
/// `%coverage-precompile` first, since arms otherwise compile on first call.
pub(super) fn coverage_instrumented(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    coverage_pairs(crate::diagnostics::coverage::instrumented(), heap)
}

/// `[file ([line col taken] …)]` pairs — the shape the branch-hit readout returns.
fn coverage_branch_pairs(
    entries: Vec<(String, Vec<(u32, u32, bool)>)>,
    heap: &mut Heap,
) -> LispResult {
    let mut out = Vec::new();
    for (file, edges) in entries {
        let file_val = heap.alloc_string(&file);
        let edge_vals: Vec<Value> = edges
            .iter()
            .map(|(line, col, taken)| {
                let items = vec![
                    Value::int(i64::from(*line)),
                    Value::int(i64::from(*col)),
                    Value::boolean(*taken),
                ];
                heap.alloc_vector(items)
            })
            .collect();
        let edges_val = heap.list(edge_vals);
        out.push(heap.alloc_vector2(file_val, edges_val));
    }
    Ok(heap.list(out))
}

/// `(%coverage-branches)` — every branch edge recorded as taken, as
/// `[file ([line col taken] …)]`. A branch is fully covered when both `taken` edges
/// (`true` and `false`) appear for one `[line col]`. Empty unless `BROOD_COVERAGE=1`.
pub(super) fn coverage_branches(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    coverage_branch_pairs(crate::diagnostics::coverage::branch_snapshot(), heap)
}

/// `(%coverage-branch-instrumented)` — every `[line col]` decision point the compiler
/// instrumented, as `[file ([line col] …)]` — the branch denominator (two edges each).
pub(super) fn coverage_branch_instrumented(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let entries = crate::diagnostics::coverage::branch_instrumented();
    let mut out = Vec::new();
    for (file, sites) in entries {
        let file_val = heap.alloc_string(&file);
        let site_vals: Vec<Value> = sites
            .iter()
            .map(|(line, col)| {
                heap.alloc_vector(vec![
                    Value::int(i64::from(*line)),
                    Value::int(i64::from(*col)),
                ])
            })
            .collect();
        let sites_val = heap.list(site_vals);
        out.push(heap.alloc_vector2(file_val, sites_val));
    }
    Ok(heap.list(out))
}

/// `(%coverage-precompile f)` — compile `f`'s body now, without calling it, so its
/// lines land in `%coverage-instrumented`. Returns true if a body was compiled.
/// See `eval::compile::precompile` for why the denominator needs this.
pub(super) fn coverage_precompile(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::boolean(crate::eval::compile::precompile(
        heap, args[0],
    )))
}

/// `(%coverage-reset)` — forget every recorded line, so a long-lived image can
/// measure more than once without runs bleeding together.
pub(super) fn coverage_reset(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    crate::diagnostics::coverage::reset();
    Ok(Value::nil())
}

// ---------- memory ----------

/// `(%)` — bytes currently allocated across the whole process.
pub(super) fn mem_bytes(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::live_bytes() as i64))
}

/// `(%)` — high-water mark of allocated bytes since the process started.
pub(super) fn mem_peak(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::peak_bytes() as i64))
}

/// `(%)` — live sizes of this process's four inline-cache tables, the
/// largest single attributed item in the green-process floor (`FRONTIER.md` lever 1
/// puts them at 896 B/process, bigger than the whole `Box<Process>`). Each of
/// `:calls` (`vm_call_ics`), `:links` (`vm_fast_links`), `:globals` (`vm_global_ics`)
/// and `:blocks` (`arm_ic_blocks`) reports `[len capacity bytes]`, with bytes from
/// live CAPACITY x the real element size — a `Vec` grown past its contents holds that
/// memory whatever its length says. `:call-entry-bytes` is `size_of::<Option<CallIcEntry>>()`,
/// the figure a shrink of that struct would move. Per-process; size it at the PARKED
/// state, not at teardown (a teardown slot count read 3.5x too high — see the
/// 2026-08-18 note in `docs/runtime-frontier.md`).
#[cfg(feature = "dev-tools")]
pub(super) fn ic_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (t, entry) = heap.ic_table_stats();
    fn triple(heap: &mut Heap, (l, c, b): (usize, usize, usize)) -> Value {
        heap.alloc_vector(vec![
            Value::int(l as i64),
            Value::int(c as i64),
            Value::int(b as i64),
        ])
    }
    let calls = triple(heap, t[0]);
    let links = triple(heap, t[1]);
    let globals = triple(heap, t[2]);
    let blocks = triple(heap, t[3]);
    let total: usize = t.iter().map(|x| x.2).sum();
    let pairs = vec![
        (value::kw("calls"), calls),
        (value::kw("links"), links),
        (value::kw("globals"), globals),
        (value::kw("blocks"), blocks),
        (value::kw("call-entry-bytes"), Value::int(entry as i64)),
        (value::kw("total-bytes"), Value::int(total as i64)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(%)` — entry counts of the two source-position side tables:
/// `:local-forms` (this process's LOCAL `form_pos`) and `:runtime-forms` (the
/// runtime-shared `positions`). Measurement surface for the position-table cost,
/// which the 2026-08-06 module-load breakdown put at 169 MB of a 933 MB load and
/// 24% of load time. Per-process for the LOCAL half, runtime-wide for the other.
#[cfg(feature = "dev-tools")]
pub(super) fn pos_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (local, local_cap, runtime, rt_cap) = heap.pos_table_stats();
    let (local_bytes, rt_bytes) = heap.pos_table_bytes();
    let pairs = vec![
        (value::kw("local-forms"), Value::int(local as i64)),
        (value::kw("local-cap"), Value::int(local_cap as i64)),
        (value::kw("local-bytes"), Value::int(local_bytes as i64)),
        (value::kw("runtime-forms"), Value::int(runtime as i64)),
        (value::kw("runtime-cap"), Value::int(rt_cap as i64)),
        (value::kw("runtime-bytes"), Value::int(rt_bytes as i64)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(%)` — a snapshot map of this process's garbage-collection activity
/// (Tier-1 observability; `docs/memory-review.md` §7). Per-process: it reports
/// the *calling* process's own LOCAL heap, never another's. Keys:
/// `:collections` (collections run since start — the automatic Stage-B
/// safepoint copies), `:copied` (cumulative LOCAL
/// objects relocated by those collections), `:reclaimed` (cumulative LOCAL
/// objects dropped), `:live` (LOCAL objects live right now), `:live-bytes` (a
/// cheap byte estimate of the LOCAL slabs — see `mem-bytes` for the process-wide
/// figure), and `:threshold` (the live count that triggers the next collection —
/// the slow/stable dial). Plus two figures for the *shared* RUNTIME code region
/// (the same for every process, not per-process): `:runtime-closures` (its total
/// promoted-closure count — grows with hot-reload churn, compacted back by the
/// safepoint, ADR-091) and `:runtime-threshold` (the count that triggers the next
/// auto-compaction). The live/reclaimable split is the expensive walk reported by
/// `(dev/runtime-collect)`, so it's not included here.
#[cfg(feature = "dev-tools")]
pub(super) fn gc_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(gc_stats_map(heap))
}

/// Build the `(%)` snapshot map of the calling process's GC activity.
/// Shared by `gc-stats` and `gc-collect` (which reports the same shape *after*
/// forcing a collection, so the delta is visible).
#[cfg(feature = "dev-tools")]
pub(super) fn gc_stats_map(heap: &mut Heap) -> Value {
    let (runs, copied, reclaimed) = heap.gc_counters();
    let pairs = vec![
        (value::kw("collections"), Value::int(runs as i64)),
        (value::kw("copied"), Value::int(copied as i64)),
        (value::kw("reclaimed"), Value::int(reclaimed as i64)),
        (
            value::kw("live"),
            Value::int(heap.local_live_count() as i64),
        ),
        (
            value::kw("live-bytes"),
            Value::int(heap.local_bytes() as i64),
        ),
        (
            value::kw("threshold"),
            Value::int(heap.gc_threshold() as i64),
        ),
        // Pause durations (the observability timing tier): cumulative wall time
        // spent in this process's collections, the worst single pause, and the
        // most recent one — µs so the numbers stay readable ints (a minor
        // collection is µs-scale; a bad pause ms-scale).
        (
            value::kw("pause-total-us"),
            Value::int((heap.gc_pause_ns().0 / 1_000) as i64),
        ),
        (
            value::kw("pause-max-us"),
            Value::int((heap.gc_pause_ns().1 / 1_000) as i64),
        ),
        (
            value::kw("pause-last-us"),
            Value::int((heap.gc_pause_ns().2 / 1_000) as i64),
        ),
        // The shared RUNTIME code region (not per-process — every process sees the
        // same figure). `:runtime-closures` is its total promoted-closure count
        // (cheap — a slab length); it grows with hot-reload churn and the eval
        // safepoint compacts it back toward `:runtime-threshold` (single-process
        // today, ADR-091). The live/reclaimable split is the expensive walk reported
        // by `(dev/runtime-collect)`'s `{:before :after :reclaimed}`, kept out of here.
        (
            value::kw("runtime-closures"),
            Value::int(heap.runtime_closure_count() as i64),
        ),
        (
            value::kw("runtime-threshold"),
            Value::int(heap.rt_gc_threshold() as i64),
        ),
        // True iff this binary was built with debug assertions (the GC tripwire /
        // verifier / poison bits are compiled in) — so a benchmark can confirm
        // it's measuring a clean release build, not a debug-armed one. `false`
        // for `make install` / `cargo build --release`.
        (
            value::kw("debug-build"),
            Value::boolean(cfg!(debug_assertions)),
        ),
    ];
    heap.map_from_pairs(pairs)
}

/// `(%)` — a snapshot map of the VM work-attribution counters (the
/// `perf-stats` feature; see `docs/benchmarking.md`). `:enabled` is `false` when
/// the binary was built without `--features perf-stats` (every other key absent —
/// the counters compiled to nothing). With the feature on: `:enabled true` plus a
/// key per counter (`:vm-apply`, `:tail-call`, `:self-tail`, `:tw-defer`,
/// `:call-ic-hit`/`:call-ic-miss`, `:global-ic-hit`/`:global-ic-miss`,
/// `:prim2-inline`/`:prim2-fallback`, `:prim1-inline`/`:prim1-fallback`,
/// `:env-get`, `:env-hops`, `:alloc`) — process-global cumulative totals across
/// every green process. The data behind the bytecode-lowering gate (ADR-096): is
/// the VM dispatch-, env-, or alloc-bound? A *counting* tool, not a timing one.
#[cfg(feature = "dev-tools")]
/// `(%tree-walker?)` — is this run's tier ceiling the tree-walker (ADR-222)?
///
/// Exists so a test can ask the RUNTIME which engine it is on instead of re-deriving it
/// from an environment variable. Two spellings select tier 0 — `BROOD_TIER=0` and the
/// older `BROOD_VM=0` alias — and a guard written against one of them silently stops
/// guarding under the other: `tests/observability_test.blsp` asserted the sampling
/// profiler had collected frames, which the tree-walker never produces, and failed under
/// `BROOD_TIER=0` because it only recognised `BROOD_VM`. Same engine, unrecognised
/// spelling. `tier_ceiling` is the one place that decides.
pub(super) fn tree_walker_p(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    let is_tw = matches!(
        crate::eval::compile::tier_ceiling(),
        crate::eval::compile::Tier::TreeWalk
    );
    Ok(Value::boolean(is_tw))
}

// Registered only under `dev-tools` (mod.rs's DEV block, whose comment says the fn defs
// are gated to match — this one wasn't).
#[cfg(feature = "dev-tools")]
pub(super) fn vm_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pairs = match crate::diagnostics::perf::snapshot() {
        Some(counters) => {
            let mut v = Vec::with_capacity(counters.len() + 1);
            v.push((value::kw("enabled"), Value::boolean(true)));
            for (name, val) in counters {
                // counter idents are snake_case; expose idiomatic kebab keywords.
                v.push((value::kw(&name.replace('_', "-")), Value::int(val as i64)));
            }
            v
        }
        None => vec![(value::kw("enabled"), Value::boolean(false))],
    };
    Ok(heap.map_from_pairs(pairs))
}

/// `(%)` — zero the work-attribution counters, returning `:enabled`.
///
/// The counters are **process-global and cumulative from process start**, so a snapshot
/// taken after a short program includes the runtime's own boot work — and boot is
/// macro-expansion-heavy, which defers to the tree-walker. Measured: the same
/// list-building program read an 84% defer rate on a cold boot cache and 0.8% on a warm
/// one, purely from whether expansion ran. Anything measuring a *region* rather than a
/// whole process must zero first; `(perf/measure thunk)` in `std/tool/perf.blsp` is that,
/// packaged.
///
/// A no-op returning `:enabled false` without `--features perf-stats`, like `(%)`.
#[cfg(feature = "dev-tools")]
pub(super) fn vm_stats_reset(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::diagnostics::perf::reset();
    let enabled = crate::diagnostics::perf::snapshot().is_some();
    Ok(heap.map_from_pairs(vec![(value::kw("enabled"), Value::boolean(enabled))]))
}

/// `(dev/runtime-collect)` — compact the shared RUNTIME code region now (reclaim
/// superseded hot-reload versions), returning `{:before :after :reclaimed :ran}`.
/// `:ran` is false (and nothing changes) when the runtime is shared with another
/// live process — see [`Heap::runtime_collect`]'s safety gate. Rarely needed: the
/// eval safepoint auto-compacts ([`Heap::maybe_runtime_collect`]) once churn
/// crosses the threshold; this is the explicit/force form.
#[cfg(feature = "dev-tools")]
pub(super) fn runtime_collect(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (before, after, ran) = match heap.runtime_collect() {
        Some((b, a)) => (b, a, true),
        None => {
            let n = heap.runtime_closure_count();
            (n, n, false)
        }
    };
    let pairs = vec![
        (value::kw("before"), Value::int(before as i64)),
        (value::kw("after"), Value::int(after as i64)),
        (value::kw("reclaimed"), Value::int((before - after) as i64)),
        (value::kw("ran"), Value::boolean(ran)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(%)` — force a collection of this process's LOCAL heap *now*,
/// returning the post-collection `(%)` map so the effect is visible.
/// An observability/test aid, **not** a load-bearing trigger: automatic
/// collection at the eval safepoint keeps memory bounded with no help from the
/// program (the removed `(hibernate)` was the load-bearing manual trigger — this
/// is not its return). Safe at any eval depth: a nullary builtin holds no
/// un-rooted LOCAL values across the collection, and every live ancestor frame
/// is already on the operand stack (ADR-061), so `collect` relocates everything
/// reachable and the freshly-built result map is allocated post-collection.
#[cfg(feature = "dev-tools")]
pub(super) fn gc_collect(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    heap.collect(&mut [], &mut []);
    Ok(gc_stats_map(heap))
}

/// `(%)` / `(gc-trace on?)` — query or set per-collection GC trace
/// logging for the calling process. With no argument, returns the current state;
/// with one, sets it (truthy = on) and returns the new state. When on, each
/// minor/major collection prints a one-line summary to stderr. Per-process and
/// defaulted from the `BROOD_GC_TRACE` env var (which traces the whole run,
/// including the root process before any `(%)` call).
#[cfg(feature = "dev-tools")]
pub(super) fn gc_trace(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let Some(&v) = args.first() {
        heap.set_gc_trace(crate::eval::truthy(v));
    }
    Ok(Value::boolean(heap.gc_trace()))
}

/// `(%)` — the hard memory ceiling in bytes (0 = unlimited). ADR-043.
pub(super) fn mem_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::hard_limit() as i64))
}

/// `(%)` — the soft memory ceiling in bytes (0 = unlimited). ADR-043.
pub(super) fn mem_soft_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::soft_limit() as i64))
}
