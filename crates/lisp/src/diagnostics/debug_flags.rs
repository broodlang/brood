//! The `BROOD_*` diagnostic-flag catalogue — what `brood --debug-flags` prints.
//!
//! The runtime's environment flags *are* its diagnostic and performance toolkit, and this
//! catalogue is their one reference, shipped in the binary (`docs/backend-seams.md` §5).
//! A flag's description belongs here, not in a doc that drifts from it; the measurement
//! history behind a default lives in its ADR or devlog entry.
//!
//! **The complete list, in triage order.** A flag nothing documents is a flag nobody
//! reaches for, so the answer is ordering, not omission: the triage groups print first
//! ([`GROUP_ORDER`]) and the environment ones last.
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
        "BROOD_TRACE_GLOBAL",
        ATTRIBUTION,
        "=<name>: narrate every root write to that ONE global (define / staged swap / live swap, with pid + isolate scope) and its value at every globals restore — the tool for a registry that is full in one test file and empty in the next (KI-141)",
    ),
    f(
        "BROOD_FEATURES_AUDIT",
        ATTRIBUTION,
        "report (with a backtrace) the write that leaves *features* listing a module whose bindings are gone — checked after every module publish, %isolate restore and live *features* write; the cause of `[refer] … imported NOTHING` (KI-193). Cheap enough to leave armed in a full suite run",
    ),
    f(
        "BROOD_REG_TRACE",
        ATTRIBUTION,
        "trace *record-ids* registry writes (with the writer's ancestry chain AND the isolate scope it was spawned under) and every globals restore — the KI-89 orphan-attribution tool; trace LEAN, heavy tracing suppresses the race",
    ),
    f(
        "BROOD_SCOPE_DBG",
        ATTRIBUTION,
        "name every process still live when %isolate rolls the globals back — an isolate is sound only while nothing else mutates globals, and this is what says when that is violated",
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
    f(
        "BROOD_FAULT_CLAIM_RACE",
        SCHED,
        "=<module>: FAULT INJECTION — the runtime's first first-use miss into <module> in a frame that can park pauses 500 ms between asking whether another process holds its module's load and taking the claim itself, so a test can take the claim inside that window. Proves a waiter that loses the claim race parks rather than blocking its worker; nothing an ordinary program does provokes one",
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
        "name each arm the profitability gate refuses, and why — the complement of DUMP_IR; `[jit-relower]` lines say when deopt feedback re-tiers an arm (a polymorphic param boxed, or float context for floats read out of vectors, ADR-378), and a `deopt-thrash-latched` line carries its `last-deopt` reason",
    ),
    f(
        "BROOD_DEOPT_TRACE",
        JIT,
        "print each type-deopt's arm and the guard that fired (`reason=<name>`, a `DeoptReason` — one per guard site) — finds an arm that lowers and then keeps falling back; needs `make perf-brood`",
    ),
    f(
        "BROOD_DUMP_CODE",
        JIT,
        "native disassembly of arms whose name contains the value — one step past CLIF",
    ),
    f(
        "BROOD_JIT_VERIFY",
        JIT,
        "scan staged call args for a stale LOCAL handle (use-after-GC); works in plain release. In a debug build it also records call-site positions, so stale-value reports name file:line",
    ),
    f(
        "BROOD_JIT_VERIFY_FN",
        JIT,
        "log every JIT'd call to this function with each arg's type — for value-level corruption",
    ),
    f(
        "BROOD_NO_JIT_COMPUTED",
        JIT,
        "bail only arms containing a computed jump — narrower bisect than BROOD_TIER=1",
    ),
    // ---- optimizer opt-outs ----
    f(
        "BROOD_NO_I64",
        OPTOUT,
        "disable the unboxed-scalar register worker (fib/pfib's path)",
    ),
    f(
        "BROOD_NO_INT_SLOT_CACHE",
        OPTOUT,
        "disable the JIT's per-slot i64 cache (re-tag-check every read of a just-stored int slot)",
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
        "BROOD_INLINE_DBG",
        OPTOUT,
        "trace which arms qualify for self/leaf inlining",
    ),
    f(
        "BROOD_NO_LAZY_LOAD",
        OPTOUT,
        "opt OUT of lazy module loading (ADR-335, default ON): a qualified FUNCTION reference loads its module on first use (at the global-lookup miss) instead of when the referencing file loads — `nest complete` stopped materialising 62 modules to print one name. Set it to A/B, to bisect, or as the stopgap if a first-use load is ever implicated. The checker and `nest run --check-boot` load eagerly regardless",
    ),
    f(
        "BROOD_NO_STDIMAGE",
        OPTOUT,
        "opt OUT of the stdlib startup image (ADR-281, default ON): `require` materialises a module's bindings from ~/.cache/brood instead of evaluating its source (json 6.5 -> 1.7 ms; a three-module script 46.5 -> 36.2 ms). Set it to A/B, to bisect a suspected materialise fault, or as the stopgap if one is found",
    ),
    f(
        "BROOD_NO_BUNDLE_IMAGE",
        OPTOUT,
        "opt OUT of a released bundle's own startup image (ADR-484, default ON): the bundle loads every module from source instead. The lever that compares the two on one binary, as BROOD_NO_STDIMAGE is for the stdlib's",
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
        "BROOD_DEVIRT_DBG",
        OPTOUT,
        "trace each ability call the compiler devirtualizes (ADR-182: a literal or record-constructor first arg skips `%identity-of`)",
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
        "before each collection, walk the live LOCAL graph and panic on a stale (old-epoch) handle, naming it and the cell that holds it — catches a stale handle STORED rather than dereferenced; works in plain release",
    ),
    f(
        "BROOD_GC_TRACE",
        GC,
        "log each minor collection's stats (debug builds)",
    ),
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
    f(
        "BROOD_MEM_LIMIT",
        GC,
        "hard memory cap for a run (bytes; default: 3/4 of a container's memory cap, else none; 0 = none)",
    ),
    f(
        "BROOD_MEM_SOFT_LIMIT",
        GC,
        "soft memory cap for a run (bytes)",
    ),
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
        "BROOD_NO_RECV_MARK",
        SCHED,
        "disable the receive-mark (O(1) ref-pinned receive instead of O(backlog))",
    ),
    f(
        "BROOD_NO_SHARE_FN",
        SCHED,
        "deep-copy an already-shared closure across a local send (L1 and serialised) instead of passing it by handle",
    ),
    f(
        "BROOD_L1_BUDGET",
        SCHED,
        "copy-work units (one heap node each, default 4096) the L1 send copy and the selective-receive rebuild may spend under the mailbox lock; past it a message takes the wire path (0 = unlimited, the pre-KI-56 behaviour)",
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
        "BROOD_TIER0_AUDIT",
        ENGINE,
        "=<file>: under BROOD_TIER=0, append each distinct call path that starts bytecode anyway, once per OS process (ADR-416's escape audit; `sort | uniq -c` the file)",
    ),
    f(
        "BROOD_NO_PRELUDE_IMAGE",
        OPTOUT,
        "opt OUT of the prelude image (ADR-314, default ON since 2026-09-04): neither read nor written, so every boot is the full source boot (a cold boot is ~11x a warm one). The only boot-artifact knob since the ADR-138 text cache was deleted (2026-09-12). KI-105/KI-106 were the image's two same-day reverts, both fixed and gated",
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
        "BROOD_TRACE_LOWER",
        ATTRIBUTION,
        "time each JIT lowering (`[lower] <dur> arm=… inlined=…`); needs `make perf-brood`",
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
        "BROOD_NO_MAPGET",
        OPTOUT,
        "opt OUT of lowering `(get m k)` — and, since ADR-368, `(get m k default)` — to an inline map probe (ADR-296; default ON since 2026-09-17)",
    ),
    f(
        "BROOD_NO_IMAGE_SIGS",
        OPTOUT,
        "opt OUT of reading function signatures from the stdlib image (ADR-370; default ON since 2026-09-18): the checker then infers loaded bodies and materialises every module they name, as before — the A/B, bisect and differential lever",
    ),
    f(
        "BROOD_NO_MAPASSOC",
        OPTOUT,
        "opt OUT of lowering `(assoc m k v)` on a map to an inline path-copy (ADR-368; default ON since 2026-09-17) — its own lever because it ALLOCATES from native code where the reads do not",
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
        "turn every `sig` into a runtime checking shim — the static checker's runtime counterpart. The `nest run`/`nest test` default (ADR-381); `=0` opts a run out, `=all` also enforces the prelude's own sigs; a released bundle never arms",
    ),
    f(
        "BROOD_NO_CONTRACT_BOUNDARY",
        OPTOUT,
        "opt OUT of the contract MODULE BOUNDARY (ADR-383): a module's calls to its own contracted functions go through the shim too, instead of the uncontracted binding",
    ),
    f(
        "BROOD_CHECK_STRICT",
        DIAG,
        "run the advisory checker in --strict mode; the check cache keys its manifest on this",
    ),
    f(
        "BROOD_CHECK_SEQUENTIAL",
        OPTOUT,
        "opt OUT of fanning `nest check`'s per-file walk across the worker pool: check every file in this process, whatever the project's size (the A/B and bisect lever; the control arm of the parallel/sequential warning differential)",
    ),
    f(
        "BROOD_LOAD_SEQUENTIAL",
        OPTOUT,
        "opt OUT of the parallel cold load (ADR-426): a cold project build loads its modules in walk order in this process instead of prefetching them across a pool of processes (the A/B and bisect lever; the control arm of `crates/nest/tests/parallel_load.rs`)",
    ),
    f(
        "BROOD_NO_CHECK_CACHE",
        DIAG,
        "bypass `nest check`'s incremental result cache AND `brood file`'s run pre-flight verdict cache — recheck everything from scratch",
    ),
    f(
        "BROOD_DERIVE_DBG",
        DIAG,
        "trace the checker's caller-derived parameter fixpoint (ADR-341): each round's moved candidates, an OPAQUE macro, NO FIXPOINT, and per file where the time went",
    ),
    f(
        "BROOD_NO_DERIVE_CACHE",
        DIAG,
        "opt OUT of memoising the checker's site walk per form across the derivation's rounds (the A/B and bisect lever; identical verdicts by construction, gated by the differential)",
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
        "BROOD_TEST_TRACE",
        DIAG,
        "arm the test runner's --trace from the environment (one line per result as it lands) — for the nextest wrapper, whose progress dots name nothing when the run is killed at its cap (KI-120)",
    ),
    // ---- host environment ----
    f(
        "BROOD_DIST_VERSION",
        HOST,
        "the `major.minor` this node says in the distribution handshake instead of its own (ADR-699) — the node of another minor a test of the refusal needs",
    ),
    f(
        "BROOD_GUI_MAIN_THREAD",
        HOST,
        "host the GUI event loop on the process MAIN thread instead of a dedicated one — forced on macOS/Windows, where AppKit allows nowhere else; the lever exists so Linux can exercise that path (KI-125)",
    ),
    f(
        "BROOD_GUI_HEADLESS",
        HOST,
        "run the GUI/display layer with no real window, and no audio — for a headless CI box",
    ),
    f(
        "BROOD_TERM_GRAPHICS",
        HOST,
        "how the terminal display draws an `[:image …]` op: `kitty` (the kitty graphics protocol, pixels) or `cells` (half blocks), without asking the terminal; unset or `auto` asks it at `term-enter` (ADR-695)",
    ),
    f(
        "BROOD_TEST_RUN",
        HOST,
        "`=1`: this process is part of a test run — set by `nest test` before the interpreter starts, inherited by what a test spawns; `test/running?` reads it (ADR-602)",
    ),
    f(
        "BROOD_TEST_SCRATCH",
        HOST,
        "the test run's private scratch home, set by `nest test` (or `BROOD_TEST_ARM`); `test/scratch-dir` reads it, and XDG_CONFIG_HOME / XDG_STATE_HOME / XDG_DATA_HOME / BROOD_HISTORY point inside it (ADR-602, ADR-608)",
    ),
    f(
        "BROOD_TEST_HOME",
        HOST,
        "`=keep`: `nest test` keeps the real XDG homes instead of giving the run a scratch home — the same as `--real-home` (ADR-602)",
    ),
    f(
        "BROOD_TEST_ARM",
        HOST,
        "`=1`: make this process a test run with a scratch home of its own, as `nest test` does — any `nest` or `brood` entry point arms before the interpreter starts and removes the request from its environment, so what it starts joins its run; what a test runner that is not `nest test` sets on its child (ADR-608)",
    ),
    f(
        "BROOD_USER_CONFIG_HOME",
        HOST,
        "the user's real config home, kept by `nest test` while XDG_CONFIG_HOME points at the scratch home, so nest's own config (registry, token) still reads from it; a run inside a run keeps the outer run's value (ADR-602, ADR-608)",
    ),
    f(
        "BROOD_GRAMMAR_VET",
        HOST,
        "`=0`: load a tree-sitter grammar library that has no verdict without vetting it in a child first — an embedder's choice, or a test that wants one loaded unvetted; a quarantined library is still refused (ADR-660)",
    ),
    f(
        "BROOD_GRAMMAR_VET_CHILD",
        HOST,
        "set by the runtime on the child it starts to vet a grammar library: the binary's first line (`cli_support::grammar_vetting_entry`) vets and exits instead of running — not for hand use (ADR-660)",
    ),
    f(
        "BROOD_GRAMMAR_VET_LIBRARY",
        HOST,
        "the grammar library a vetting child loads, set beside BROOD_GRAMMAR_VET_CHILD by the runtime that starts it (ADR-660)",
    ),
    f(
        "BROOD_GRAMMAR_VET_LANG",
        HOST,
        "the language a vetting child loads its library as (its `tree_sitter_<lang>` entry point), set by the runtime that starts it (ADR-660)",
    ),
    f(
        "BROOD_GRAMMAR_VET_SHA256",
        HOST,
        "the SHA-256 the parent hashed a vetting child's library to; the child refuses to vet bytes that differ (ADR-660)",
    ),
    f(
        "BROOD_GRAMMAR_VET_PARSE_MS",
        HOST,
        "a vetting child's budget per parse, a quarter of its watchdog: a parse past it is slow, not a fault, and is cut short (ADR-660)",
    ),
    f(
        "BROOD_GUI_GPU",
        HOST,
        "`=0`: paint on the CPU (softbuffer) in a `--with-gui-gpu` build instead of the GPU (wgpu) render target it selects by default — the A/B lever, and the escape hatch for a misbehaving GPU driver; the pixel-space `:quad` / `:sprite` ops draw on both (in software here)",
    ),
    f(
        "BROOD_GUI_DAMAGE",
        HOST,
        "`=0`: re-rasterise and blit the whole buffer each frame instead of the changed rows (the safe fallback)",
    ),
    f(
        "BROOD_GUI_BLIT",
        HOST,
        "`=0`: rasterise every dirty strip instead of copying one that is a translation of old pixels (ADR-343) — the escape hatch if a blitted strip ever differs from a drawn one",
    ),
    f(
        "BROOD_GUI_COLUMNS",
        HOST,
        "`=0`: re-rasterise a changed cell row across the whole window instead of only the columns its changed ops can paint (ADR-482) — the escape hatch if a leaf ever paints outside the columns it is given",
    ),
    f(
        "BROOD_GUI_TRACE",
        HOST,
        "`=1`: print every GUI paint's breakdown (rows repainted, clusters, timing) — BROOD_STALL_MS at threshold 0 without the per-native-call flood",
    ),
    f(
        "BROOD_UI_TRACE",
        HOST,
        "`=1`: print every `ui-run` turn's Brood-side breakdown (view, draw, update in ms) — the other half of BROOD_GUI_TRACE",
    ),
    f(
        "BROOD_GUI_NO_DUAL_SOURCE",
        HOST,
        "`=1`: the GPU painter declines dual-source blending and draws subpixel text in three passes, one per colour channel — the path an adapter without the feature takes (`lcd=` in BROOD_GUI_TRACE says which)",
    ),
    f(
        "BROOD_GUI_DUMP",
        HOST,
        "`=<path.ppm>`: write the painted frame as a PPM after every paint (the CPU canvas, or a readback of the GPU frame under BROOD_GUI_GPU) — look at the raster (text AA, hairlines, sprites) from a script",
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
/// below must not demand a catalogue line for them. All four are BUILD-time: `BROOD_GIT_SHA`,
/// `BROOD_STDLIB_HASH` and `BROOD_TARGET` (the triple `system/target` reports, ADR-714) are
/// stamped by `crates/lisp/build.rs` and read back with `env!`, and `BROOD_EMBED_RUNTIME` is
/// read by `crates/nest/build.rs` to pick the `brood` binary a `nest` embeds (ADR-038).
/// Setting any of them for a *run* does nothing.
#[cfg(test)]
const NON_FLAGS: &[&str] = &[
    "BROOD_GIT_SHA",
    "BROOD_STDLIB_HASH",
    "BROOD_TARGET",
    "BROOD_EMBED_RUNTIME",
];

/// `BROOD_JIT_BAIL_TRACE` is set — read once. The one reader: the trace is asked at twenty
/// sites across the tiering code.
pub fn jit_bail_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_JIT_BAIL_TRACE")
}

/// `BROOD_TRACE_COMPILE` is set — read once. Asked on every closure compile and at each
/// stale-bindings mark (`[compile] …` lines).
pub fn trace_compile() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_TRACE_COMPILE")
}

/// `BROOD_TRACE_LOWER` is set — read once. Times each JIT lowering (`[lower] …` lines).
pub fn trace_lower() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_TRACE_LOWER")
}

// ---- The evaluator's and JIT's toggles, each read once and cached. A lever that is ON by
// default answers `true` unless its `BROOD_NO_*` opt-out is set. One reader per flag, so the
// catalogue above and the runtime cannot disagree about which flags exist.

/// `name` is set (to anything) — read once into `cell`. The one spelling of a cached
/// presence flag; the subsystems that keep their own reader (GC, scheduler, mailbox,
/// image, contracts) call it too, so "set" means the same thing for every flag.
pub(crate) fn is_set(cell: &std::sync::OnceLock<bool>, name: &str) -> bool {
    *cell.get_or_init(|| std::env::var_os(name).is_some())
}

/// `name` is set to something other than empty or `0` — read once into `cell`.
pub(crate) fn is_armed(cell: &std::sync::OnceLock<bool>, name: &str) -> bool {
    *cell.get_or_init(|| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// `BROOD_DEFER_DBG`: name each closure call that defers to the tree-walker.
pub fn defer_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_DEFER_DBG")
}

/// `BROOD_ROUTE_DBG`: name each closure the tree-walker routes back to the VM.
pub fn route_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_ROUTE_DBG")
}

/// `BROOD_INLINE_DBG`: trace which arms qualify for self/leaf inlining.
pub fn inline_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_INLINE_DBG")
}

/// `BROOD_JIT_CB_TRACE`: trace each runtime callback from native code (debug builds).
pub fn jit_callback_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_armed(&ON, "BROOD_JIT_CB_TRACE")
}

/// `BROOD_DBG_CONST`: trace JIT constant-pool decisions.
pub fn debug_constants() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_DBG_CONST")
}

/// `BROOD_DEOPT_TRACE`: print each JIT type-deopt's arm and guard (`perf-stats` builds).
pub fn deopt_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_DEOPT_TRACE")
}

/// `BROOD_JIT_DUMP_IR`: dump each lowered arm's bytecode fingerprint and CLIF.
pub fn jit_dump_ir() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_armed(&ON, "BROOD_JIT_DUMP_IR")
}

/// `BROOD_JIT_VERIFY`: scan every JIT'd call's staged args for a stale LOCAL handle.
pub fn jit_verify() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_JIT_VERIFY")
}

/// `BROOD_JIT_VERIFY_FN=<fn>`: the function whose JIT'd calls log each staged arg's type.
pub fn jit_verify_function() -> Option<&'static str> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| std::env::var("BROOD_JIT_VERIFY_FN").ok())
        .as_deref()
}

/// `BROOD_TIER0_AUDIT=<file>`: the file a tier-0 run appends each path that starts bytecode to.
pub fn tier0_audit_path() -> Option<&'static str> {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        std::env::var("BROOD_TIER0_AUDIT")
            .ok()
            .filter(|p| !p.is_empty())
    })
    .as_deref()
}

/// `BROOD_NO_JIT_COMPUTED`: keep arms with a computed-head non-tail call on the VM.
pub fn no_jit_computed() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_NO_JIT_COMPUTED")
}

/// The tree-walker routes VM-eligible callees back to the VM (ADR-318); `BROOD_NO_TW_REENTRY`
/// opts out.
pub fn tree_walker_reentry() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_TW_REENTRY")
}

/// `(get m k)` lowers to `PrimOp::MapGet` (ADR-296); `BROOD_NO_MAPGET` opts out.
pub fn map_get_lowering() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_MAPGET")
}

/// `(assoc m k v)` lowers to `PrimOp3::MapAssoc` (ADR-368); `BROOD_NO_MAPASSOC` opts out.
pub fn map_assoc_lowering() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_MAPASSOC")
}

/// `BROOD_IMAGE_TRACE` is set — read once. Where stdlib-image time goes, by phase.
pub fn image_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_IMAGE_TRACE")
}

/// `BROOD_BOOT_TRACE` is set — read once. Boot timings, per slow prelude form.
pub fn boot_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_BOOT_TRACE")
}

/// `BROOD_SCOPE_DBG` is set — read once. The module-scope tracer (KI-120's class).
pub fn scope_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_SCOPE_DBG")
}

/// `BROOD_DERIVE_DBG` is set — read once. Where a file's check spent its time.
pub fn derive_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_DERIVE_DBG")
}

/// `BROOD_DEVIRT_DBG` is set — read once. Devirtualisation decisions.
pub fn devirt_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_DEVIRT_DBG")
}

/// `BROOD_TRACE_GCBLOCK` is set — read once. Checked on every eval entry and exit under
/// debug assertions, where an uncached read took the environment lock on the hottest path
/// of exactly the builds used to hunt races.
pub fn trace_gcblock() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    is_set(&ON, "BROOD_TRACE_GCBLOCK")
}

/// The linear-map rewrites — pipeline and range-fold fusion, and the in-place table
/// tally; `BROOD_LINMAP=0` opts out. Read at every top-level form's expansion, so cached.
pub fn linmap() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !*OFF.get_or_init(|| std::env::var_os("BROOD_LINMAP").is_some_and(|v| v == "0"))
}

/// The in-IR call-site fast link; `BROOD_NO_JIT_ICALL` opts out.
pub fn jit_inline_call() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_JIT_ICALL")
}

/// The higher-order-function cached-arm fast path; `BROOD_NO_HOF` opts out.
pub fn higher_order_fast_path() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_HOF")
}

/// The higher-order call's native fast frame; `BROOD_NO_HOF_JIT` opts out.
pub fn higher_order_native() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_HOF_JIT")
}

/// The unboxed-scalar register worker; `BROOD_NO_I64` opts out.
pub fn jit_scalar_worker() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_I64")
}

/// The JIT's per-slot i64 cache; `BROOD_NO_INT_SLOT_CACHE` opts out.
pub fn int_slot_cache() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_INT_SLOT_CACHE")
}

/// The inline fast-frame call path in hot re-lowerings; `BROOD_NO_XCALL` opts out.
pub fn inline_fast_frame() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_XCALL")
}

/// Lazy module loading (ADR-335); `BROOD_NO_LAZY_LOAD` pins the eager policy.
pub fn lazy_load() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_LAZY_LOAD")
}

/// Signatures from the stdlib image (ADR-370); `BROOD_NO_IMAGE_SIGS` opts out.
pub fn image_signatures() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !is_set(&OFF, "BROOD_NO_IMAGE_SIGS")
}

/// Print the catalogue, grouped, for `brood --debug-flags`.
pub fn print_catalogue() {
    println!(
        "BROOD_* environment flags — all {} the runtime reads, triage groups first.",
        FLAGS.iter().filter(|f| f.ours).count()
    );
    println!("Set any to 1 unless a value is described. The measurement history behind a");
    println!("default lives in its ADR or devlog entry.");
    let width = FLAGS.iter().map(|f| f.name.len()).max().unwrap_or(0);
    // Drive the grouping from GROUP_ORDER rather than from adjacency in the array, so an
    // entry added away from its siblings cannot split its group and print the heading twice.
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
                    // catalogued name find itself and the assertion vacuously true.
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
