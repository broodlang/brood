//! The compiling execution engine — ADR-076, [`docs/bytecode-vm.md`].
//!
//! A **closure-compiling VM over a lexically-addressed IR**: a form compiles once
//! into a [`Node`] tree run by a trampoline ([`vm_apply`]). The crux is GC: a
//! call's frame slots are a contiguous region of the **existing** `Heap::roots`
//! operand stack, so the moving collector relocates them in place (`arena_flip`'s
//! root walk) with **no new root set** — `Node::Local(i)` reads `root_at(base+i)`.
//!
//! **The VM is the default engine** (ADR-076 Stage 3); `BROOD_VM=0` forces the
//! tree-walker. A closure is VM-compiled when it's built from the core vocabulary
//! ([`Node`] below): `if`/`do`/`let`/`letrec`/`fn`/`quote` plus calls and vector/map
//! literals, with `&optional` (nil- *or* real-default) and any capture (global *or*
//! local — Stage 2c). Because `match`/`match*`/`and`/`or` are macros that expand to
//! exactly these forms, **pattern-matching `fn`s and `match` run on the VM too** (the
//! `quote`/literal in `match*`'s no-match arm used to force them to defer). Anything
//! still outside the set — `def`/`quasiquote`/`defmacro`/`binding`, or a body built
//! from movable (conased) forms — **defers to the tree-walker** (`eval::eval`)
//! per-form, so partial compilation is always safe and the language is unchanged.
//! Macros are already expanded by this point (`eval::macros::compile` ran), so the
//! compiler never sees a macro call.
//!
//! Naming note: [`run`] runs **after** `eval::macros::compile` (macroexpand-all +
//! namespace-resolve), on the already-expanded, already-resolved form.

use smallvec::SmallVec;

use std::ptr::NonNull;

use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};

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
    /// frontier docs quote (`fib` 54×, `collatz` 40×) and previously had to be produced by hand.
    ///
    /// **Deliberately not the differential set.** `tests/differential.rs` compares an explicit
    /// pair; iterating this there would add a third full suite run to every `make test-both` and
    /// to CI, for coverage `tests/jit.rs` already provides. See that file's `DIFFERENTIAL_TIERS`.
    pub const ALL: &'static [Tier] = &[Tier::Native, Tier::Bytecode, Tier::TreeWalk];

    /// Short label for benchmark rows and traces. `Vm`/`Tw` are unchanged from the two-engine
    /// era on purpose: `scripts/bench_ratio.py` reads them out of divan's arg column and
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
/// One knob, `BROOD_TIER=0|1|2`, with the two flags it replaces kept as **aliases** because they
/// are in muscle memory and quoted across the docs:
///
/// | set | ceiling |
/// |---|---|
/// | `BROOD_TIER=0` \| `BROOD_VM` falsy (`0`/`false`/`off`/`no`/empty) | [`Tier::TreeWalk`] |
/// | `BROOD_TIER=1` \| `BROOD_NO_JIT` set | [`Tier::Bytecode`] |
/// | `BROOD_TIER=2`, or nothing | [`Tier::Native`] |
///
/// `BROOD_TIER` wins if both are set. Before ADR-222 these were two unrelated checks in two
/// modules — this one and `jit_tier`'s own `BROOD_NO_JIT` read — which is why "the engine" and
/// "the JIT switch" looked like different kinds of thing. The env/default choice is read once and
/// cached; it cannot change mid-run, but the override can.
pub fn tier_ceiling() -> Tier {
    if let Some(forced) = FORCED_CEILING.with(|c| c.get()) {
        return forced;
    }
    static ON: std::sync::OnceLock<Tier> = std::sync::OnceLock::new();
    fn truthy(v: &str) -> bool {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off" | "no"
        )
    }
    *ON.get_or_init(|| {
        match std::env::var("BROOD_TIER").as_deref().map(str::trim) {
            Ok("0") => return Tier::TreeWalk,
            Ok("1") => return Tier::Bytecode,
            Ok("2") => return Tier::Native,
            // Anything else (unset, or a value we do not recognise) falls through to the
            // aliases rather than guessing — a typo must not silently lower the ceiling.
            _ => {}
        }
        if std::env::var("BROOD_VM").is_ok_and(|v| !truthy(&v)) {
            return Tier::TreeWalk;
        }
        if std::env::var_os("BROOD_NO_JIT").is_some() {
            return Tier::Bytecode;
        }
        Tier::Native
    })
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

pub use ir::{rewrite_arm_handles, Inst};

pub use ir::{
    Chunk, CompiledArm, CompiledClosure, ConstVal, HandleKind, Node, PrimOp, PrimOp1, PrimOp3,
};

// pub(super) items from ir: explicitly imported so `use ir::*` (pub-only) doesn't miss them.
// pub items re-exported above; these are pub(super) items needed internally:
use ir::{next_arm_uid, ArmSpec, ChunkExit, Step};

// The process-local arm handle (KI-40) — named by `Heap`'s IC entry and live-arm stack,
// so it is re-exported at the crate-visible `compile::` path rather than kept private.
pub use ir::ArmHandle;

// NodePtr is pub in ir, but not re-exported from mod.rs — import privately:
use ir::NodePtr;

mod lower;
pub(crate) use lower::*;
mod walkers;
pub(crate) use walkers::*;
mod closure;
pub(crate) use closure::*;
mod emit;
pub(crate) use emit::*;

mod exec_value;

pub(crate) use exec_value::*;

mod dispatch;

pub(crate) use dispatch::*;

mod exec_chunk;

pub(crate) use exec_chunk::*;

mod vm_run_bc;

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
/// This used to accept only a *direct* native binding, on the stated grounds that its one
/// member `table-put` "has no prelude wrapper to follow". The namespacing waves made that
/// premise false: the head is now `table/put`, a `std/table.blsp` closure whose whole body
/// is `(%table-put t k v)`. Nothing errored — the call simply stopped inlining and became
/// an ordinary `Call` inside the hot arm, which is what put `sieve` **11.6× slower**
/// (34 → 394 ms) with no failing test anywhere. The 2-ary path kept following its wrapper,
/// so `table/has?` went on inlining beside it and the asymmetry was invisible.
///
/// The identity map is required rather than applied: `Node::Prim3` has no argument
/// permutation (unlike `Node::Prim2`'s `map`), so a wrapper that reorders its parameters
/// must decline rather than silently compile the arguments in the wrong order.
fn resolve_prim3(heap: &Heap, h: Symbol) -> Option<PrimOp3> {
    let nid = match heap.env_get(heap.global(), h)?.unpack() {
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
}

/// A captured VM continuation — the reified call stack of a green process parked at a
/// clean `receive` (ADR-100 §8, the corosensei-removal migration). It is plain `Send`
/// data: `frames` (the pending non-tail callers) + `cur` (the frame that was running)
/// + the driver's entry marks (for unwinding on a later error) + the `receive`
/// deadline (so the scheduler arms a timer). The operand stack and frame slots it
/// references stay live on the owning process's `Heap::roots`; this struct only holds
/// the *control* state. Hand it back to [`vm_run_bc`] as `resume` to replay from the
/// suspending `%receive` call. The scheduler cutover (§8.3) stores it in place of a
/// `Coroutine`; for now only the capture→resume unit test consumes it.
pub(crate) struct Suspended {
    frames: Vec<BcFrame>,
    cur: BcFrame,
    entry_roots: usize,
    entry_env: usize,
    entry_arms: usize,
    /// The `(receive … (after ms …))` absolute wake time, or `None` to wait forever —
    /// the scheduler arms a timer from this so a parked process still fires its
    /// `after` clause.
    pub(crate) deadline: Option<web_time::Instant>,
}

impl Suspended {
    /// One-line park/resume descriptor for `BROOD_SCHED_DBG` (KI-88's ip-trace probe):
    /// the current frame's ip + pending frame count, enough to spot a resume whose
    /// continuation does not match any recorded park.
    pub(crate) fn dbg_line(&self) -> String {
        format!("ip={} frames={}", self.cur.ip, self.frames.len())
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
    Suspended(Suspended),
    /// The reduction budget was exhausted at a loop-top safepoint (the state-capture
    /// analogue of `Suspend::Preempt`): captured the continuation so `run_one` can
    /// **re-enqueue** it (possibly onto another worker — live migration, §7).
    Preempted(Suspended),
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
    let mut scope = Scope::new();
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
            // A top-level `let` introduces frame slots too — give the form a frame
            // of `scope.max` nil slots (like a 0-param closure), then tear it down.
            // The top-level env is the (immovable) process global, so `root_env`
            // keeps it inline; rooting it uniformly keeps `exec_node`'s contract.
            //
            // Wrap the transient top-level node in a throwaway arm and register it as
            // LIVE: like a `vm_apply` frame, its `Const` literals are promoted RUNTIME
            // handles that a nested compaction (a sub-call into `load`/`eval`) would
            // strand — registering it lets `runtime_collect` rewrite them in place.
            let has_runtime_handles = node_has_rt_handles(&node);
            let arm = Arc::new(CompiledArm {
                nrequired: 0,
                noptional: 0,
                optional_defaults: Box::new([]),
                rest_slot: None,
                nslots: scope.max,
                // Top-level forms allocate sites through this scope like any arm; the
                // throwaway arm needs the counts so `exec_value`'s activation resolves
                // an IC block covering them.
                nsites: scope.sites,
                ngsites: scope.gsites,
                uid: next_arm_uid(),
                site_pos: std::mem::take(&mut scope.site_pos).into_boxed_slice(),
                body: node,
                // Top-level forms run via `exec_value` below, not the bytecode loop
                // (Stage 1 bytecode is reached only through `vm_apply`); no chunk.
                chunk: None,
                has_runtime_handles,
                jit_code: AtomicPtr::new(std::ptr::null_mut()),
                jit_calls: AtomicU32::new(0),
                deopt_watch: false,
                jit_deopts: AtomicU32::new(0),
                float_globals: std::sync::OnceLock::new(),
                self_global_ok: std::sync::atomic::AtomicBool::new(false),
                ckpt_slot: u32::MAX,
                compile_epoch: AtomicU64::new(0),
                share_key: None,
                shared_published: std::sync::atomic::AtomicBool::new(false),
                fn_name: None,
                src_file: None,
                capture_names: Box::new([]),
                #[cfg(feature = "jit")]
                inline_name: None,
                dbg_name: None,
                #[cfg(feature = "jit")]
                inline_stride: 0,
                #[cfg(feature = "jit")]
                inline_nslots: 0,
                #[cfg(feature = "jit")]
                inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
                #[cfg(feature = "jit")]
                inline_queued: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "jit")]
                inline_installed: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "jit")]
                xcall_wanted: std::sync::OnceLock::new(),
                #[cfg(feature = "jit")]
                leaf: None,
            });
            let arm_slot = if arm.has_runtime_handles {
                heap.live_arm_push(ArmHandle::new(arm.clone()))
            } else {
                usize::MAX
            };
            let env_base = heap.env_roots_len();
            let genv = heap.root_env(env);
            let base = heap.roots_len();
            for _ in 0..scope.max {
                heap.push_root(Value::nil());
            }
            // Install the top-level form's IC block for its activation (ADR-175
            // Phase A), restoring the caller's after — `run` nests (a `load` inside
            // an arm), so the enclosing activation's cursors must survive.
            let saved_bases = heap.set_ic_bases(heap.vm_arm_block(&arm));
            let r = exec_value(heap, &arm.body, base, genv);
            heap.set_ic_bases(saved_bases);
            heap.truncate_roots(base);
            heap.truncate_env_roots(env_base);
            if arm_slot != usize::MAX {
                heap.live_arm_truncate(arm_slot);
            }
            r
        }
        None => crate::eval::eval(heap, form, env),
    }
}

/// Apply a closure *value* (not a source form) to `args` through the VM when it's
/// VM-eligible, falling back to the tree-walker (`eval::apply`) otherwise — the
/// entry point for callers that hold a [`Value::Fn`] and want VM execution. A
/// spawned process's body uses this so it runs on the VM (with inlined
/// primitives) like top-level code via [`run`], instead of the tree-walker:
/// before this, `eval::apply` ran every green process tree-walked even under
/// `BROOD_VM=1`, ~4–5× slower (most of `pfib`'s gap to Elixir). `genv` is the
/// env a *native* callee runs in; a VM closure runs in its own captured env
/// (read off the closure inside `dispatch`). `tail = false`: this is a value
/// context, so any tail call is forced to completion by `force`.
pub fn apply_value(heap: &mut Heap, callee: Value, args: &[Value], genv: EnvId) -> LispResult {
    let argv: SmallVec<[Value; 4]> = args.iter().copied().collect();
    let step = dispatch(heap, callee, argv, false, genv)?;
    force(heap, step)
}

/// Apply `callee` through the active engine: the VM when enabled (a VM-eligible
/// callback runs compiled), the tree-walker under `BROOD_VM=0` (keeps the
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
// backend present. This is why the `#[cfg(not(feature = "jit"))]` stubs that used to live here
// are gone rather than moved — there is one definition now, and it always compiles.
mod jit_plan;

use jit_plan::{jit_ckpt_depth, jit_spill_reserve};

#[cfg(feature = "jit")]
mod jit_lower;

#[cfg(feature = "jit")]
pub(crate) use jit_lower::{
    jit_lower_arm, jit_lower_arm_hot, jit_lower_inlined_arm, take_mid_emit_reason,
    xcall_relower_enabled,
};

// Reached by `jit::cranelift`'s `JitBackend` tiering advisories, which is the only way the
// tiering glue is allowed to ask "have I demoted this fn off the register worker?" — it used to
// call straight into the Cranelift backend's i64 submodule (ADR-221's one remaining hole).
#[cfg(feature = "jit")]
pub(crate) use jit_lower::{arm_i64_eligible, arm_i64_too_deep, i64_mark_too_deep};

#[test]
fn test_inst_size() {
    // Not an assertion — just surfaces the IR `Inst` size in test output (a
    // regression in it shows up here). A non-zero size is guaranteed by the type.
    eprintln!("Inst size: {}", std::mem::size_of::<Inst>());
}
