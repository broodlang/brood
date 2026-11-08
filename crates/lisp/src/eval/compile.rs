//! The compiling execution engine — ADR-076, [`docs/bytecode-vm.md`].
//!
//! A **closure-compiling VM over a lexically-addressed IR**: a form compiles once
//! into a [`Node`] tree run by a trampoline ([`vm_apply`]). The crux is GC: a
//! call's frame slots are a contiguous region of the **existing** `Heap::roots`
//! operand stack, so the moving collector relocates them in place (the collector's
//! root walk) with **no new root set** — `Node::Local(i)` reads `root_at(base+i)`.
//!
//! **The VM is the default engine** (ADR-076 Stage 3); `BROOD_TIER=0` forces the
//! tree-walker. A closure is VM-compiled when it's built from the core vocabulary
//! ([`Node`] below): `if`/`do`/`let`/`letrec`/`fn`/`quote` plus calls and vector/map
//! literals, with `&optional` (nil- *or* real-default) and any capture (global *or*
//! local — Stage 2c). Because `match`/`match*`/`and`/`or` are macros that expand to
//! exactly these forms, **pattern-matching `fn`s and `match` run on the VM too**. Anything
//! still outside the set — `def`/`quasiquote`/`defmacro`/`binding`, or a body built
//! from movable (conased) forms — **defers to the tree-walker** (`eval::eval`)
//! per-form, so partial compilation is always safe and the language is unchanged.
//! Macros are already expanded by this point (`eval::macros::compile` ran), so the
//! compiler never sees a macro call.
//!
//! Naming note: [`run`] runs **after** `eval::macros::compile` (macroexpand-all +
//! namespace-resolve), on the already-expanded, already-resolved form.

use smallvec::SmallVec;

use std::sync::atomic::{AtomicU64, Ordering};

use std::sync::Arc;

use crate::core::heap::{EnvRoot, Heap, VmCacheKey};

use crate::core::keywords as kw;

use crate::core::value::{
    self, BigIntId, ClosureId, EnvId, MapId, NativeId, PairId, RopeId, StrId, Symbol, Value,
    ValueRef, VecId,
};

use crate::error::{LispError, LispResult, Pos};

/// How far up the execution **tier ladder** a form may go (ADR-222).
///
/// Brood does not choose between engines; it runs a ladder, and each tier falls back to the one
/// below when it declines:
///
/// ```text
/// Native    ──deopt (outcome 1)──▶  Bytecode  ──defer──▶  TreeWalk
/// ```
///
/// Both falls are real and load-bearing, not error paths: `jit_deopt` is documented as "a tiered
/// arm deopted back to the VM mid-run", and `tw_defer` as "calls that fell back to the
/// tree-walker" — `compile::run` "falls back to the tree-walker per form for anything outside the
/// VM's vocabulary". [`Tier::TreeWalk`] is *total*: it accepts every form, which is what makes a
/// ceiling meaningful at all.
///
/// **This type is a ceiling, not a selection.** The runtime always uses the highest tier that
/// applies; the ceiling only says how high it may reach. That is why the ordering is derived and
/// why call sites compare (`>= Tier::Bytecode`) rather than matching every variant: a new tier
/// slots into the ladder instead of forcing a decision at each site. Contrast the `JitBackend`
/// trait, where alternatives genuinely are alternatives.
///
/// **A ceiling bounds capability, too.** Tier 0 has no reified frame stack, so a top-level
/// `receive` blocks its worker (`vm_run_bc`'s carve-out) and it is not preemptible the same way;
/// only tier 1 and above tier to native. Under the old two-engine framing those read as
/// inconsistencies between peers. As tiers they are simply what you give up by lowering the
/// ceiling.
///
/// **What the ladder does not abstract.** `eval/compile/ir.rs` — `Node`, `Inst`, `Chunk`,
/// `CompiledArm` — is shared by every tier *and* the deopt/journal protocol. So replacing tier 1
/// means rewriting `exec_chunk.rs` + `vm_run_bc.rs` (~2 kLOC) while keeping that IR: a register
/// VM, threaded code, or computed-goto dispatch all fit; a different IR does not.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    /// **Tier 0** — tree-walk the `Node` (`eval::eval`). The bottom of the ladder, the deopt
    /// floor, and the differential reference. Total by construction: whatever a higher tier
    /// declines lands here, so it must accept everything. ~10× slower than tier 1.
    TreeWalk,
    /// **Tier 1** — the closure-compiling bytecode VM (ADR-076 Stage 3). Defers a form outside
    /// its vocabulary to tier 0; the default floor for real work.
    Bytecode,
    /// **Tier 2** — native code from a [`crate::jit::JitBackend`] (ADR-101/221), *earned* by
    /// tiering at run time. Deopts to tier 1 on a guard miss. The default ceiling.
    ///
    /// AOT would be this tier **admitted at load** rather than earned — which is the whole of
    /// the difference, and the reason the ladder framing is worth having before AOT exists.
    Native,
}

impl Tier {
    /// Every tier, highest first. Used by `crates/lisp/benches/eval.rs` to build its tier × size
    /// grid, so a new tier gets benchmark rows without touching each `#[divan::bench]` — and the
    /// tier-1 row is itself useful, since JIT-vs-no-JIT per row is a standing measurement the
    /// frontier docs quote.
    ///
    /// **Deliberately not the differential set.** `tests/differential.rs` compares an explicit
    /// pair; iterating this there would add a third full suite run to every `make test-both` and
    /// to CI, for coverage `tests/jit.rs` already provides. See that file's `DIFFERENTIAL_TIERS`.
    pub const ALL: &'static [Tier] = &[Tier::Native, Tier::Bytecode, Tier::TreeWalk];

    /// Short label for benchmark rows and traces. `Vm`/`Tw` are unchanged from the two-engine
    /// era on purpose: `scripts/bench-ratio.awk` reads them out of divan's arg column and
    /// `docs/benchmarking.md` quotes them.
    pub const fn short(self) -> &'static str {
        match self {
            Tier::Native => "Jit",
            Tier::Bytecode => "Vm",
            Tier::TreeWalk => "Tw",
        }
    }
}

thread_local! {
    /// Per-thread ceiling override for the differential harnesses (and any tool that wants to pin
    /// a tier); `None` defers to the cached env/default choice. Checked before the cache so it
    /// wins; only a top-level form consults it, so the cost is negligible.
    /// See [`set_forced_ceiling`].
    static FORCED_CEILING: std::cell::Cell<Option<Tier>> = const { std::cell::Cell::new(None) };
}

/// Force (or clear) the tier ceiling for the current thread, overriding the env and the build
/// default — this is what lets one process run a form at several ceilings
/// (`crates/lisp/tests/differential.rs`). `None` restores the default.
pub fn set_forced_ceiling(choice: Option<Tier>) {
    FORCED_CEILING.with(|c| c.set(choice));
}

/// How high this thread may climb the ladder. A per-thread [`set_forced_ceiling`] override wins;
/// otherwise the default is [`Tier::Native`] — every build tiers to native unless told not to.
///
/// One knob, `BROOD_TIER` (ADR-222):
///
/// | set | ceiling |
/// |---|---|
/// | `BROOD_TIER=0` | [`Tier::TreeWalk`] |
/// | `BROOD_TIER=1` | [`Tier::Bytecode`] |
/// | `BROOD_TIER=2`, unset, or anything else | [`Tier::Native`] |
///
/// An unrecognised value keeps the default rather than guessing — a typo must not silently
/// lower the ceiling. The env/default choice is read once and cached; it cannot change mid-run,
/// but the override can.
pub fn tier_ceiling() -> Tier {
    if let Some(forced) = FORCED_CEILING.with(|c| c.get()) {
        return forced;
    }
    static CEILING: std::sync::OnceLock<Tier> = std::sync::OnceLock::new();
    *CEILING.get_or_init(
        || match std::env::var("BROOD_TIER").as_deref().map(str::trim) {
            Ok("0") => Tier::TreeWalk,
            Ok("1") => Tier::Bytecode,
            _ => Tier::Native,
        },
    )
}

/// Run one already-macro-expanded **top-level** form as high up the ladder as the ceiling allows.
///
/// The tier choice for a top-level form lives here and nowhere else. Three call sites
/// (`eval`/`load` in `builtins/evaluation.rs`, `Interp::eval_str` in `lib.rs`) had this exact
/// `if`/`else` inline, so a third [`Engine`] would have meant finding all of them; now it means
/// adding an arm to one `match`. The two sites that legitimately differ keep their own dispatch:
/// `eval.rs`'s top-level `def` (its VM branch tags the error with the RHS's position) and
/// `vm_run_bc`'s tree-walker carve-out (an inverse check that hands the *whole* form over).
///
/// `compile::run` itself falls back to the tree-walker per form for anything outside the VM's
/// vocabulary, so a ceiling of [`Tier::Bytecode`] or above never restricts what can be
/// evaluated — it only decides how fast.
pub(crate) fn run_top_form(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    // A comparison, not a match: the ceiling says how high the ladder may go, and tier 1 is
    // where a top-level form enters it (tier 2 is reached from inside, by tiering). A new tier
    // above Bytecode needs no change here — which is the ladder's point.
    if tier_ceiling() >= Tier::Bytecode {
        run(heap, form, env)
    } else {
        crate::eval::eval(heap, form, env)
    }
}

/// "This `Node::Call` has no call-site inline cache" — the callee isn't a free
/// global reference (ADR-096).
pub const NO_SITE: u32 = u32::MAX;

mod ir;

// `ArmHandle`, the process-local arm handle (KI-40), is named by `Heap`'s IC entry and
// live-arm stack, so it is re-exported at the crate-visible `compile::` path too.
pub use ir::{
    ArmHandle, Chunk, CompiledArm, CompiledClosure, ConstVal, HandleKind, Inst, Node, PrimOp,
    PrimOp1, PrimOp3, rewrite_arm_handles,
};

// The pub(super) items of `ir` this module needs internally.
use ir::{ArmSpec, ChunkExit, Step, next_arm_uid};

mod lower;
pub(crate) use lower::*;
mod walkers;
pub(crate) use walkers::*;
mod closure;
pub(crate) use closure::*;
mod emit;
pub(crate) use emit::*;

mod prim_exec;

pub(crate) use prim_exec::*;

mod dispatch;

pub(crate) use dispatch::*;

mod exec_chunk;

pub(crate) use exec_chunk::*;

mod vm_run_bc;

/// How a top-level program ended — handed to a host that drives one itself (`Interp::start_program`).
pub use vm_run_bc::ProgramExit;
pub(crate) use vm_run_bc::*;

mod inline;

pub(crate) use inline::*;

#[cfg(feature = "jit")]
mod jit_runtime;

#[cfg(feature = "jit")]
pub(crate) use jit_runtime::*;

#[cfg(test)]
mod tests;

// ===================== executor (Node → value) =====================

/// Resolve a [`Step`] to a value, running a `Tail` to completion. In value
/// positions the step is always `Done` (sub-nodes compile with `tail = false`);
/// this also makes a stray tail safe rather than a panic. A `Tail` carries its own
/// callee env (Stage 2c), so `force` needs no ambient env.
fn force(heap: &mut Heap, step: Step) -> LispResult {
    match step {
        Step::Done(v) => Ok(v),
        Step::Tail {
            compiled,
            args,
            genv,
            // `vm_apply` resolves + installs the callee's block itself (and restores
            // the caller's on exit), so the step's memoised bases aren't needed here.
            bases: _,
        } => vm_apply(heap, compiled, &args, genv),
    }
}

/// Resolve a 3-arg call head to an inlinable [`PrimOp3`], following a **thin wrapper**
/// exactly as the 2-ary [`resolve_prim`] does. Read against the live global env — a
/// redefined head simply doesn't match.
///
/// Following the wrapper is load-bearing: `table/put` is a `std/table.blsp` closure whose
/// whole body is `(%table-put t k v)`, and accepting only a direct native binding silently
/// demotes it to an ordinary `Call` in the hot arm (an order-of-magnitude loss on `sieve`
/// that no correctness test sees).
///
/// The identity map is required rather than applied: `Node::Prim3` has no argument
/// permutation (unlike `Node::Prim2`'s `map`), so a wrapper that reorders its parameters
/// must decline rather than silently compile the arguments in the wrong order.
fn resolve_prim3(heap: &Heap, h: Symbol) -> Option<PrimOp3> {
    let v = heap.env_get(heap.global(), h)?;
    // The two map ops (ADR-368), by head — the 3-ary twins of `PrimOp::MapGet`'s `get`
    // rule: accepted only while the head still resolves to the PRELUDE closure, so a user
    // `(def get …)` cleanly disables the inline and the epoch guard re-validates on a
    // redefinition. `get`'s 3-arity is `%map-get` then, on a miss, `%lookup-miss` with the
    // default as its fallback, which `Heap::map_get3_inline` mirrors; `assoc`'s single-pair arm is `%map-assoc` for a map
    // and `%vector-assoc` for a vector, and only the map half is inlined.
    if crate::core::value::symbol_is(h, "get") {
        // `BROOD_NO_MAPGET` opts out.
        if !crate::diagnostics::debug_flags::map_get_lowering() {
            return None;
        }
        return match v.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => {
                Some(PrimOp3::MapGet3)
            }
            _ => None,
        };
    }
    // `(nth v i default)`: the canonical prelude `nth`, the 3-ary twin of
    // `resolve_prim`'s `nth` → `VectorRef` rule.
    if crate::core::value::symbol_is(h, "nth") {
        return match v.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => {
                Some(PrimOp3::VectorRefOr)
            }
            _ => None,
        };
    }
    if crate::core::value::symbol_is(h, "assoc") {
        // `BROOD_NO_MAPASSOC` opts out — its own lever, apart from `BROOD_NO_MAPGET`, because
        // this one ALLOCATES from native code where the reads do not.
        if !crate::diagnostics::debug_flags::map_assoc_lowering() {
            return None;
        }
        return match v.unpack() {
            ValueRef::Fn(id) if id.region() == crate::core::value::PRELUDE => {
                Some(PrimOp3::MapAssoc)
            }
            _ => None,
        };
    }
    let nid = match v.unpack() {
        ValueRef::Native(id) => id,
        ValueRef::Fn(id) => {
            let (inner_head, map) = crate::eval::passthrough_arm(heap, id, 3)?;
            if map[..] != [0, 1, 2] {
                return None;
            }
            let inner = match inner_head.unpack() {
                ValueRef::Sym(s) => heap.env_get(heap.global(), s)?,
                _ => inner_head,
            };
            match inner.unpack() {
                ValueRef::Native(id) => id,
                _ => return None,
            }
        }
        _ => return None,
    };
    PrimOp3::from_native_name(&heap.native(nid).name)
}

/// If `form` is `(def name rhs)` (name a symbol, exactly one value), return `(name, rhs)`
/// so the driver can run `rhs` capturably and bind after. `None` for anything else — a
/// `(def name)` with no value, a `def` with a bad shape (handled by the normal `def`
/// error), or any non-`def` form.
fn def_rhs(heap: &Heap, form: Value) -> Option<(Value, Value)> {
    if !matches!(form.unpack(), ValueRef::Pair(_)) {
        return None;
    }
    let parts = heap.list_to_vec(form).ok()?;
    if parts.len() == 3
        && matches!(parts[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::DEF))
        && matches!(parts[1].unpack(), ValueRef::Sym(_))
    {
        Some((parts[1], parts[2]))
    } else {
        None
    }
}

/// Bind `name` to the already-computed value `v` with the full `def` semantics, by
/// re-evaluating `(def name (quote v))` on the tree-walker — a trivial form (the RHS is a
/// literal, so it neither compiles-to-VM nor `receive`s), which reuses `def`'s naming,
/// promote-into-shared-RUNTIME, and reload diagnostics rather than re-implementing them.
fn bind_def(heap: &mut Heap, name: Value, v: Value) -> Result<(), LispError> {
    let g = heap.global();
    let base = heap.roots_len();
    heap.push_root(name);
    heap.push_root(v);
    let v = heap.root_at(base + 1);
    let quote = heap.list(vec![value::sym(kw::QUOTE), v]);
    heap.push_root(quote);
    let name = heap.root_at(base);
    let quote = heap.root_at(base + 2);
    let form = heap.list(vec![value::sym(kw::DEF), name, quote]);
    heap.push_root(form);
    let form = heap.root_at(base + 3);
    let r = crate::eval::eval(heap, form, g);
    heap.truncate_roots(base);
    r.map(|_| ())
}

/// Runaway guard for the explicit frame stack: a clean `STACK_DEPTH_EXCEEDED` once
/// the bytecode call depth crosses this many frames, replacing the native-stack byte
/// guard the `Node` engine uses (the driver doesn't grow the native stack per Brood
/// call, so unbounded non-tail recursion grows `frames` + `Heap::roots` instead).
/// Generous — the soft-memory cap (ADR-043) is the real backstop; this just turns an
/// infinite non-tail recursion into a catchable error before it exhausts memory.
const MAX_BC_FRAMES: usize = 1 << 20;

/// The `entry_epoch` a FRESH frame starts at: never a real epoch, so the frame's first
/// `Inst::SelfCall` takes the hot-reload guard's slow path and proves the arm's `dbg_name`
/// still names this arm before looping on it. A frame entered through an alias of a
/// rebound name would otherwise loop on the old body forever, so starting at the CURRENT
/// epoch (assuming the name still means this arm at entry) is unsound. The proof is
/// memoised on the arm ([`CompiledArm::self_bound_stamp`]), so the cost is one compare
/// per activation while no `def` intervenes. Distinct from `u64::MAX` (the stale-arm and
/// deopt-resume sentinel), which always re-resolves.
pub(crate) const SELF_UNCHECKED_EPOCH: u64 = u64::MAX - 1;

/// The key under which [`CompiledArm::self_bound_stamp`] memoises "`dbg_name` resolves to
/// this arm": the runtime's tag in the high 24 bits over the global epoch in the low 40.
/// A shared prelude arm is reached from several runtimes whose epochs count
/// independently, so the epoch alone could prove a fact about another runtime's table.
#[inline]
pub(crate) fn self_binding_stamp(heap: &Heap, epoch: u64) -> u64 {
    (heap.runtime_tag() << 40) | (epoch & ((1u64 << 40) - 1))
}

/// One suspended bytecode activation: where to resume (`ip`) and how to tear its
/// frame down. Promoted out of [`vm_run_bc`]'s body (it was a local `struct Frame`)
/// so a captured [`Suspended`] continuation can hold the whole stack. The indices
/// (`base`/`env_base`/`arm_slot`) are positions into `Heap::roots`/`env_roots`/
/// `live_vm_arms`, which stay valid across a suspend because the driver does **not**
/// unwind them when it captures (a collection while parked relocates the *values* at
/// those positions in place, keeping the indices good — ADR-100 §8).
pub(crate) struct BcFrame {
    arm: Arc<ArmHandle>,
    ip: usize,
    base: usize,
    env: EnvRoot,
    env_base: usize,
    arm_slot: usize,
    /// This frame's arm's IC block bases (ADR-175 Phase A) — reinstalled as the
    /// heap's current cursors when control returns to (or resumes) this frame.
    ic_bases: (u32, u32),
    /// Persisted back-edge counter for this frame — see `exec_chunk`'s `back_edges` param.
    #[cfg(feature = "jit")]
    back_edges: u32,
    /// The global epoch this frame's `Inst::SelfCall` hot-reload guard last validated at —
    /// see `exec_chunk`'s `entry_epoch` param. Persisted for the same reason as
    /// `back_edges`: the frame re-enters `exec_chunk` after every non-tail call.
    entry_epoch: u64,
}

/// A captured VM continuation — the reified call stack of a green process parked at a
/// clean `receive` (ADR-100 §8, the corosensei-removal migration). It is plain `Send`
/// data: `frames` (the pending non-tail callers) + `cur` (the frame that was running)
/// + the driver's entry marks (for unwinding on a later error) + the `receive`
/// deadline (so the scheduler arms a timer). The operand stack and frame slots it
/// references stay live on the owning process's `Heap::roots`; this struct only holds
/// the *control* state. Hand it back to [`vm_run_bc`] as `resume` to replay from the
/// suspending `%receive` call. The scheduler (`run_one`) stores it on the parked
/// process's mailbox, or re-enqueues it on a preempt.
pub(crate) struct Suspended {
    frames: Vec<BcFrame>,
    cur: BcFrame,
    entry_roots: usize,
    entry_env: usize,
    entry_arms: usize,
    /// `Heap::vm_handlers` length at the driver's entry: the open `try`s below it belong
    /// to an outer driver and are never this one's to catch into (ADR-400).
    handler_floor: u32,
    /// The dynamic stack's length at the driver's entry: an error that leaves the driver
    /// truncates back to it, undoing every inline `binding` it skipped (ADR-402). `u32`s,
    /// both, so the two floors take the space one word did — a parked process's
    /// continuation counts against the per-process floor (`tests/process_floor.rs`).
    dyn_floor: u32,
    /// The `(receive … (after ms …))` absolute wake time, or `None` to wait forever —
    /// the scheduler arms a timer from this so a parked process still fires its
    /// `after` clause.
    pub(crate) deadline: Option<crate::process::SchedDeadline>,
}

impl Suspended {
    /// One-line park/resume descriptor for `BROOD_SCHED_DBG` (KI-88's ip-trace probe):
    /// the current frame's ip + pending frame count, enough to spot a resume whose
    /// continuation does not match any recorded park.
    pub(crate) fn dbg_line(&self) -> String {
        format!("ip={} frames={}", self.cur.ip, self.frames.len())
    }
}

/// A parked or preempted process's continuation: the VM's frame stack ([`Suspended`]) or the
/// tree-walker's ([`TwSuspended`](crate::eval::TwSuspended), ADR-416). Both are control state
/// only — what they reference stays on the owning heap's root stacks — so either can resume on
/// any worker.
pub(crate) enum Continuation {
    Vm(Box<Suspended>),
    Tw(Box<crate::eval::TwSuspended>),
}

impl Continuation {
    /// The VM's continuation. A nested VM run only ever captures its own.
    pub(crate) fn into_vm(self) -> Box<Suspended> {
        match self {
            Continuation::Vm(s) => s,
            Continuation::Tw(_) => unreachable!("a VM run captured a tree-walker continuation"),
        }
    }

    /// One-line park/resume descriptor for `BROOD_SCHED_DBG`.
    pub(crate) fn dbg_line(&self) -> String {
        match self {
            Continuation::Vm(s) => s.dbg_line(),
            Continuation::Tw(s) => s.dbg_line(),
        }
    }
}

/// What a [`vm_run_bc`] call produced (ADR-100 §8). A real error is the `Err` of the
/// enclosing `Result`. A **nested** run (`vm_apply`, `top_level=false`) only ever
/// produces `Done` (it can't capture across the native boundary); the other three are
/// the scheduler outcomes the **top-level body driver** reifies at its loop-top
/// safepoint in place of a coroutine yield.
pub(crate) enum VmOutcome {
    /// The body finished with this value.
    Done(Value),
    /// A clean `receive` parked: the captured continuation to store + resume on a
    /// wake (§8.2). `run_one` parks it on the mailbox.
    Suspended(Continuation),
    /// The reduction budget was exhausted at a loop-top safepoint (the state-capture
    /// analogue of `Suspend::Preempt`): captured the continuation so `run_one` can
    /// **re-enqueue** it (possibly onto another worker — live migration, §7).
    Preempted(Continuation),
    /// A hard `:kill` was pending at a loop-top safepoint (the analogue of
    /// `Suspend::Kill`): stop now, no capture — `run_one` retires the process with the
    /// mailbox's kill reason. Untrappable by construction (fires below `%try`).
    Killed,
}

/// Compile-then-run a resolved top-level `form` — the VM entry the form loops use
/// at a ceiling of [`Tier::Bytecode`] or above. A form built from the core vocabulary runs on
/// the VM (an
/// empty lexical scope: no locals at top level); anything else defers to the
/// tree-walker. `env` is the process's global/root env.
pub fn run(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    // A top-level form runs as a NESTED driver (`vm_apply`), which cannot be captured, and
    // every caller is native (`reflect/eval`, `eval-string`, the loader, a tree-walked `def`). A `receive`
    // in it must block, not suspend: a suspend rode up to the enclosing body driver, which
    // captured at the CALL into the native, and the resume ran the whole form again — a
    // `reflect/eval`'d spawn-then-receive spawned anew on every message (KI-245). The same
    // guard the tree-walker's entries take.
    let _not_capturable = crate::process::TreeWalkGuard::enter();
    let mut scope = Scope::new();
    scope.stage_calls = true;
    // When invoked with a *non-global* env — a `def` RHS evaluated inside a `let`,
    // e.g. `(let (me …) (def f (fn () me)))` — the form's closures must be able to
    // capture the enclosing lexicals. Seed them as `enclosing` names so a VM-compiled
    // closure snapshots them (`compile_captures` reads each via `env_get` on the live
    // env at `MakeClosure` time); without this the closure resolves them as unbound
    // globals once the lexical frame is gone (e.g. when a `def`'d closure is later
    // called, or shipped to another node). The overwhelmingly common case is
    // `env == global` (top-level forms): no lexical frames, so this is a no-op.
    if !heap.is_global(env) {
        let mut e = env;
        while !heap.is_global(e) {
            let (parent, bindings) = heap.env_frame_ref(e);
            for &(sym, _) in bindings.iter() {
                scope.enclosing.push(sym);
            }
            match parent {
                Some(p) => e = p,
                None => break,
            }
        }
    }
    match compile_node(heap, form, &mut scope, false) {
        Some(node) => {
            // The form becomes a throwaway zero-parameter arm with a chunk of its own and
            // runs on the bytecode driver like any other activation — through `vm_apply`,
            // the nested-run entry: the frame (`scope.max` slots, a top-level `let`'s
            // binders included), the IC block install and the caller's restore (`run`
            // nests — a `load` inside an arm), the live-arm registration that keeps its
            // RUNTIME `Const`s rewritable under a nested compaction, and the native-stack
            // guard are the driver's, as for any call.
            let chunk = compile_chunk(&node);
            let arm = CompiledArm {
                nslots: scope.max,
                nsites: scope.sites,
                ngsites: ir::gsite_reservation(scope.gsites, scope.sites),
                site_pos: std::mem::take(&mut scope.site_pos).into_boxed_slice(),
                staged_head_pos: std::mem::take(&mut scope.staged_head_pos).into_boxed_slice(),
                local_call_heads: scope.take_local_call_heads(),
                has_runtime_handles: node_has_rt_handles(&node),
                chunk,
                body: node,
                ..CompiledArm::default()
            };
            // Runs once: nothing to tier, and no frame reserve laid out for native code.
            #[cfg(feature = "jit")]
            arm.latch_bailed("runs-once");
            vm_apply(heap, ArmHandle::new(Arc::new(arm)), &[], env)
        }
        None => crate::eval::eval(heap, form, env),
    }
}

/// Apply a closure *value* (not a source form) to `args` through the VM when it's
/// VM-eligible, falling back to the tree-walker (`eval::apply`) otherwise — the
/// entry point for callers that hold a [`Value::Fn`] and want VM execution (a
/// spawned process's body, for one). `genv` is the env a *native* callee runs in; a VM closure runs in its own captured env
/// (read off the closure inside `dispatch`). `tail = false`: this is a value
/// context, so any tail call is forced to completion by `force`.
pub fn apply_value(heap: &mut Heap, callee: Value, args: &[Value], genv: EnvId) -> LispResult {
    let argv: SmallVec<[Value; 4]> = args.iter().copied().collect();
    let step = dispatch(heap, callee, argv, false, genv)?;
    force(heap, step)
}

/// Apply `callee` through the active engine: the VM when enabled (a VM-eligible
/// callback runs compiled), the tree-walker under `BROOD_TIER=0` (keeps the
/// differential / escape-hatch mode honest). `eval::apply` must stay pure
/// tree-walker — it's `dispatch`'s fallback, so routing it back through
/// `apply_value` would recurse. Use for once-per-call thunks (`try`, `binding`,
/// `isolate`); NOT for the `apply` builtin itself — that needs the TW's inline
/// `apply`-unfolding trampoline for O(1)-stack `(apply f …)`-driven tail recursion.
pub fn apply_engine(heap: &mut Heap, callee: Value, args: &[Value], genv: EnvId) -> LispResult {
    if tier_ceiling() >= Tier::Bytecode {
        apply_value(heap, callee, args, genv)
    } else {
        crate::eval::apply(heap, callee, args, genv)
    }
}

// The backend-independent lowering decisions. **Not** gated on `feature = "jit"`: the frame
// layout two of them describe (`jit_spill_reserve`, `jit_ckpt_depth`) is what the VM sizes
// frames by, JIT or no JIT, and the rest is analysis any consumer may want to read without a
// backend present.
mod jit_plan;

use jit_plan::{jit_ckpt_depth, jit_spill_reserve};

#[cfg(feature = "jit")]
mod jit_lower;

#[cfg(feature = "jit")]
mod deopt_reason;

#[cfg(feature = "jit")]
pub(crate) use deopt_reason::DeoptReason;

#[cfg(feature = "jit")]
pub(crate) use jit_lower::{
    jit_lower_arm, jit_lower_arm_hot, jit_lower_inlined_arm, take_mid_emit_reason,
    xcall_relower_enabled,
};

// Reached by `jit::cranelift`'s `JitBackend` tiering advisories, which is the only way the
// tiering glue is allowed to ask "have I demoted this fn off the register worker?" (ADR-221).
#[cfg(feature = "jit")]
pub(crate) use jit_lower::{arm_i64_eligible, arm_i64_too_deep, i64_mark_too_deep};
