//! The `BROOD_*` diagnostic-flag catalogue — what `brood --debug-flags` prints.
//!
//! The runtime has ~75 environment flags, and they *are* the performance toolkit: which
//! one to reach for was tribal knowledge, discoverable only by reading `CLAUDE.md`'s
//! table, which is not shipped in the binary. This makes the answer available from the
//! binary itself (`docs/backend-seams.md` §5).
//!
//! **A curated subset, on purpose.** These are the flags for *performance triage and
//! A/B* — attribution, the JIT, the GC, the optimizer opt-out levers, the scheduler
//! knobs. Editor/GUI/audio/TUI flags are omitted, as are the ones whose only use is
//! inside a specific test. `CLAUDE.md`'s table remains the long form: it carries the
//! measurement history behind each default, which is the part you want when deciding
//! whether to flip one, and is far too long to print.
//!
//! [`FLAGS`] is checked against the source tree by a test below, so a flag that gets
//! renamed or deleted cannot leave a stale line here. The reverse direction — a new flag
//! that never gets added — is deliberately *not* asserted: this is a subset, so "missing"
//! is not an error, and a test that forced every new flag in here would push editor and
//! test-only flags into a performance list.

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
const OPTOUT: &str = "Optimizer opt-outs — the A/B and bisect levers (all default ON)";
const GC: &str = "GC and memory";
const SCHED: &str = "Scheduler and messaging";
const ENGINE: &str = "Engine selection";

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
        "BROOD_TW_REENTRY",
        OPTOUT,
        "opt IN (=1) to the tree-walker routing VM-eligible callees back to the engine (60x on the viral defer shape; startup -7%). Off by default until KI-88 (a routed spawn burst can strand one process unscheduled) is fixed",
    ),
    f(
        "BROOD_SCHED_DBG",
        SCHED,
        "trace every enqueue, quantum start (body source prefix) and quantum outcome per pid — the per-pid lifecycle counters cannot show (KI-88's tool)",
    ),
    f(
        "BROOD_ROUTE_DBG",
        ATTRIBUTION,
        "name each closure the tree-walker routes to the VM (pairs with BROOD_TW_REENTRY)",
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
        "name each module materialised from an image, and time the boot install — the only way          to tell a module that came from the image from one that loaded from source anyway",
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
        "BROOD_NO_CHECK",
        ENGINE,
        "skip the implicit advisory type-check before a run (raw eval, e.g. when timing)",
    ),
];

/// Print the catalogue, grouped, for `brood --debug-flags`.
pub fn print_catalogue() {
    println!("BROOD_* diagnostic flags — the performance-triage subset.");
    println!("Set any to 1 unless a value is described. CLAUDE.md's table is the long form:");
    println!("it carries the measurement history behind each default.");
    let width = FLAGS.iter().map(|f| f.name.len()).max().unwrap_or(0);
    let mut group = "";
    for flag in FLAGS {
        if flag.group != group {
            group = flag.group;
            println!("\n{group}");
        }
        // Mark a dependency's flag so a reader does not go looking for it in brood's source.
        let tag = if flag.ours { "" } else { " [not brood's]" };
        println!(
            "  {:<width$}  {}{tag}",
            flag.name,
            flag.effect,
            width = width
        );
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
