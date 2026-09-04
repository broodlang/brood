//! The `BROOD_*` diagnostic-flag catalogue — what `brood --debug-flags` prints.
//!
//! The runtime has ~75 environment flags, and they *are* the performance toolkit: which
//! one to reach for was tribal knowledge, discoverable only by reading `CLAUDE.md`'s
//! table, which is not shipped in the binary. This makes the answer available from the
//! binary itself (`docs/backend-seams.md` §5).
//!
//! **The complete list, in triage order.** It began as a curated performance subset, on
//! the reasoning that a GUI or REPL flag does not belong in a list you consult while
//! chasing a regression. What that produced instead was a gap of 43 flags out of 101 —
//! including the worker count, the reduction budget and every GC tuning knob, none of
//! which is anything but performance. A flag nothing documents is a flag nobody reaches
//! for, so the answer is ordering, not omission: the triage groups print first
//! ([`GROUP_ORDER`]) and the environment ones last. `CLAUDE.md`'s table remains the long
//! form — it carries the measurement history behind each default, which is what you want
//! when deciding whether to flip one, and is far too long to print.
//!
//! Both directions are gated by the tests below. A catalogued flag must still appear in
//! the source, so a rename cannot leave a line telling a reader to set something the
//! runtime ignores; and every `BROOD_*` the runtime reads must appear here, so the gap
//! cannot re-open one flag at a time. The build-time names are exempted explicitly
//! ([`NON_FLAGS`]) rather than by being forgotten.

/// One catalogued flag: name, the group it belongs to, and what it does in one line.
pub struct DebugFlag {
    pub name: &'static str,
    pub group: &'static str,
    pub effect: &'static str,
    /// False when the flag belongs to a *dependency* rather than to brood — read by the
    /// allocator, the OS, a linked library. Those are worth listing (they move real numbers)
    /// but brood's own source never mentions them, so the drift test must not expect it to.
    pub ours: bool,
}

const fn f(name: &'static str, group: &'static str, effect: &'static str) -> DebugFlag {
    DebugFlag {
        name,
        group,
        effect,
        ours: true,
    }
}

/// A flag a *dependency* reads, not brood. Listed because it moves measurements; exempt from
/// the source-presence test for the same reason.
const fn external(name: &'static str, group: &'static str, effect: &'static str) -> DebugFlag {
    DebugFlag {
        name,
        group,
        effect,
        ours: false,
    }
}

const ATTRIBUTION: &str = "Attribution — where does the work go";
const JIT: &str = "JIT — did it lower, and what did it emit";
const OPTOUT: &str =
    "Optimizer levers — the A/B and bisect switches (every `NO_` one is default ON)";
const GC: &str = "GC and memory";
const SCHED: &str = "Scheduler and messaging";
const ENGINE: &str = "Engine selection";
const DIAG: &str = "Diagnostics, checking and reloading";
const HOST: &str = "Host environment — GUI, audio, distribution, REPL";

/// Print order. A group absent here would never print, so a test asserts every flag's
/// group appears — the entries themselves may then sit anywhere in [`FLAGS`], which is
/// what lets a new one be added beside its siblings without reordering the array.
const GROUP_ORDER: &[&str] = &[ATTRIBUTION, JIT, OPTOUT, GC, SCHED, ENGINE, DIAG, HOST];

/// The catalogue, in print order.
pub const FLAGS: &[DebugFlag] = &[
    // ---- attribution ----
    f(
        "BROOD_PERF_STATS",
        ATTRIBUTION,
        "dump the VM work-attribution counters at exit; needs `make perf-brood`. In-image: `(perf/summary)`",
    ),
    f(
        "BROOD_BOOT_TRACE",
        ATTRIBUTION,
        "cold-start phase breakdown (builtins/read/expand/eval/freeze) + any form expanding >300us",
    ),
    f(
        "BROOD_TRACE_COMPILE",
        ATTRIBUTION,
        "name every closure the VM bytecode-compiles — answers \"is a compiled body being reused?\"",
    ),
    f(
        "BROOD_TRACE_PROMOTE",
        ATTRIBUTION,
        "name every closure entering the append-only RUNTIME region; per-operation promotion is a leak",
    ),
    f(
        "BROOD_REG_TRACE",
        ATTRIBUTION,
        "trace *record-ids* registry writes (with the writer's ancestry chain) and every globals restore — the KI-89 orphan-attribution tool; trace LEAN, heavy tracing suppresses the race",
    ),
    f(
        "BROOD_L1_STATS",
        ATTRIBUTION,
        "hit rate of the L1 local-send fast path — check it applied before crediting a message result",
    ),
    f(
        "BROOD_DEFER_DBG",
        ATTRIBUTION,
        "name each closure that defers to the tree-walker — the tw_defer counter says how many, this says WHO (one defer tree-walks its OWN body; eligible callees route back to the VM)",
    ),
    f(
        "BROOD_NO_TW_REENTRY",
        OPTOUT,
        "opt OUT of the tree-walker routing VM-eligible callees back to the engine (default ON since 2026-09-04, ADR-318: 60x on the viral defer shape; startup -7%). The A/B and bisect lever; set it if a routed shape is ever implicated (KI-88's dormant signature has its own watchdog)",
    ),
    f(
        "BROOD_SCHED_DBG",
        SCHED,
        "trace every enqueue, quantum start (body source prefix) and quantum outcome per pid, and arm the quantum-age ledger (KI-88's tool). The stranded-work watchdog — pool-wide starvation with queued work, KI-88's never-scheduled signature — reports without this flag",
    ),
    f(
        "BROOD_ROUTE_DBG",
        ATTRIBUTION,
        "name each closure the tree-walker routes to the VM (the router is default ON; BROOD_NO_TW_REENTRY disables it)",
    ),
    f(
        "BROOD_FAULT_QUANTUM_TAIL",
        SCHED,
        "=<n>: FAULT INJECTION — panic on the nth quantum's post-drive tail. Proves that path stays survivable (the worker lives, the process is retired, not silently dropped); nothing an ordinary program does provokes one",
    ),
    f(
        "BROOD_FAULT_STRANDED",
        SCHED,
        "FAULT INJECTION — over-count STEALABLE by one at pool start, so the pool believes a process is queued that no worker can find. Proves the stranded-work watchdog (KI-88's never-scheduled signature) actually fires; nothing an ordinary program does provokes one",
    ),
    // ---- JIT ----
    f(
        "BROOD_JIT_DUMP_IR",
        JIT,
        "per-lowered-arm opcode fingerprint + CLIF (and the scalar-register worker). Absence = did not lower",
    ),
    f(
        "BROOD_JIT_BAIL_TRACE",
        JIT,
        "name each arm the profitability gate refuses, and why — the complement of DUMP_IR",
    ),
    f(
        "BROOD_DEOPT_TRACE",
        JIT,
        "print each type-deopt's arm — finds an arm that lowers and then keeps falling back",
    ),
    f(
        "BROOD_DUMP_CODE",
        JIT,
        "native disassembly of arms whose name contains the value — one step past CLIF",
    ),
    f(
        "BROOD_JIT_VERIFY",
        JIT,
        "scan staged call args for a stale LOCAL handle (use-after-GC); works in plain release",
    ),
    f(
        "BROOD_JIT_VERIFY_FN",
        JIT,
        "log every JIT'd call to this function with each arg's type — for value-level corruption",
    ),
    f(
        "BROOD_NO_JIT",
        JIT,
        "alias for BROOD_TIER=1 — no native tiering, interpret on the VM; rules a JIT-only miscompile in or out",
    ),
    f(
        "BROOD_NO_JIT_COMPUTED",
        JIT,
        "bail only arms containing a computed jump — narrower bisect than NO_JIT",
    ),
    // ---- optimizer opt-outs ----
    f(
        "BROOD_NO_I64",
        OPTOUT,
        "disable the unboxed-scalar register worker (fib/pfib's path)",
    ),
    f(
        "BROOD_NO_INLINE",
        OPTOUT,
        "disable the JIT's recursive self-inliner (the two-stage deferred upgrade)",
    ),
    f(
        "BROOD_NO_LEAF_INLINE",
        OPTOUT,
        "disable leaf-callee splicing",
    ),
    f(
        "BROOD_NO_PARTIAL_LEAF",
        OPTOUT,
        "revert to all-or-nothing leaf splicing (no residual call beside the spliced leaves)",
    ),
    f(
        "BROOD_NO_FLOAT_GLOBAL",
        OPTOUT,
        "disable unboxing float-valued global reads (nbody's silent-interpretation bug)",
    ),
    f(
        "BROOD_LINMAP",
        OPTOUT,
        "`=0` disables the linear-map rewrite (in-place build of a provably-linear map accumulator)",
    ),
    f(
        "BROOD_NO_HOF",
        OPTOUT,
        "disable the higher-order-function fast path (per-element callback routing)",
    ),
    f(
        "BROOD_NO_SHARED_ARMS",
        OPTOUT,
        "every process compiles its own copy of every closure — costs spawn-live ~25% CPU",
    ),
    f(
        "BROOD_INLINE_DBG",
        OPTOUT,
        "trace which arms qualify for self/leaf inlining (pairs with the two NO_*INLINE flags)",
    ),
    f(
        "BROOD_NO_STDIMAGE",
        OPTOUT,
        "opt OUT of the stdlib startup image (ADR-281, default ON): `require` materialises a module's bindings from ~/.cache/brood instead of evaluating its source (json 6.5 -> 1.7 ms; a three-module script 46.5 -> 36.2 ms). Set it to A/B, to bisect a suspected materialise fault, or as the stopgap if one is found",
    ),
    f(
        "BROOD_STDIMAGE",
        ENGINE,
        "ask for the image explicitly. Redundant now that it is the default, and kept for one reason: an explicit request that goes UNMET prints a line, where the default path falls back to source in silence. Use it when measuring — `(stdimage/status)` says which of absent/stale/unreadable it was",
    ),
    f(
        "BROOD_IMAGE_TRACE",
        ENGINE,
        "name each module materialised from an image, and time the boot install — the only way to tell a module that came from the image from one that loaded from source anyway",
    ),
    f(
        "BROOD_NO_XCALL",
        OPTOUT,
        "opt OUT of the hot re-lowering / inline fast-frame call path (§7.5, default ON): a hot arm's body is recompiled on the deferred queue with the Brood-to-Brood call ceremony emitted inline (bintree -10.6%, -19% at 2x run length; short runs unaffected). The A/B and bisect lever",
    ),
    f(
        "BROOD_XCALL",
        JIT,
        "=1: additionally emit the inline call path in EVERY body's first compile (the experiment lever) — measured as a ~115M-instruction per-run compile constant, so not the default; the default arms it only in hot re-lowerings",
    ),
    f(
        "BROOD_XADMIT",
        JIT,
        "=1: admit profitability-gate-refused named defns at the HOT stage (deferred compile, inline call blob, frame cap). Measured NEGATIVE 2026-08-31 — nqueens +7.6% cycles, pipeline +7.6%: a call-dominated boxed arm is better interpreted even on the cheapest native call path we have. Kept as the one-env-var re-test for when the call convention changes (§7.5 increment 4)",
    ),
    f(
        "BROOD_MONO",
        OPTOUT,
        "opt IN to ability-dispatch monomorphization (off by default — it trades late binding)",
    ),
    f(
        "BROOD_MONO_DBG",
        OPTOUT,
        "trace each BROOD_MONO devirtualization",
    ),
    // ---- GC / memory ----
    f(
        "BROOD_GC_STRESS",
        GC,
        "collect at every safepoint — turns a rare GC race into a deterministic one",
    ),
    f(
        "BROOD_GC_VERIFY",
        GC,
        "walk the live graph each collection and name the root->cell path to a stale handle",
    ),
    f("BROOD_GC_TRACE", GC, "log each minor collection's stats (debug builds)"),
    f(
        "BROOD_GC_FLOOR",
        GC,
        "LOCAL collection threshold floor — sweep it to test whether a row is GC-frequency-bound",
    ),
    f(
        "BROOD_RT_GC_FLOOR",
        GC,
        "RUNTIME code-region reclamation threshold; a huge value effectively disables compaction",
    ),
    f("BROOD_MEM_LIMIT", GC, "hard memory cap for a run (bytes)"),
    f("BROOD_MEM_SOFT_LIMIT", GC, "soft memory cap for a run (bytes)"),
    external(
        "MIMALLOC_PURGE_DELAY",
        GC,
        "`=0` makes the allocator return pages, cutting RSS on churn for ~4% throughput",
    ),
    // ---- scheduler / messaging ----
    f(
        "BROOD_SPAWN_SPILL",
        SCHED,
        "backlog at which a spawn stops going to the spawner's own worker (default 1)",
    ),
    f(
        "BROOD_SPAWN_RR",
        SCHED,
        "force round-robin spawn placement — the A/B endpoint showing why the threshold exists",
    ),
    f(
        "BROOD_NO_HANDOFF",
        SCHED,
        "disable the scheduler's direct-handoff wake policy",
    ),
    f(
        "BROOD_NO_RECV_MARK",
        SCHED,
        "disable the receive-mark (O(1) ref-pinned receive instead of O(backlog))",
    ),
    f(
        "BROOD_NO_SHARE_FN",
        SCHED,
        "deep-copy an already-shared closure across a local send instead of passing it by handle",
    ),
    f(
        "BROOD_L1_BUDGET",
        SCHED,
        "copy-work units the L1 fast path may spend under the mailbox lock (0 = unlimited, the pre-KI-56 behaviour)",
    ),
    f(
        "BROOD_NO_MSGTAG",
        SCHED,
        "drop the L1 fast path's leading-keyword tag, defeating the selective-receive pre-filter",
    ),
    f(
        "BROOD_NO_DROP_WARN",
        SCHED,
        "silence the once-per-name warning that a message was dropped for an unregistered name",
    ),
    f(
        "BROOD_NO_CRASH_REPORT",
        SCHED,
        "opt OUT of the default crash reporter (ADR-305): `brood file`, `nest run`, a bundle and the REPL otherwise print one report per crash site for any process that exits abnormally",
    ),
    // ---- engine ----
    f(
        "BROOD_TIER",
        ENGINE,
        "the tier ceiling: 0 tree-walk, 1 bytecode VM, 2 native (ADR-222). Measured 57.4s/3.2s/0.12s",
    ),
    f(
        "BROOD_VM",
        ENGINE,
        "alias for BROOD_TIER=0 — `=0` runs the tree-walker (~10x slower; the differential reference)",
    ),
    f(
        "BROOD_NO_BOOT_CACHE",
        ENGINE,
        "skip the expanded-prelude boot cache — a cold boot is ~11x a warm one, so this is visible",
    ),
    f(
        "BROOD_PRELUDE_IMAGE",
        ENGINE,
        "opt IN to the prelude image (ADR-314) — a warm boot materialises the prelude's bindings instead of evaluating 544 forms (startup -11%), but OFF by default: with it on, a multi-file `nest check` loses a record's ability impl (KI-106)",
    ),
    f(
        "BROOD_NO_CHECK",
        ENGINE,
        "skip the implicit advisory type-check before a run (raw eval, e.g. when timing)",
    ),
    // ---- attribution (added when the catalogue was completed) ----
    f(
        "BROOD_STALL_MS",
        ATTRIBUTION,
        "`=<ms>`: report any GC pause, scheduler quantum or GUI paint at or over this — the lag tracer",
    ),
    f(
        "BROOD_COMPILE_TRACE",
        ATTRIBUTION,
        "time each JIT lowering (`[compile] <dur> arm=… inlined=…`); needs `make perf-brood`",
    ),
    f(
        "BROOD_EVAL_TRACE",
        ATTRIBUTION,
        "trace each form entering the TREE-WALKING evaluator (debug builds) — who left the VM",
    ),
    f(
        "BROOD_VM_TRACE",
        ATTRIBUTION,
        "trace each bytecode instruction as it executes (debug builds)",
    ),
    // ---- JIT (added when the catalogue was completed) ----
    f(
        "BROOD_JIT_CB_TRACE",
        JIT,
        "trace each `brood_rt_*` callback from native code back into Rust (debug builds)",
    ),
    f(
        "BROOD_DBG_CONST",
        JIT,
        "trace JIT constant-pool decisions — for diagnosing a wrong-constant miscompile",
    ),
    f(
        "BROOD_MKCLO",
        JIT,
        "opt IN to admitting `MakeClosure` to the JIT subset; default OFF (`docs/compute-frontier.md`)",
    ),
    f(
        "BROOD_MAPGET",
        JIT,
        "opt IN to lowering `(get m k)` to a native map probe; default OFF — a miss-heavy loop can deopt to BAILED",
    ),
    // ---- optimizer opt-outs (added when the catalogue was completed) ----
    f(
        "BROOD_NO_HOF_JIT",
        OPTOUT,
        "opt out of the higher-order call's native fast-frame (narrower than BROOD_NO_HOF)",
    ),
    f(
        "BROOD_NO_JIT_ICALL",
        OPTOUT,
        "opt out of the in-IR call-site fast-link; every call takes `brood_rt_call_slow` (fib ~20%)",
    ),
    f(
        "BROOD_NO_DEOPT_RESUME",
        OPTOUT,
        "chicken switch: drop deopt checkpoints and re-run a deopted arm from ip 0 instead",
    ),
    f(
        "BROOD_INLINE_DEPTH",
        OPTOUT,
        "`=<n>`: recursion levels the self-inliner splices (default 2) — the A/B knob",
    ),
    f(
        "BROOD_INLINE_MAXBODY",
        OPTOUT,
        "`=<n>`: body-size ceiling for self-inline expansion past the first pass",
    ),
    // ---- GC / memory (added when the catalogue was completed) ----
    f(
        "BROOD_GC_MAJOR",
        GC,
        "`=<count>`: live-object floor before a MAJOR collection (default 256K; K/M suffixes ok)",
    ),
    f(
        "BROOD_GC_TENURE",
        GC,
        "`=<count>`: nursery pressure at which a minor TENURES survivors rather than flipping (default 16K)",
    ),
    f(
        "BROOD_GC_GROWTH",
        GC,
        "`=<factor>`: minor-threshold growth per collection, 1.05–8.0 (default 2.0)",
    ),
    f(
        "BROOD_MAJOR_GROWTH",
        GC,
        "`=<factor>`: old-gen growth allowed before the next major, >=2 (default 4)",
    ),
    f(
        "BROOD_GC_TENURE_RESERVE",
        GC,
        "restore the peak-sized nursery reservation after a tenure — the A/B lever for that RSS fix",
    ),
    f(
        "BROOD_STACK_BUDGET",
        GC,
        "`=<bytes>`: the non-tail-recursion stack guard's budget",
    ),
    f(
        "BROOD_TRACE_GCBLOCK",
        GC,
        "trace GC-block depth (debug builds)",
    ),
    // ---- scheduler / messaging / distribution (added when the catalogue was completed) ----
    f(
        "BROOD_J",
        SCHED,
        "`=<n>`: worker threads in the scheduler pool (default = available parallelism)",
    ),
    f(
        "BROOD_REDUCTIONS",
        SCHED,
        "`=<n>`: eval iterations a process runs before it must yield (default 2000; huge ≈ no preemption)",
    ),
    f(
        "BROOD_STEAL_GRACE_NS",
        SCHED,
        "`=<ns>`: a spawner's first refusal on its own child before a peer may steal it (default 5000; 0 off)",
    ),
    f(
        "BROOD_NO_STEAL_WAKE",
        SCHED,
        "opt out of the spawn-time peer wake; idle workers wait for their own steal re-probe",
    ),
    f(
        "BROOD_NO_SHARE_FN_MSG",
        SCHED,
        "opt out of handing a shared closure by handle on the SERIALISED send (BROOD_NO_SHARE_FN's sibling)",
    ),
    f(
        "BROOD_NO_MESH",
        SCHED,
        "point-to-point node links only — no automatic mesh to every node a peer already knows",
    ),
    // ---- diagnostics and checking ----
    f(
        "BROOD_COVERAGE",
        DIAG,
        "arm line-coverage instrumentation. Set it before the first Interp — the prelude compiles then — and note it stands the prelude/std images aside, since a materialised binding is never compiled",
    ),
    f(
        "BROOD_CONTRACTS",
        DIAG,
        "turn every `sig` into a runtime checking shim — the static checker's runtime counterpart",
    ),
    f(
        "BROOD_CHECK_STRICT",
        DIAG,
        "run the advisory checker in --strict mode; the check cache keys its manifest on this",
    ),
    f(
        "BROOD_NO_CHECK_CACHE",
        DIAG,
        "bypass `nest check`'s incremental result cache — recheck everything from scratch",
    ),
    f(
        "BROOD_CHECK_CACHE_MAX",
        DIAG,
        "`=<n>`: project file count above which the check cache stands aside (default 50000)",
    ),
    f(
        "BROOD_NO_RELOAD_DIAG",
        DIAG,
        "silence the hot-reload `def` diagnostics (arity changed / macro redefined)",
    ),
    f(
        "BROOD_NO_SHADOW_WARN",
        DIAG,
        "silence the warning that a `(:use …)` import shadows a prelude/root global",
    ),
    f(
        "BROOD_TEST_NO_SCOPE",
        DIAG,
        "revert `nest test` to load-all-then-run-all instead of the per-file `%isolate` scope",
    ),
    // ---- host environment ----
    f(
        "BROOD_GUI_HEADLESS",
        HOST,
        "run the GUI/display layer with no real window, and no audio — for a headless CI box",
    ),
    f(
        "BROOD_GUI_GPU",
        HOST,
        "select the experimental OpenGL render backend at runtime (build with --with-gui-gpu)",
    ),
    f(
        "BROOD_GUI_DAMAGE",
        HOST,
        "`=0`: blit the whole buffer each frame instead of the damage region (the safe fallback)",
    ),
    f(
        "BROOD_AUDIO",
        HOST,
        "`=0`: disable `audio-beep` (also off with no device present, or under BROOD_GUI_HEADLESS)",
    ),
    f(
        "BROOD_COOKIE",
        HOST,
        "the distribution auth cookie, ahead of `~/.brood_cookie` — configuration, not a diagnostic",
    ),
    f(
        "BROOD_HISTORY",
        HOST,
        "`=<path>`: where the REPL stores its history",
    ),
    f(
        "BROOD_RC",
        HOST,
        "`=<path>`: the REPL's user startup file (default `$HOME/.broodrc.blsp`)",
    ),
];

/// `BROOD_*` names that are **not** runtime environment flags, so the completeness test
/// below must not demand a catalogue line for them. All three are BUILD-time: the first two
/// are stamped by `crates/lisp/build.rs` and read back with `env!`, and the third is read by
/// `crates/nest/build.rs` to pick the `brood` binary a `nest` embeds (ADR-038). Setting any
/// of them for a *run* does nothing.
#[cfg(test)]
const NON_FLAGS: &[&str] = &["BROOD_GIT_SHA", "BROOD_STDLIB_HASH", "BROOD_EMBED_RUNTIME"];

/// Print the catalogue, grouped, for `brood --debug-flags`.
pub fn print_catalogue() {
    println!(
        "BROOD_* environment flags — all {} the runtime reads, triage groups first.",
        FLAGS.iter().filter(|f| f.ours).count()
    );
    println!("Set any to 1 unless a value is described. CLAUDE.md's table is the long form:");
    println!("it carries the measurement history behind each default.");
    let width = FLAGS.iter().map(|f| f.name.len()).max().unwrap_or(0);
    // Drive the grouping from GROUP_ORDER rather than from adjacency in the array: an entry
    // added next to its siblings used to split its group in two and print the heading twice.
    for group in GROUP_ORDER {
        println!("\n{group}");
        for flag in FLAGS.iter().filter(|f| f.group == *group) {
            // Mark a dependency's flag so a reader does not go looking for it in our source.
            let tag = if flag.ours { "" } else { " [not brood's]" };
            println!(
                "  {:<width$}  {}{tag}",
                flag.name,
                flag.effect,
                width = width
            );
        }
    }
    println!("\nStart here:");
    println!("  make perf-brood                      # counters compiled in");
    println!("  BROOD_PERF_STATS=1 brood prog.blsp   # dump them at exit");
    println!("  (perf/summary)                       # in-image, interpreted (auto-loads perf)");
    println!("  scripts/bench-ratio.sh               # TIMES (counter-free — never this build)");
}

#[cfg(test)]
mod tests {
    use super::FLAGS;

    /// Every catalogued name must still appear in the source tree. This is the drift guard:
    /// renaming or deleting a flag without touching this file leaves a line that tells a
    /// reader to set something the runtime ignores — a silent wrong answer, not an error.
    ///
    /// Only this direction is asserted. [`FLAGS`] is a curated performance subset, so a flag
    /// present in the source and absent here is a deliberate omission, not a failure.
    #[test]
    fn every_catalogued_flag_exists_in_the_source() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let mut haystack = String::new();
        let mut stack = vec![root.join("crates"), root.join("std")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Skip build output — it contains generated copies and is huge.
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(path);
                } else if path
                    .extension()
                    .is_some_and(|e| e == "rs" || e == "blsp" || e == "mk")
                {
                    // Skip THIS file. It lives under `crates/`, so including it makes every
                    // catalogued name find itself and the assertion vacuously true — verified
                    // by sabotage: renaming a flag here passed until this line existed.
                    if path.file_name().is_some_and(|n| n == "debug_flags.rs") {
                        continue;
                    }
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        haystack.push_str(&text);
                    }
                }
            }
        }
        // `Makefile` has no extension but names flags in its recipes.
        if let Ok(text) = std::fs::read_to_string(root.join("Makefile")) {
            haystack.push_str(&text);
        }
        // If the source tree isn't there (a vendored/packaged build), skip rather than report
        // every flag as missing — a scan that found nothing proves nothing, and failing here
        // would blame the catalogue for an absent checkout.
        if haystack.len() < 100_000 {
            eprintln!(
                "skipping: only {} bytes of source found under {} — not a full checkout",
                haystack.len(),
                root.display()
            );
            return;
        }
        let missing: Vec<&str> = FLAGS
            .iter()
            .filter(|f| f.ours) // a dependency's flag is never in our source, by definition
            .map(|f| f.name)
            .filter(|name| !haystack.contains(*name))
            .collect();
        assert!(
            missing.is_empty(),
            "catalogued in debug_flags.rs but absent from the source (renamed or deleted?): {missing:?}"
        );
    }

    /// Every flag's group must be in `GROUP_ORDER`, because the printer iterates that list:
    /// a group missing from it makes its flags vanish from `--debug-flags` silently.
    #[test]
    fn every_group_is_in_the_print_order() {
        let mut orphans: Vec<&str> = FLAGS
            .iter()
            .map(|f| f.group)
            .filter(|g| !super::GROUP_ORDER.contains(g))
            .collect();
        orphans.sort_unstable();
        orphans.dedup();
        assert!(
            orphans.is_empty(),
            "group(s) not in GROUP_ORDER, so their flags never print: {orphans:?}"
        );
    }

    /// The other direction: every `BROOD_*` the RUNTIME reads must be catalogued here.
    ///
    /// The catalogue was a curated performance subset for its first months, and the gap
    /// grew to 43 of 101 flags — among them the worker count, the reduction budget and
    /// every GC tuning knob. A flag nothing documents is a flag nobody reaches for, so
    /// `--debug-flags` is now the complete list and this test keeps it that way.
    ///
    /// Scope is deliberately `crates/*/src` + `std/` — what the runtime itself reads.
    /// Test fixtures name env vars that are not flags (`BROOD_SURELY_MISSING_VAR_XYZ`),
    /// and cataloguing those would describe the suite rather than the runtime.
    #[test]
    fn every_runtime_flag_is_catalogued() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let mut found: Vec<(String, String)> = Vec::new();
        let mut scanned = 0usize;
        let mut stack = vec![root.join("crates"), root.join("std")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if path.is_dir() {
                    // Build output, and the test/bench trees whose fixtures are not flags.
                    if matches!(name.as_str(), "target" | "tests" | "benches" | "examples") {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }
                if name == "debug_flags.rs"
                    || !path.extension().is_some_and(|e| e == "rs" || e == "blsp")
                {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                scanned += text.len();
                // A quoted literal, not a bare mention: a doc comment that recalls a
                // DELETED flag ("replaces the old BROOD_JIT_INLINE") must not be demanded.
                for (i, _) in text.match_indices("\"BROOD_") {
                    let rest = &text[i + 1..];
                    let Some(end) = rest.find('"') else { continue };
                    let flag = &rest[..end];
                    if flag
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                    {
                        found.push((flag.to_string(), name.clone()));
                    }
                }
            }
        }
        // A scan that found nothing proves nothing — see the sibling test.
        if scanned < 100_000 {
            eprintln!(
                "skipping: only {scanned} bytes scanned under {} — not a full checkout",
                root.display()
            );
            return;
        }
        let mut missing: Vec<String> = found
            .iter()
            .filter(|(flag, _)| {
                !super::NON_FLAGS.contains(&flag.as_str()) && !FLAGS.iter().any(|f| f.name == flag)
            })
            .map(|(flag, file)| format!("{flag} (read in {file})"))
            .collect();
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "read by the runtime but absent from the catalogue in debug_flags.rs \
             (add an `f(NAME, GROUP, \"…\")` entry, or NON_FLAGS if it is not an env flag): {missing:#?}"
        );
    }

    /// No duplicate entries — a flag listed twice prints twice and suggests two meanings.
    #[test]
    fn catalogue_has_no_duplicates() {
        let mut names: Vec<&str> = FLAGS.iter().map(|f| f.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate flag in the catalogue");
    }
}
