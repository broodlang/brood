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
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::core::heap::{EnvRoot, Heap, VmCacheKey};
use crate::core::keywords as kw;
use crate::core::value::{
    self, BigIntId, ClosureId, EnvId, MapId, NativeId, PairId, RopeId, StrId, Symbol, Value, VecId,
};
use crate::error::{LispError, LispResult, Pos};

thread_local! {
    /// Per-thread engine override for the differential test harness (and any tool
    /// that wants to pin the engine): `Some(true)` forces the VM, `Some(false)` the
    /// tree-walker, `None` defers to the cached `BROOD_VM`/default choice. Checked
    /// before the cache so it wins; only a top-level form consults it, so the cost
    /// is negligible. See [`set_forced_engine`].
    static FORCED_ENGINE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force (or clear) the execution engine for the current thread, overriding
/// `BROOD_VM` and the build default — lets one process run a form through *both*
/// engines (the differential harness, `crates/lisp/tests/differential.rs`).
/// `Some(true)` = VM, `Some(false)` = tree-walker, `None` = default.
pub fn set_forced_engine(choice: Option<bool>) {
    FORCED_ENGINE.with(|c| c.set(choice));
}

/// Is the compiling VM enabled? A per-thread [`set_forced_engine`] override wins;
/// otherwise **the VM is the default engine** (ADR-076 Stage 3 cutover): every build
/// runs it unless `BROOD_VM` is set to a falsy value (`0`/`false`/`off`/`no`/empty),
/// which forces the tree-walker — the one-env-var escape hatch retained for at least
/// one release. Any other `BROOD_VM` value (or none) selects the VM. The env/default
/// choice is read once and cached; it can't change mid-run, but the override can.
pub fn vm_enabled() -> bool {
    if let Some(forced) = FORCED_ENGINE.with(|c| c.get()) {
        return forced;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    fn truthy(v: &str) -> bool {
        !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "off" | "no")
    }
    *ON.get_or_init(|| match std::env::var("BROOD_VM") {
        Ok(v) => truthy(&v), // explicit override (BROOD_VM=0 → tree-walker)
        Err(_) => true,      // VM is the default engine
    })
}

/// "This `Node::Call` has no call-site inline cache" — the callee isn't a free
/// global reference (ADR-096).
pub const NO_SITE: u32 = u32::MAX;

/// A core 2-ary numeric/comparison primitive the compiler inlines (perf #1). Each
/// maps to a Rust builtin (`%add`/`%sub`/`%mul`/`%lt`/`%le`/`%eq`); a
/// [`Node::Prim2`] runs the `(Int, Int)` case inline (a plain `i64` op — no call
/// frame, no `argv`, no native dispatch) and defers every other operand shape to
/// the real primitive so semantics (float coercion, overflow, structural `=`) stay
/// bit-identical. Comparisons spelt `>`/`>=` reach `%lt`/`%le` through the
/// passthrough arg-map (the operands are swapped), so they inline too.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimOp {
    Add,
    Sub,
    Mul,
    Lt,
    Le,
    Eq,
    // Integer division family (perf): `rem` is the native of that name; `%div`
    // backs `/`; `%quot` backs `quot` (truncating integer division). Inlining
    // these on `(Int, Int)` keeps tight integer loops (`collatz`, `mod`, hashing)
    // off the per-op native-dispatch path; non-int / edge cases defer to the
    // native so semantics + error messages stay identical (see `prim_apply`).
    Rem,
    Div,
    Quot,
    // `cons` (ADR-096): the list-building workhorse. Unlike the numeric ops it
    // allocates, so it's handled in the exec arm (which has the heap) rather
    // than `prim_apply`; it accepts any operands, so it never defers on shape.
    Cons,
    // `vector-ref` (perf): a dense-array O(1) indexed read. Like `Cons` it needs
    // the heap (a slab index), so it's handled in the exec arm; the `(Vector, Int)`
    // in-bounds case runs inline, and every other shape — non-vector, non-int, or
    // out-of-range — defers to the native `vector-ref` so its bounds error and
    // type errors stay bit-identical. This is the per-element cost of `matmul` and
    // (through the prelude `nth`) any indexed vector walk. Kept out of the JIT
    // subset for now (no cranelift lowering yet), so a JIT arm containing it
    // pre-bails rather than mis-lowering.
    VectorRef,
}

/// A core 1-ary sequence primitive the compiler inlines (ADR-096) — the list
/// iteration workhorses. The `Pair`/`Nil` cases run inline (a slab read — no
/// call frame, no dispatch); every other operand shape (vectors, ranges, the
/// canonical type errors) defers to the real native so semantics stay
/// bit-identical. Same epoch-guard discipline as [`PrimOp`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimOp1 {
    First,
    Rest,
}

impl PrimOp1 {
    fn from_native_name(name: &str) -> Option<PrimOp1> {
        Some(match name {
            "first" => PrimOp1::First,
            "rest" => PrimOp1::Rest,
            _ => return None,
        })
    }
}

impl PrimOp {
    fn from_native_name(name: &str) -> Option<PrimOp> {
        Some(match name {
            "%add" => PrimOp::Add,
            "%sub" => PrimOp::Sub,
            "%mul" => PrimOp::Mul,
            "%lt" => PrimOp::Lt,
            "%le" => PrimOp::Le,
            "rem" => PrimOp::Rem,
            "%div" => PrimOp::Div,
            "%quot" => PrimOp::Quot,
            "cons" => PrimOp::Cons,
            "vector-ref" => PrimOp::VectorRef,
            _ if name == kw::EQ_PRIM => PrimOp::Eq,
            _ => return None,
        })
    }
}

/// Which movable heap-handle kind a [`ConstVal::Handle`] carries — fixed at compile
/// time (a `Str` literal stays a `Str`), so only the handle's index bits move under
/// a RUNTIME compaction and the variant tells [`ConstVal`] how to re-wrap them.
#[derive(Clone, Copy)]
pub enum HandleKind {
    Str,
    BigInt,
    Pair,
    Vector,
    Map,
    Rope,
    Fn,
    Macro,
}

/// A constant baked into a compiled [`Node`] (a `Const` literal or a
/// `MakeClosure` `fn_rest`). Either a truly-immovable **atom** kept inline, or a
/// movable **heap handle** stored as `(kind, AtomicU64)` so the RUNTIME compactor
/// ([`Heap::runtime_collect`](crate::core::heap::Heap::runtime_collect)) can rewrite
/// it **in place** — the `Node` tree lives behind an `Arc` that `exec_node` walks by
/// `&Node`, so the `Arc` can't be swapped for a relocated copy; the handle bits must
/// move under the live reference. The atomic also keeps `ConstVal`/`Node`
/// `Send + Sync` (required because `Arc<CompiledArm>` is cached in a `Send` `Heap`).
/// Pre-ADR-076 every promoted constant was immovable, so this was a plain `Value`;
/// the compactor made promoted handles movable, which is the slab-OOB / corruption
/// bug this encoding fixes (`docs/known-issues.md`).
pub enum ConstVal {
    /// An inline scalar / interned symbol-or-keyword / `Nil` — never relocated.
    Atom(Value),
    /// A movable RUNTIME/PRELUDE heap handle, rewritable in place. PRELUDE handles
    /// never actually move (the flush is a no-op for them), but storing them here is
    /// harmless and keeps the compile-time split purely atom-vs-handle.
    Handle { kind: HandleKind, bits: AtomicU64 },
}

impl ConstVal {
    /// Build from a (already-`promote`d, immovable-or-RUNTIME) value: an atom stays
    /// inline; a heap handle is split into `(kind, bits)`.
    fn new(v: Value) -> ConstVal {
        match v {
            Value::Str(id) => ConstVal::Handle { kind: HandleKind::Str, bits: AtomicU64::new(id.0) },
            Value::BigInt(id) => {
                ConstVal::Handle { kind: HandleKind::BigInt, bits: AtomicU64::new(id.0) }
            }
            Value::Pair(id) => {
                ConstVal::Handle { kind: HandleKind::Pair, bits: AtomicU64::new(id.0) }
            }
            Value::Vector(id) => {
                ConstVal::Handle { kind: HandleKind::Vector, bits: AtomicU64::new(id.0) }
            }
            Value::Map(id) => ConstVal::Handle { kind: HandleKind::Map, bits: AtomicU64::new(id.0) },
            Value::Rope(id) => {
                ConstVal::Handle { kind: HandleKind::Rope, bits: AtomicU64::new(id.0) }
            }
            Value::Fn(id) => ConstVal::Handle { kind: HandleKind::Fn, bits: AtomicU64::new(id.0) },
            Value::Macro(id) => {
                ConstVal::Handle { kind: HandleKind::Macro, bits: AtomicU64::new(id.0) }
            }
            atom => ConstVal::Atom(atom),
        }
    }

    /// The current value (decoding a handle from its live bits).
    #[inline]
    pub fn load(&self) -> Value {
        match self {
            ConstVal::Atom(v) => *v,
            ConstVal::Handle { kind, bits } => {
                let b = bits.load(Ordering::Relaxed);
                match kind {
                    HandleKind::Str => Value::Str(StrId(b)),
                    HandleKind::BigInt => Value::BigInt(BigIntId(b)),
                    HandleKind::Pair => Value::Pair(PairId(b)),
                    HandleKind::Vector => Value::Vector(VecId(b)),
                    HandleKind::Map => Value::Map(MapId(b)),
                    HandleKind::Rope => Value::Rope(RopeId(b)),
                    HandleKind::Fn => Value::Fn(ClosureId(b)),
                    HandleKind::Macro => Value::Macro(ClosureId(b)),
                }
            }
        }
    }

    /// Rewrite a `Handle` in place through `f` (a `runtime_collect` flush). The kind
    /// is invariant under evacuation (a `Str` stays a `Str`), so only the bits change;
    /// an `Atom` is left untouched. Single-threaded (the owning process), so `Relaxed`
    /// suffices.
    fn rewrite(&self, f: &mut dyn FnMut(Value) -> Value) {
        if let ConstVal::Handle { bits, .. } = self {
            let new = f(self.load());
            let nb = match new {
                Value::Str(id) => id.0,
                Value::BigInt(id) => id.0,
                Value::Pair(id) => id.0,
                Value::Vector(id) => id.0,
                Value::Map(id) => id.0,
                Value::Rope(id) => id.0,
                Value::Fn(id) | Value::Macro(id) => id.0,
                // `f` (flush_rt_value) never changes the handle *kind*, so this is
                // unreachable; keep the old bits rather than panic if it ever does.
                _ => return,
            };
            bits.store(nb, Ordering::Relaxed);
        }
    }
}

/// A compiled IR node (ADR-076). Stage 1 vocabulary — the core forms a top-level
/// arithmetic/recursive body is built from. Anything outside this set makes the
/// whole closure ineligible (it runs on the tree-walker instead), so there is no
/// `Defer` node: a VM-run body is *fully* compiled, which is what lets `exec_node`
/// never need an `EnvId` for locals.
pub enum Node {
    /// A self-evaluating literal (number, bool, nil, string, keyword), as a
    /// [`ConstVal`]: an immovable atom inline, or a movable RUNTIME/PRELUDE heap
    /// handle as `(kind, AtomicU64)`. Construct only via [`const_node`], which
    /// `promote`s out of LOCAL first. The cached `Node` tree is an `Arc` off the GC
    /// root graph, so the collector never traces it — a LOCAL handle here would
    /// dangle (the use-after-GC bug fixed 2026-05-31), and a *RUNTIME* handle would
    /// dangle under a compaction unless rewritten in place, which is why the handle
    /// case is atomic (`runtime_collect` walks live arms and rewrites it).
    Const(ConstVal),
    /// A lexically-addressed local read: frame-slot `index` (depth 0 in the
    /// slice — only the callee's own params). Reads `root_at(frame_base + index)`.
    Local(usize),
    /// A free reference — resolved at run time through the global env (`env_get`,
    /// which also consults the dynamic-binding stack), exactly as the tree-walker
    /// resolves a non-local symbol. Used for capture-list reads (which resolve
    /// through a *captured* env, never the table) and call heads (whose
    /// resolution the call's own site IC caches); body value-position reads
    /// compile to [`Node::GlobalIc`] instead.
    Global(Symbol),
    /// A free reference in value position, with a **global-read inline cache**
    /// (ADR-096): when the enclosing frame resolves free names through the
    /// process global, the read validates `(sym, epoch)` against the per-process
    /// [`Heap::vm_global_ics`] entry and skips the `env_get` walk; otherwise (a
    /// captured-env frame, an epoch change, a dynamic symbol) it falls back to
    /// exactly the [`Node::Global`] path.
    GlobalIc { sym: Symbol, site: u32 },
    /// `(if cond then else)` — `cond` in value position, the branches inheriting
    /// the enclosing tail position.
    If(Box<Node>, Box<Node>, Box<Node>),
    /// `(do a b … z)` — all but the last for effect, the last in tail position.
    Do(Box<[Node]>),
    /// A vector literal `[a b …]` — evaluate each element (value position), then
    /// build a fresh vector. (A *quoted* vector `'[…]` is immutable data and compiles
    /// to a single immovable `Const` via `quote`, not this.)
    Vector(Box<[Node]>),
    /// A map literal `{k v …}` — evaluate each key and value (value position), then
    /// build a fresh map. (A *quoted* map is a `Const`, not this.)
    Map(Box<[(Node, Node)]>),
    /// A combination. `tail` marks a tail call (the trampoline reuses the frame
    /// instead of recursing — proper TCO). Non-tail calls recurse via [`vm_apply`].
    /// `pos` is the source `line:col` of this combination, captured at compile time
    /// (when the form's reader-recorded position is still live — see
    /// [`Heap::form_pos`]); an error from this call is tagged with it (innermost
    /// wins, like the tree-walker's `or_form_pos`) so VM diagnostics keep line/col.
    /// `None` for a promoted RUNTIME body (whose forms carry no recorded position —
    /// neither engine tags those).
    /// `site` is this call's **inline-cache id** (ADR-096) when the callee is a
    /// free global reference — an index into the per-process
    /// [`Heap::vm_call_ics`] table caching the site's last resolution (callee
    /// value + compiled arm + captured env, epoch-stamped). [`NO_SITE`] for a
    /// local/computed callee. Plain data: the entry lives in the *heap*, not the
    /// node, so the shared `Arc`'d tree stays immutable and the table can be
    /// dropped wholesale on a RUNTIME compaction.
    Call {
        callee: Box<Node>,
        args: Box<[Node]>,
        tail: bool,
        pos: Option<Pos>,
        site: u32,
    },
    /// A **direct `letrec` self-recursive tail call** (the self-call optimization).
    /// Emitted only for a tail call whose head is the closure's own self-name with
    /// exactly the arm's required arity (see [`Scope::self_call`]). Lowered to
    /// `Inst::SelfCall`, which hands the driver a `ChunkExit::SelfTail` for the
    /// *current* arm — no callee resolution, no `env_get` walk, no `vm_cache` lookup,
    /// no dispatch. Safe because a letrec binder is an immutable lexical slot (no
    /// `def`/late binding to observe). Only ever appears in tail position. `pos` tags
    /// an error from an argument's eval. The arm has no `&optional`/`&` rest (gated in
    /// `compile_arm`), so `args.len()` always equals the arm's frame arity.
    SelfCall {
        args: Box<[Node]>,
        pos: Option<Pos>,
    },
    /// `let`/`let*`/`letrec` (Stage 2a). Lexical scope is **flattened** into the
    /// single activation frame: each binder owns a frame slot (pre-allocated in
    /// `nslots`). Evaluate each `rhs` and write it into its `slot`
    /// (`set_root_at`), in order, then run `body` (tail-propagated). `let`/`let*`
    /// are sequential (a rhs sees earlier binders); `letrec` pre-allocates all
    /// slots (init `nil`) so a rhs can reference any binder.
    LetBind {
        binds: Box<[(usize, Node)]>,
        body: Box<Node>,
    },
    /// `(fn …)` evaluated *inside* a compiled body (Stage 2c). Builds
    /// a closure value that closes over a **flat snapshot** of the enclosing lexical
    /// environment: a fresh env frame (parent = the process global) is filled from
    /// `captures` — each `(name, src)` evaluates `src` in the current frame and
    /// binds it under `name` — and the closure captures that frame. Free vars in the
    /// new closure's body then resolve by name through it (`env_get`), exactly as a
    /// tree-walker-built closure resolves through its captured env chain (Brood
    /// bindings are immutable, so a value snapshot is equivalent to an env
    /// reference). `fn_rest` is the `(fn …)` form's cdr — an immovable RUNTIME
    /// sub-form parsed by [`crate::eval::make_closure`] at run time (reusing all the
    /// arity/optional/doc parsing).
    MakeClosure {
        /// The `(fn …)` form's cdr (an immovable RUNTIME sub-form), as a [`ConstVal`]
        /// so a runtime compaction rewrites it in place like a `Const` handle.
        fn_rest: ConstVal,
        captures: Box<[(Symbol, Node)]>,
        /// Direct `letrec` self-recursion: when this `(fn …)` is the RHS of a
        /// `letrec` binder it references, the closure must see *itself*. A value
        /// snapshot can't express that (the binder slot is still nil at build
        /// time), so the binder name rides here and the exec arm `env_define`s it
        /// to the freshly-built closure in the closure's own captured env —
        /// exactly the late-bind the tree-walker's `letrec` does. `None` for an
        /// ordinary (non-self-recursive) nested closure. A `Symbol` (interned
        /// `u32`), not a heap handle, so `rewrite_node` needn't touch it.
        self_name: Option<Symbol>,
    },
    /// An inlined 2-ary primitive (perf #1) — `(+ a b)`, `(< a b)`, `(= a b)`, etc.
    /// `a`/`b` are the operands in **source order**; `map` routes them to the
    /// underlying `%`-primitive's argument order (`[0,1]` for `+`/`<`, `[1,0]` for the
    /// `>`/`>=` wrappers that forward to `%lt`/`%le` with swapped args). The
    /// `(Int, Int)` case runs inline; any other operand shape — or a redefinition of
    /// the operator (detected by `guard` ≠ the current [`Heap::global_epoch`]) — falls
    /// back to a general call on `head`, so the language stays exactly as the
    /// tree-walker sees it. `guard` is the global epoch at which `head` was last
    /// confirmed to resolve to `op`; an [`AtomicU64`] (not a `Cell`) so the node stays
    /// `Send + Sync` and a migrating process's heap stays `Send`.
    /// `broot`: must operand `a`'s value be rooted across operand `b`'s eval
    /// (ADR-096)? `false` when `b` is a **safepoint-free leaf** (`Const` /
    /// `Local` / `Global` / `GlobalIc` — none can allocate, call, or collect),
    /// which is the overwhelmingly common shape in hot loops (`(+ acc n)`,
    /// `(< n 2)`): the whole inline path then runs with zero operand-stack
    /// traffic. The fallback (non-inline shapes, redefined operator) roots both
    /// operands before `dispatch` regardless.
    Prim2 {
        op: PrimOp,
        a: Box<Node>,
        b: Box<Node>,
        map: [u8; 2],
        head: Symbol,
        guard: AtomicU64,
        pos: Option<Pos>,
        broot: bool,
    },
    /// An inlined 1-ary sequence primitive (ADR-096) — `(first xs)` / `(rest xs)`.
    /// The `Pair`/`Nil` cases run inline; any other operand shape — or a
    /// redefinition of the operator — falls back to a general call on `head`,
    /// exactly like [`Node::Prim2`]'s guard discipline.
    Prim1 {
        op: PrimOp1,
        a: Box<Node>,
        head: Symbol,
        guard: AtomicU64,
        pos: Option<Pos>,
    },
}

/// The compiled counterpart of a [`ClosureArm`](crate::core::value::ClosureArm):
/// the frame layout and the compiled body. Cached per closure on the heap
/// (`Heap::vm_cache_*`). Immutable and `Send + Sync` (its `Node`s hold only
/// immovable handles + symbols + indices), so it lives behind an `Arc`.
///
/// Slot layout: required params `0..nrequired`, then `&optional` params
/// `nrequired..nrequired+noptional`, then the `&` rest slot (if any), then the
/// `let`/`letrec` binders — up to `nslots`. A missing optional takes its default:
/// `nil` (no eval) for a nil-default param, or the compiled `optional_defaults`
/// node (evaluated against the partially-built frame, so it can reference earlier
/// params) for a real default.
pub struct CompiledArm {
    /// Required params — `argv[0..nrequired]` fill slots `0..nrequired`. Selection
    /// guarantees `argc >= nrequired`, so they're always present.
    pub nrequired: usize,
    /// Count of `&optional` params. A provided arg fills the slot; a missing one
    /// takes its default (see `optional_defaults`).
    pub noptional: usize,
    /// Per-optional default, indexed `0..noptional`: `None` = nil-default (just push
    /// `nil`), `Some(node)` = a real default form, compiled in a scope where the
    /// required params and *earlier* optionals are bound. Evaluated by `push_frame`
    /// only when the optional's arg is missing — left-to-right, so a later default
    /// sees earlier ones (matching the tree-walker).
    pub optional_defaults: Box<[Option<Node>]>,
    /// `&` rest param's slot, if any: collects `argv[nrequired+noptional..]` into a
    /// fresh list.
    pub rest_slot: Option<usize>,
    /// Total frame slots (params + optionals + rest + `let`/`letrec` binders).
    pub nslots: usize,
    pub body: Node,
    /// The body compiled to flat **bytecode** (`Chunk`). [`vm_run_bc`] runs this — the
    /// sole VM executor since ADR-100 Stage 5. `compile_arm` always fills it (every
    /// `Node` shape lowers via [`compile_chunk`]); it's `Option` only for the synthetic
    /// chunk-less arms (`run`'s top-level wrapper, tests) that never reach `vm_run_bc`.
    /// `body` is retained as the lowering *source* (and the tree-walker's reference);
    /// the differential test enforces that bytecode matches it exactly.
    pub chunk: Option<Chunk>,
    /// True when the body or any optional default contains a `Node::Const` with a
    /// movable RUNTIME handle (`ConstVal::Handle`), or a `Node::MakeClosure` (whose
    /// `fn_rest` is always a RUNTIME Pair). Arms without RUNTIME handles do not need
    /// to be registered in `Heap::live_vm_arms` because a `runtime_collect` pass has
    /// nothing to rewrite in their node tree — skipping the registration avoids an
    /// `Arc::clone` on the hot call path, removing cross-worker cache-line contention
    /// on the shared refcount when many processes call the same function in parallel.
    pub has_runtime_handles: bool,
    /// JIT tiering (ADR-101, feature "jit"): native code pointer for this arm —
    /// null = not compiled, `1` (BAILED) = tried & out-of-subset, else a callable
    /// `extern "C" fn(*mut Heap, base) -> i64`. `jit_calls` counts invocations to
    /// trigger compilation past a threshold. Shared across `Arc<CompiledArm>` clones.
    pub jit_code: std::sync::atomic::AtomicPtr<u8>,
    pub jit_calls: std::sync::atomic::AtomicU32,
    /// The [`Heap::global_epoch`] at which this arm was last compiled to native code —
    /// the inline-cache epoch guard (ADR-096 §4.A) for the JIT'd arm. The lowered code
    /// inlines arithmetic operators (`+`/`<`/…) as raw machine ops, valid only while
    /// those globals still resolve to their native primitives. A `def` rebinding any
    /// global bumps `global_epoch`; [`jit_tier`] compares it against this before each
    /// native entry, and on a mismatch invalidates the arm so it re-validates its
    /// operators and re-tiers (or bails if one was genuinely redefined). A JIT'd arm
    /// never evaluates Brood, so no `def` can occur *during* a native run — checking
    /// once per activation (not per loop iteration) is sufficient and keeps hot loops
    /// fast. Only meaningful once `jit_code` holds a real pointer.
    pub compile_epoch: std::sync::atomic::AtomicU64,
}

/// One arm of a closure: its arity shape plus the compiled body **if** it was
/// VM-eligible. Every arm is recorded — even ones that defer — so [`arm_for`]
/// reproduces [`Closure::select_arm`](crate::core::value::Closure::select_arm)
/// *exactly* (picks the same arm) before checking whether that arm can run on the
/// VM. Without the full table a variadic arm (which accepts a *range* of arities)
/// could be picked where the tree-walker would pick an overlapping fixed arm — a
/// silent wrong-arm miscompile.
struct ArmSpec {
    nrequired: usize,
    noptional: usize,
    has_rest: bool,
    compiled: Option<Arc<CompiledArm>>,
}

impl ArmSpec {
    fn accepts(&self, argc: usize) -> bool {
        argc >= self.nrequired && (self.has_rest || argc <= self.nrequired + self.noptional)
    }
}

/// A compiled closure: every arm's arity shape + (if VM-eligible) its compiled body.
pub struct CompiledClosure {
    arms: Vec<ArmSpec>,
}

impl CompiledClosure {
    /// The compiled arm to run for `argc`, or `None` to defer to the tree-walker.
    /// Mirrors `Closure::select_arm`: among accepting arms, prefer a fixed (no-rest)
    /// arm, then the most required params; ties resolve to the later arm (Rust's
    /// `max_by_key`, same as eval). Returns the winner's compiled body iff it was
    /// VM-eligible — otherwise `None`, so the tree-walker runs the *same* arm.
    pub(crate) fn arm_for(&self, argc: usize) -> Option<&Arc<CompiledArm>> {
        let winner = self
            .arms
            .iter()
            .filter(|a| a.accepts(argc))
            .max_by_key(|a| (!a.has_rest, a.nrequired))?;
        winner.compiled.as_ref()
    }
}

/// The result of `dispatch` (and the value-position `exec_call`/`exec_value` path):
/// a finished value, or a *tail call* the caller continues. `Tail` carries a resolved
/// VM arm un-run, so a tail call reuses a frame (in [`vm_run_bc`]) or is forced (in
/// value position, via [`force`]). (`exec_value`/`exec_call` survive for `push_frame`'s
/// `&optional` defaults and top-level `run`; the bytecode driver uses [`ChunkExit`].)
enum Step {
    Done(Value),
    Tail {
        compiled: Arc<CompiledArm>,
        args: SmallVec<[Value; 4]>,
        /// The tail callee's own captured env — switched to so the next arm resolves
        /// its free vars in *its* scope (Stage 2c: a tail call can cross into a
        /// closure with a different captured env).
        genv: EnvId,
    },
}

/// What running a bytecode [`Chunk`] frame yields back to the explicit-frame driver
/// ([`vm_run_bc`], ADR-100 Stage 4). Unlike [`Step`] (which the `Node` trampoline
/// uses), this adds `Call` — a **non-tail** call to a chunked VM arm, which the
/// driver turns into a **frame push** rather than native recursion. `Tail`/`SelfTail`
/// reuse the current frame (TCO); `Done` pops it. A non-tail call to a native or a
/// tree-walked arm is already executed inside `exec_chunk` (via `dispatch`) and
/// surfaces as an ordinary pushed value, never as `Call`.
enum ChunkExit {
    Done(Value),
    Tail { arm: Arc<CompiledArm>, args: SmallVec<[Value; 4]>, genv: EnvId },
    Call { arm: Arc<CompiledArm>, args: SmallVec<[Value; 4]>, genv: EnvId },
    /// A clean `receive` on an empty mailbox raised `Control::Suspend` through the
    /// `%receive` native (state-capture path, ADR-100 §8). `exec_chunk` rewound `ip`
    /// so re-entry re-runs the suspending `Inst::Call`, leaving the callee + args on
    /// the operand stack untouched; the driver ([`vm_run_bc`]) captures the whole
    /// frame stack as a [`Suspended`] and returns it to the scheduler to park. Produced
    /// only by a clean top-level `receive` (a native-nested one blocks the worker, §7.4).
    Suspend { deadline: Option<std::time::Instant> },
    /// Hard `:kill` was pending at the inline `SelfCall` safepoint. The frame is already
    /// reset (ip=0, new args in slots); the driver retires the process.
    Killed,
    /// Reduction budget exhausted at the inline `SelfCall` safepoint (capture mode). The
    /// frame is already reset (ip=0, new args in slots); the driver captures as usual.
    Preempt,
    /// Back-edge tiering (`--features jit`): a hot self-tail loop periodically exits the
    /// inline `SelfCall` loop so the driver can tier it. The frame is already reset
    /// (ip=0, the iteration's args in slots), so the driver just re-enters the *same* arm
    /// at ip 0 with `try_jit` set — counting toward the threshold while untried, and
    /// running the native code (which loops internally) once it's installed. Without this
    /// a self-tail loop is one arm entry and never reaches the per-entry tier threshold.
    SelfTail,
}

// ===================== compiler (form → Node) =====================

/// Compile-time lexical scope: `let`/`letrec`/param binders flattened into one
/// activation frame (ADR-076 Stage 2a). Each in-scope name maps to a frame slot;
/// `next` is the next free slot and `max` is the high-water mark (= the arm's
/// `nslots`). Shadowing: `lookup` scans newest-first. `bind` claims a slot;
/// `restore` pops a scope's binders (reusing their slots — safe, the bindings are
/// dead once out of scope).
///
/// `enclosing` (Stage 2c) holds the names lexically visible from *outer* closures —
/// derived once, by walking this closure's captured env, in [`compile_closure`].
/// They aren't frame slots (they live in the captured env, reached by name via
/// `Node::Global`), but a nested `(fn …)` must still snapshot them when it captures
/// the lexical environment, so the compiler has to know which free names are
/// enclosing *lexicals* (snapshot) vs true globals (resolved live, never snapshot).
///
/// `unsafe_slots` marks frame slots that are **not yet finalized** — the binders of
/// a `letrec` whose rhs are still being compiled. A `(fn …)` that would capture one
/// can't be VM-built (a value snapshot can't express letrec's recursive
/// late-binding), so it defers to the tree-walker.
struct Scope {
    names: Vec<(Symbol, usize)>,
    next: usize,
    max: usize,
    enclosing: Vec<Symbol>,
    unsafe_slots: Vec<usize>,
    /// While compiling a `letrec` binder whose RHS is *directly* a `(fn …)`, the
    /// slot of that binder — so a nested closure capturing it recognises the
    /// **direct self-recursion** case and binds its own name to itself at build
    /// time (see [`compile_captures`]) rather than deferring. `None` everywhere
    /// else, so an ordinary capture of an in-progress letrec binder (mutual
    /// recursion) still defers.
    letrec_self: Option<usize>,
    /// `(self-name, arity)` when this arm is a plain fixed-arity local recursive
    /// helper (a `letrec` binder bound to itself — see [`compile_closure`]). A
    /// **tail** call to `self-name` with exactly `arity` args compiles to a
    /// [`Node::SelfCall`] that re-invokes the current arm directly, skipping the
    /// env-resolve + dispatch the generic call path pays per iteration. `None`
    /// for an ordinary closure (and unset while compiling a nested `(fn …)`, which
    /// gets its own scope).
    self_call: Option<(Symbol, usize)>,
}

impl Scope {
    fn new() -> Self {
        Scope {
            names: Vec::new(),
            next: 0,
            max: 0,
            enclosing: Vec::new(),
            unsafe_slots: Vec::new(),
            letrec_self: None,
            self_call: None,
        }
    }
    fn with_params(params: &[Symbol]) -> Self {
        let mut s = Scope::new();
        for &p in params {
            s.bind(p);
        }
        s
    }
    /// As [`with_params`](Self::with_params) but seeded with the enclosing lexical
    /// names a nested closure closes over (Stage 2c).
    fn with_params_enclosing(params: &[Symbol], enclosing: Vec<Symbol>) -> Self {
        let mut s = Scope::with_params(params);
        s.enclosing = enclosing;
        s
    }
    fn lookup(&self, sym: Symbol) -> Option<usize> {
        self.names.iter().rev().find(|(n, _)| *n == sym).map(|&(_, slot)| slot)
    }
    fn bind(&mut self, sym: Symbol) -> usize {
        let slot = self.next;
        self.next += 1;
        if self.next > self.max {
            self.max = self.next;
        }
        self.names.push((sym, slot));
        slot
    }
    fn is_unsafe(&self, slot: usize) -> bool {
        self.unsafe_slots.contains(&slot)
    }
    /// Snapshot for scope exit: `(names-len, next-slot)`.
    fn mark(&self) -> (usize, usize) {
        (self.names.len(), self.next)
    }
    fn restore(&mut self, (names_len, next): (usize, usize)) {
        self.names.truncate(names_len);
        self.next = next;
    }
}

/// Extract a binding form's elements (`[n1, v1, n2, v2, …]`) from either a list
/// `(n1 v1 …)` or a vector `[n1 v1 …]` (both accepted in Brood binding position),
/// or `None` if it isn't one.
fn binding_elems(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form {
        Value::Nil => Some(Vec::new()),
        Value::Vector(vid) => Some(heap.vector(vid).to_vec()),
        Value::Pair(_) => heap.list_to_vec(form).ok(),
        _ => None,
    }
}

/// Compile a body (a `do`-like sequence): all but the last for effect, the last
/// in `tail` position. Empty → `nil`. A single form returns that node directly.
fn compile_body(heap: &Heap, forms: &[Value], scope: &mut Scope, tail: bool) -> Option<Node> {
    if forms.is_empty() {
        return Some(const_node(heap, Value::Nil));
    }
    let n = forms.len();
    let mut nodes = Vec::with_capacity(n);
    for (i, &f) in forms.iter().enumerate() {
        nodes.push(compile_node(heap, f, scope, tail && i + 1 == n)?);
    }
    Some(if nodes.len() == 1 {
        nodes.pop().unwrap()
    } else {
        Node::Do(nodes.into_boxed_slice())
    })
}

/// Compile a `let`/`let*` (sequential) or `letrec` form to a [`Node::LetBind`], or
/// `None` (defer) if a binder isn't a plain symbol or anything fails. Pushes the
/// binders into `scope` for the body, then restores on the way out.
fn compile_let(heap: &Heap, items: &[Value], scope: &mut Scope, tail: bool, rec: bool) -> Option<Node> {
    if items.len() < 2 {
        return None;
    }
    let elems = binding_elems(heap, items[1])?;
    if elems.len() % 2 != 0 {
        return None;
    }
    let saved = scope.mark();
    let unsafe_saved = scope.unsafe_slots.len();
    let result = (|| {
        let mut binds: Vec<(usize, Node)> = Vec::with_capacity(elems.len() / 2);
        if rec {
            // letrec: pre-allocate every binder's slot (init nil) so a rhs can
            // reference any binder; then compile the rhs in order.
            let mut slots = Vec::with_capacity(elems.len() / 2);
            for pair in elems.chunks_exact(2) {
                match pair[0] {
                    Value::Sym(s) => slots.push(scope.bind(s)),
                    _ => return None,
                }
            }
            // While compiling the rhs, the letrec slots aren't yet filled — a
            // nested `(fn …)` capturing one would snapshot `nil` (a value snapshot
            // can't do letrec's recursive late-binding), so mark them unsafe to
            // capture; they become safe once we reach the body (all rhs done).
            scope.unsafe_slots.extend_from_slice(&slots);
            for (pair, &slot) in elems.chunks_exact(2).zip(slots.iter()) {
                // A binder whose RHS is *directly* a `(fn …)` enables the direct
                // self-recursion path: `compile_captures` may bind that name to the
                // built closure instead of deferring. Set it only for the fn-RHS
                // case (and only across this one `compile_node`, which consumes it
                // without recursing first) so a fn nested elsewhere in a non-fn RHS
                // — e.g. `(g (fn …))`, whose binder value is the *call* result, not
                // the fn — never misclaims self-recursion.
                let saved_self = scope.letrec_self;
                scope.letrec_self = is_fn_form(heap, pair[1]).then_some(slot);
                let rhs = compile_node(heap, pair[1], scope, false);
                scope.letrec_self = saved_self;
                binds.push((slot, rhs?));
            }
            scope.unsafe_slots.truncate(unsafe_saved);
        } else {
            // let/let*: sequential — a rhs sees only earlier binders.
            for pair in elems.chunks_exact(2) {
                let name = match pair[0] {
                    Value::Sym(s) => s,
                    _ => return None,
                };
                if is_fn_form(heap, pair[1]) {
                    // A fn-valued binder: pre-allocate the slot before compiling
                    // the RHS so compile_captures can route a self-reference through
                    // self_name, producing a structural env cycle. The tree-walker's
                    // let captures the scope frame by reference — env_define adds f
                    // to it after the closure is built — so the TW closure IS
                    // structurally self-referential (send rejects it). Without this
                    // path the VM closure gets env=global (no frame, no cycle), send
                    // accepts it, and the two engines diverge.
                    let slot = scope.bind(name);
                    let unsafe_before = scope.unsafe_slots.len();
                    scope.unsafe_slots.push(slot);
                    let saved_self = scope.letrec_self;
                    scope.letrec_self = Some(slot);
                    let rhs = compile_node(heap, pair[1], scope, false);
                    scope.letrec_self = saved_self;
                    scope.unsafe_slots.truncate(unsafe_before);
                    binds.push((slot, rhs?));
                } else {
                    let rhs = compile_node(heap, pair[1], scope, false)?;
                    let slot = scope.bind(name);
                    binds.push((slot, rhs));
                }
            }
        }
        let body = compile_body(heap, &items[2..], scope, tail)?;
        Some(Node::LetBind {
            binds: binds.into_boxed_slice(),
            body: Box::new(body),
        })
    })();
    scope.restore(saved);
    scope.unsafe_slots.truncate(unsafe_saved); // also undo on the early-`None` paths
    result
}

/// Is `fn_rest` (a `(fn …)` form's cdr) safe to bake into a cached [`Node`]? It
/// must be an immovable handle: the body the closure will parse from it lives there
/// for the life of the compiled body, so a movable LOCAL form (e.g. a top-level
/// freshly-read or quasiquote-built `fn`) would dangle after a collection. Such a
/// form simply defers to the tree-walker.
fn fn_rest_is_stable(v: Value) -> bool {
    match v {
        Value::Pair(p) => p.region() != value::LOCAL,
        Value::Nil => true, // `(fn)` — degenerate, but stable
        _ => false,
    }
}

/// Bake a self-evaluating literal into a [`Node::Const`], guaranteeing the embedded
/// value is **immovable**. A compiled `Node` tree lives in an `Arc` *off* the GC
/// root graph, so the collector neither traces nor relocates a handle inside it: a
/// LOCAL heap handle (e.g. a freshly-read `Value::Str` in a top-level form, which
/// `run()` never `promote`s) would dangle after a collection *during that form's own
/// evaluation* and be read as freed/moved memory by a later sub-form — a
/// use-after-GC (the bug fixed 2026-05-31; it's why `(do (doc-search …) "lit")`
/// crashed under GC stress). `promote` freezes a LOCAL string/heap literal into the
/// immovable RUNTIME code region (the same freeze a `def`/`defn` body's literals
/// get) and is a no-op for inline atoms, interned keywords, and already-shared
/// PRELUDE/RUNTIME handles. **Route every literal `Const` through here** — the
/// invariant is easy to bypass with a bare `Node::Const(form)` (which is exactly how
/// the `Value::Str` arm originally introduced the bug); the sibling `MakeClosure`
/// path guards the same hazard via [`fn_rest_is_stable`] (deferring instead of
/// freezing).
fn const_node(heap: &Heap, v: Value) -> Node {
    let frozen = heap.promote(v);
    debug_assert!(
        value_is_immovable(frozen),
        "Node::Const must hold an immovable handle (the Arc'd AST is off the GC root \
         graph and can't relocate it); promote left a movable {frozen:?}"
    );
    Node::Const(ConstVal::new(frozen))
}

/// A `Value` carrying no relocatable LOCAL heap handle — an inline scalar, an
/// interned symbol/keyword, or a PRELUDE/RUNTIME handle. The postcondition
/// [`const_node`] asserts; the handle kinds mirror those [`Heap::promote`] copies
/// out of LOCAL.
///
/// Not `#[cfg(debug_assertions)]`: `debug_assert!` still *compiles* its condition
/// in release (it expands to `if cfg!(debug_assertions) { assert!(…) }` — a dead
/// branch, but the call must resolve), so gating this out breaks the release
/// build. In release the optimizer drops the never-taken branch.
fn value_is_immovable(v: Value) -> bool {
    match v {
        Value::Str(id) => id.region() != value::LOCAL,
        Value::BigInt(id) => id.region() != value::LOCAL,
        Value::Pair(id) => id.region() != value::LOCAL,
        Value::Vector(id) => id.region() != value::LOCAL,
        Value::Map(id) => id.region() != value::LOCAL,
        Value::Rope(id) => id.region() != value::LOCAL,
        Value::Fn(id) | Value::Macro(id) => id.region() != value::LOCAL,
        // A `Range` is a `VecId` and a `Transient` a `TransientId` — both movable when
        // LOCAL, so they must be checked too (else this tripwire would wrongly pass a
        // movable LOCAL `Range`/`Transient` baked into a Const).
        Value::Range(id) => id.region() != value::LOCAL,
        Value::Transient(id) => id.region() != value::LOCAL,
        // Inline scalars (Int/Float/Bool/Nil), interned Sym/Keyword, and the
        // remaining handle-free kinds carry nothing the GC relocates.
        _ => true,
    }
}

/// The capture list for a nested `(fn …)` — the enclosing lexical environment it
/// closes over, snapshotted by value (Brood bindings are immutable, so this is
/// equivalent to capturing the env by reference). Each current-frame lexical maps
/// to a `Node::Local` slot read; each name inherited from an *outer* closure maps
/// to a `Node::Global` read through the current captured env. True globals are
/// **not** captured — they resolve live (late-bound) through the new closure's
/// frame parent. Returns `None` (defer) if a capture would read a not-yet-finalized
/// `letrec` slot, which a value snapshot can't express.
fn compile_captures(scope: &Scope) -> Option<(Vec<(Symbol, Node)>, Option<Symbol>)> {
    let mut seen: Vec<Symbol> = Vec::new();
    let mut caps: Vec<(Symbol, Node)> = Vec::new();
    let mut self_name: Option<Symbol> = None;
    // Current-frame lexicals, innermost binding first (so shadowing wins).
    for &(sym, slot) in scope.names.iter().rev() {
        if seen.contains(&sym) {
            continue;
        }
        seen.push(sym);
        if scope.is_unsafe(slot) {
            // An in-progress `letrec` binder. If it's the very binder this `(fn …)`
            // is the RHS of (direct self-recursion — `scope.letrec_self`), the
            // closure references *itself*: don't snapshot the slot (still nil),
            // record the name for the exec arm to bind to the built closure (the
            // tree-walker's late-bind). Any *other* unsafe binder is mutual
            // recursion a value snapshot can't express — defer the whole closure.
            if Some(slot) == scope.letrec_self {
                self_name = Some(sym);
                continue;
            }
            return None;
        }
        caps.push((sym, Node::Local(slot)));
    }
    // Lexicals inherited from outer closures — read by name from the current env.
    for &sym in scope.enclosing.iter() {
        if seen.contains(&sym) {
            continue;
        }
        seen.push(sym);
        caps.push((sym, Node::Global(sym)));
    }
    Some((caps, self_name))
}

/// Is `form` *directly* a `(fn …)` combination? Used by `letrec` to
/// gate the direct self-recursion path (only a fn-valued binder can be its own
/// recursive callee).
fn is_fn_form(heap: &Heap, form: Value) -> bool {
    if let Value::Pair(p) = form {
        if let Value::Sym(h) = heap.pair(p).0 {
            return value::symbol_is(h, kw::FN);
        }
    }
    false
}

/// Compile a `(fn …)` evaluated inside a compiled body to a
/// [`Node::MakeClosure`] (Stage 2c), or `None` (defer) if it can't be VM-built. The
/// closure's *body* is not compiled here — it's compiled lazily by [`compiled_for`]
/// when the closure is first called, keyed by its RUNTIME body handle.
fn compile_make_closure(heap: &Heap, form: Value, scope: &Scope) -> Option<Node> {
    // Post-macroexpand a pattern-param / multi-clause `fn` is already lowered to
    // `match*`; a `fn` reaching here should be plain. Defer defensively otherwise.
    if crate::eval::macros::fn_needs_lowering(heap, form) {
        return None;
    }
    let fn_rest = match form {
        Value::Pair(p) => heap.pair(p).1,
        _ => return None,
    };
    if !fn_rest_is_stable(fn_rest) {
        return None;
    }
    let (captures, self_name) = compile_captures(scope)?;
    Some(Node::MakeClosure {
        fn_rest: ConstVal::new(fn_rest),
        captures: captures.into_boxed_slice(),
        self_name,
    })
}

/// Resolve a 2-arg call head `h` to a core inlinable [`PrimOp`] plus the arg-map
/// that routes the call's operands to the underlying `%`-primitive (perf #1), or
/// `None` if `h` isn't (currently) one. `h` may bind the primitive **directly** (a
/// `Value::Native`, map `[0,1]`) or — the common case — be a prelude wrapper
/// (`+`/`<`/`>`…) whose 2-arg arm is a pure passthrough to the `%`-native; that one
/// hop is followed via [`crate::eval::passthrough_arm`], inheriting its arg-map so
/// the `>`/`>=` wrappers (which forward to `%lt`/`%le` with swapped args) inline
/// too. Read against the live global env, so a user who has redefined the operator
/// away from the builtin simply doesn't match (and the call compiles normally).
fn resolve_prim(heap: &Heap, h: Symbol) -> Option<(PrimOp, [usize; 2])> {
    let v = heap.env_get(heap.global(), h)?;
    // The canonical prelude `nth`: `(nth v i)` on a vector is a bounds-checked
    // slab read, so inline it as `VectorRef` — the call's own `head` (`nth`) drives
    // the deopt, so the list / out-of-range / explicit-default cases dispatch the
    // real `nth` unchanged. Guarded by region: a user `(def nth …)` rebinds `nth`
    // to a non-PRELUDE closure, which fails this check, so the inline cleanly
    // disables (and the same epoch guard that protects every other inlined prim
    // re-validates here on a redefinition).
    if value::symbol_is(h, "nth") {
        return match v {
            Value::Fn(id) if id.region() == crate::core::value::PRELUDE => {
                Some((PrimOp::VectorRef, [0, 1]))
            }
            _ => None,
        };
    }
    let (nid, map): (NativeId, [usize; 2]) = match v {
        Value::Native(id) => (id, [0, 1]),
        Value::Fn(id) => {
            let (inner_head, m) = crate::eval::passthrough_arm(heap, id, 2)?;
            if m.len() != 2 {
                return None;
            }
            let inner = match inner_head {
                Value::Sym(s) => heap.env_get(heap.global(), s)?,
                other => other,
            };
            match inner {
                Value::Native(id) => (id, [m[0], m[1]]),
                _ => return None,
            }
        }
        _ => return None,
    };
    let op = PrimOp::from_native_name(&heap.native(nid).name)?;
    Some((op, map))
}

/// Resolve a fold *reducer value* `f` to an inlinable associative [`PrimOp`]
/// (`+`/`*` only — the cases a counted range fold can run without a per-element
/// `apply`). The sibling of [`resolve_prim`], but it starts from the reducer
/// value `reduce`/`fold` actually hold (a `Native`, or the prelude `+`/`*`
/// closure) rather than a head symbol, and accepts only the in-order arg-map
/// `[0, 1]` so a swapped wrapper (`>` → `%lt`) can never be misread as a fold.
/// Read against the live global env, so a redefined `+` simply doesn't match.
pub fn reduce_prim_op(heap: &Heap, f: Value) -> Option<PrimOp> {
    let nid = match f {
        Value::Native(id) => id,
        Value::Fn(id) => {
            let (inner_head, m) = crate::eval::passthrough_arm(heap, id, 2)?;
            if m.len() != 2 || m[0] != 0 || m[1] != 1 {
                return None;
            }
            match inner_head {
                Value::Sym(s) => match heap.env_get(heap.global(), s)? {
                    Value::Native(id) => id,
                    _ => return None,
                },
                Value::Native(id) => id,
                _ => return None,
            }
        }
        _ => return None,
    };
    let op = PrimOp::from_native_name(&heap.native(nid).name)?;
    matches!(op, PrimOp::Add | PrimOp::Mul).then_some(op)
}

/// Apply an inlinable 2-ary [`PrimOp`] to a single `(x, y)` pair from outside the
/// bytecode loop (the `range_reduce` fast path). `Ok(Some(v))` when handled inline;
/// `Ok(None)` to defer to the real reducer (i64 overflow → BigInt, or a
/// Float/BigInt operand the scalar path doesn't own) so results stay bit-identical.
pub fn prim_apply_step(op: PrimOp, x: Value, y: Value) -> Result<Option<Value>, LispError> {
    prim_apply(op, x, y)
}

/// Resolve a 1-arg call head `h` to a core inlinable [`PrimOp1`], or `None` if it
/// isn't one. Unlike [`resolve_prim`] there's no passthrough hop: `first`/`rest`
/// are bound directly to their natives. Read against the live global env, so a
/// redefinition simply doesn't match.
fn resolve_prim1(heap: &Heap, h: Symbol) -> Option<PrimOp1> {
    match heap.env_get(heap.global(), h)? {
        Value::Native(id) => PrimOp1::from_native_name(&heap.native(id).name),
        _ => None,
    }
}

/// Compile an already-expanded, already-resolved `form` against the lexical
/// `scope`. `tail` is whether this form is in tail position. Returns `None` when
/// the form uses anything outside the VM's vocabulary (the caller then defers the
/// whole closure to the tree-walker).
fn compile_node(heap: &Heap, form: Value, scope: &mut Scope, tail: bool) -> Option<Node> {
    match form {
        // Self-evaluating literals. `const_node` freezes any embedded heap handle
        // into the immovable RUNTIME region — load-bearing for `Value::Str` (a LOCAL
        // string baked raw into the off-GC-graph AST is the use-after-GC class; see
        // `const_node`), a no-op for the inline/interned atoms.
        Value::Int(_)
        | Value::BigInt(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Nil
        | Value::Str(_)
        | Value::Keyword(_) => Some(const_node(heap, form)),

        // A name: a local frame slot if bound, else a global reference with a
        // read IC (ADR-096).
        Value::Sym(s) => match scope.lookup(s) {
            Some(slot) => Some(Node::Local(slot)),
            None => Some(Node::GlobalIc { sym: s, site: heap.vm_gsite_alloc() }),
        },

        // A combination — a special form we handle (`if`/`do`) or a function call.
        Value::Pair(_) => {
            let items = heap.list_to_vec(form).ok()?;
            let head = *items.first()?;
            if let Value::Sym(h) = head {
                if value::symbol_is(h, kw::IF) {
                    // (if cond then) or (if cond then else)
                    if items.len() != 3 && items.len() != 4 {
                        return None;
                    }
                    let cond = compile_node(heap, items[1], scope, false)?;
                    let then = compile_node(heap, items[2], scope, tail)?;
                    let els = match items.get(3) {
                        Some(&e) => compile_node(heap, e, scope, tail)?,
                        None => const_node(heap, Value::Nil),
                    };
                    return Some(Node::If(Box::new(cond), Box::new(then), Box::new(els)));
                }
                if value::symbol_is(h, kw::DO) {
                    return compile_body(heap, &items[1..], scope, tail);
                }
                if value::symbol_is(h, kw::QUOTE) {
                    // Quoted data → one immovable `Const` (`const_node` promotes the
                    // datum into the shared RUNTIME region). Unblocks any body that
                    // quotes data — notably match*'s no-match arm,
                    // `(throw [:match-error (quote :ctx) m (quote pats)])`, which had
                    // been forcing every non-total `match` / pattern-dispatch `fn`
                    // onto the tree-walker.
                    //
                    // `(quote a b)` is malformed — the tree-walker rejects it with an
                    // arity error. Defer the whole closure so both engines agree;
                    // compiling only `a` here would silently drop the tail.
                    if items.len() != 2 {
                        return None;
                    }
                    return Some(const_node(heap, items[1]));
                }
                // `let` is sequential; `letrec` pre-allocates all slots.
                if value::symbol_is(h, kw::LET) {
                    return compile_let(heap, &items, scope, tail, false);
                }
                if value::symbol_is(h, kw::LETREC) {
                    return compile_let(heap, &items, scope, tail, true);
                }
                // `(fn …)` inside a compiled body (Stage 2c): build a closure
                // capturing a flat snapshot of the enclosing lexicals.
                if value::symbol_is(h, kw::FN) {
                    return compile_make_closure(heap, form, scope);
                }
                // Any *other* special form (`def`/`quasiquote`/`binding`) is outside
                // the VM's vocabulary — defer the whole closure to the tree-walker.
                // (`if`/`do`/`let`/`letrec`/`fn`/`quote` are handled above;
                // `defmacro`/`and`/`or`/`match`/`match*` aren't special forms — they're
                // macros, already expanded to these core forms by the compile pass.)
                if crate::eval::is_special_form(h) {
                    return None;
                }
                // A call whose head is an (as-yet-)**unexpanded macro**. The compile
                // pass (`macroexpand_all`) expands macros that are already defined,
                // but a macro **defined after** the closure — a forward reference, or
                // a prelude fn using a macro defined later in the prelude (e.g.
                // `sleep` calls `receive`) — can't be expanded then, so it survives
                // verbatim in the stored body. The VM only runs *expanded* forms (and
                // would otherwise compile the macro's argument syntax — pin patterns,
                // `~`-unquotes — as ordinary calls), so defer the whole closure to the
                // tree-walker, which expands macros lazily at eval time. Macros live
                // in the global table; a locally-bound head can't be one.
                if scope.lookup(h).is_none()
                    && crate::eval::macros::macro_head_id(heap, heap.global(), h).is_some()
                {
                    return None;
                }
                // Primitive inlining (perf #1): a 2-arg call whose head is a free
                // (non-shadowed) reference resolving — through at most one passthrough
                // hop — to a core numeric/comparison primitive compiles to a
                // `Node::Prim2`. The `(Int, Int)` case then runs inline in `exec_node`,
                // skipping the global lookup, passthrough redirect, `compiled_for`
                // cache hit, arity check, and native dispatch the generic call path
                // pays per operator per iteration. Guarded by the global epoch so a
                // redefinition of the operator cleanly falls back (see `Node::Prim2`).
                // 1-ary sequence primitives (`first`/`rest`) inline the same way
                // (ADR-096) — the list-iteration workhorses of every prelude
                // sequence fn.
                if items.len() == 2 && scope.lookup(h).is_none() {
                    if let Some(op) = resolve_prim1(heap, h) {
                        let a = compile_node(heap, items[1], scope, false)?;
                        return Some(Node::Prim1 {
                            op,
                            a: Box::new(a),
                            head: h,
                            guard: AtomicU64::new(heap.global_epoch()),
                            pos: heap.form_pos(form),
                        });
                    }
                }
                if items.len() == 3 && scope.lookup(h).is_none() {
                    if let Some((op, map)) = resolve_prim(heap, h) {
                        let a = compile_node(heap, items[1], scope, false)?;
                        let b = compile_node(heap, items[2], scope, false)?;
                        // `a`'s value needs a root slot across `b`'s eval only
                        // if `b` can reach a safepoint (see the field doc).
                        let broot = !matches!(
                            b,
                            Node::Const(_) | Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. }
                        );
                        return Some(Node::Prim2 {
                            op,
                            a: Box::new(a),
                            b: Box::new(b),
                            map: [map[0] as u8, map[1] as u8],
                            head: h,
                            guard: AtomicU64::new(heap.global_epoch()),
                            pos: heap.form_pos(form),
                            broot,
                        });
                    }
                }
            }
            // Direct `letrec` self-recursive tail call (the self-call optimization):
            // a tail call whose head is this closure's own self-name, not shadowed by
            // a local, with exactly the arm's arity. Re-runs the current arm via the
            // trampoline without resolving the callee or dispatching. A non-tail
            // self-call, a shadowed name, or a mismatched arity falls through to the
            // regular env-resolved path below (still correct).
            if tail {
                if let (Value::Sym(h), Some((name, arity))) = (head, scope.self_call) {
                    if h == name && scope.lookup(h).is_none() && items.len() - 1 == arity {
                        let mut args = Vec::with_capacity(arity);
                        for &a in &items[1..] {
                            args.push(compile_node(heap, a, scope, false)?);
                        }
                        return Some(Node::SelfCall {
                            args: args.into_boxed_slice(),
                            pos: heap.form_pos(form),
                        });
                    }
                }
            }
            // Function call: compile the callee and every argument (value position).
            // A free-symbol head compiles to a plain `Node::Global` (not a
            // `GlobalIc`): the call's own site IC below caches the head's full
            // resolution, so a read IC there would be redundant (and waste a site).
            let callee = match head {
                Value::Sym(h) if scope.lookup(h).is_none() => Node::Global(h),
                _ => compile_node(heap, head, scope, false)?,
            };
            let mut args = Vec::with_capacity(items.len() - 1);
            for &a in &items[1..] {
                args.push(compile_node(heap, a, scope, false)?);
            }
            // A free-global callee gets a call-site inline-cache id (ADR-096);
            // a local/computed callee can resolve to a different function per
            // call, so it keeps the generic path.
            let site = match callee {
                Node::Global(_) => heap.vm_site_alloc(),
                _ => NO_SITE,
            };
            Some(Node::Call {
                callee: Box::new(callee),
                args: args.into_boxed_slice(),
                tail,
                // Capture the combination's source position now, while its
                // reader-recorded `form_pos` entry is live (a later collection moves
                // the LOCAL form, but `Pos` is plain data and stays valid).
                pos: heap.form_pos(form),
                site,
            })
        }

        // Vector literal — evaluate each element (value position), build fresh.
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut nodes = Vec::with_capacity(items.len());
            for e in items {
                nodes.push(compile_node(heap, e, scope, false)?);
            }
            Some(Node::Vector(nodes.into_boxed_slice()))
        }
        // Map literal — evaluate each key and value (value position), build fresh.
        Value::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let kn = compile_node(heap, k, scope, false)?;
                let vn = compile_node(heap, v, scope, false)?;
                pairs.push((kn, vn));
            }
            Some(Node::Map(pairs.into_boxed_slice()))
        }

        // Opaque handles, etc. — outside the VM's vocabulary.
        _ => None,
    }
}

/// Compile a closure's body to a [`CompiledArm`], or `None` if it isn't
/// VM-eligible (multi-arm with no exact arity, every arm `&optional`/`&` rest, or
/// every arm body uses a non-core form). Single-arm, exact-arity arms compile;
/// **local-capturing closures are eligible** (Stage 2c) — a free var resolves by
/// name through the closure's captured env (`Node::Global` → `env_get(genv, …)`),
/// which `vm_apply` sets to the closure's own env, so the body compiles the same
/// way whether the capture is global or local.
/// Compile one arm to a [`CompiledArm`], or `None` (defer this arm to the
/// tree-walker) if its body or any real `&optional` default uses a form outside the
/// VM vocabulary. Binds frame slots in layout order — required params, then each
/// optional (its default compiled *before* the optional's own slot is bound, so a
/// default sees the required params and earlier optionals but never itself), then
/// the `&` rest param — then compiles the body. The default nodes ride along in
/// `optional_defaults` for `push_frame` to evaluate on a missing arg.
/// Returns `true` if `node` (or any of its children) contains a
/// [`ConstVal::Handle`] or a [`Node::MakeClosure`] (whose `fn_rest` is always a
/// RUNTIME Pair handle). Used to set [`CompiledArm::has_runtime_handles`] at
/// compile time so `vm_apply` can skip `live_vm_arms` registration for pure
/// arithmetic / control-flow bodies that have nothing for `runtime_collect` to
/// rewrite.
fn node_has_rt_handles(node: &Node) -> bool {
    match node {
        Node::Const(cv) => matches!(cv, ConstVal::Handle { .. }),
        Node::MakeClosure { fn_rest, captures, .. } => {
            // fn_rest is always a RUNTIME Pair; captures may contain handles too.
            matches!(fn_rest, ConstVal::Handle { .. })
                || captures.iter().any(|(_, n)| node_has_rt_handles(n))
        }
        Node::If(a, b, c) => {
            node_has_rt_handles(a) || node_has_rt_handles(b) || node_has_rt_handles(c)
        }
        Node::Do(ns) => ns.iter().any(node_has_rt_handles),
        Node::Vector(ns) => ns.iter().any(node_has_rt_handles),
        Node::Map(pairs) => pairs.iter().any(|(k, v)| node_has_rt_handles(k) || node_has_rt_handles(v)),
        Node::Call { callee, args, .. } => {
            node_has_rt_handles(callee) || args.iter().any(node_has_rt_handles)
        }
        Node::SelfCall { args, .. } => args.iter().any(node_has_rt_handles),
        Node::LetBind { binds, body } => {
            binds.iter().any(|(_, n)| node_has_rt_handles(n)) || node_has_rt_handles(body)
        }
        Node::Prim2 { a, b, .. } => node_has_rt_handles(a) || node_has_rt_handles(b),
        Node::Prim1 { a, .. } => node_has_rt_handles(a),
        Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => false,
    }
}

fn compile_arm(
    heap: &Heap,
    required: &[Symbol],
    optionals: &[(Symbol, Value)],
    rest: Option<Symbol>,
    body: &[Value],
    enclosing: Vec<Symbol>,
    self_name: Option<Symbol>,
    defn_name: Option<Symbol>,
) -> Option<CompiledArm> {
    let nrequired = required.len();
    let noptional = optionals.len();
    let mut scope = Scope::with_params_enclosing(&[], enclosing);
    // The self-call optimization applies only to a plain fixed-arity closure (no
    // `&optional`/`&` rest), where a tail call passing exactly `nrequired` args
    // re-runs this arm verbatim. With optionals/rest the frame-fill differs per
    // call, so such calls fall back to the regular env-resolved path (correct,
    // just unoptimized).
    if let Some(name) = self_name {
        if noptional == 0 && rest.is_none() {
            scope.self_call = Some((name, nrequired));
        }
    }
    // `defn` tail self-calls get the same inline frame-reset via SelfCall. The
    // in-flight call holds an Arc to its own compiled arm, so it correctly runs
    // the current compiled version even if the global is redefined mid-call.
    if let Some(name) = defn_name {
        if noptional == 0 && rest.is_none() {
            scope.self_call = Some((name, nrequired));
        }
    }
    for &p in required {
        scope.bind(p);
    }
    let mut optional_defaults: Vec<Option<Node>> = Vec::with_capacity(noptional);
    for (name, default) in optionals {
        // A nil default needs no eval (push_frame just leaves the slot nil); a real
        // default compiles in the current scope (required + earlier optionals bound).
        let node = match default {
            Value::Nil => None,
            d => Some(compile_node(heap, *d, &mut scope, false)?),
        };
        optional_defaults.push(node);
        scope.bind(*name);
    }
    if let Some(r) = rest {
        scope.bind(r);
    }
    let body = compile_body(heap, body, &mut scope, true)?;
    let optional_defaults = optional_defaults.into_boxed_slice();
    let has_runtime_handles = node_has_rt_handles(&body)
        || optional_defaults.iter().flatten().any(node_has_rt_handles);
    // Stage 1: try to compile the body to flat bytecode (a call-free, handle-free
    // subset for now — `compile_chunk` returns `None` otherwise, and the arm runs
    // via `exec_node` exactly as before).
    let chunk = compile_chunk(&body);
    // Reserve a few extra frame slots (above the compiler's `scope.max`) when the arm
    // has ≥2 non-tail calls, so a JIT-lowered version can spill call-result handles
    // that must survive a later call's safepoint (two-call recursion: `fib`, bintree
    // `check`). The VM never references these slots; `push_frame` nil-inits them like
    // any other. Computed identically here (to size the frame) and in `jit_lower_arm`
    // (to place spills) via `jit_spill_reserve`.
    let spill_reserve = chunk.as_ref().map_or(0, |c| jit_spill_reserve(&c.code));
    Some(CompiledArm {
        nrequired,
        noptional,
        optional_defaults,
        rest_slot: rest.map(|_| nrequired + noptional),
        nslots: scope.max + spill_reserve,
        body,
        chunk,
        has_runtime_handles,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        compile_epoch: AtomicU64::new(0),
    })
}

fn compile_closure(heap: &Heap, id: ClosureId) -> Option<CompiledClosure> {
    let cl = heap.closure(id);
    // The lexical names this closure inherits from outer closures (Stage 2c) —
    // empty for a global-capturing (top-level) closure. A nested `(fn …)` in the
    // body needs these to snapshot the enclosing environment it captures.
    let enclosing: Vec<Symbol> = match cl.env {
        Some(e) if !heap.is_global(e) => heap.env_chain_names(e),
        _ => Vec::new(),
    };
    // Direct `letrec` self-recursion (the self-call optimization): a closure whose
    // captured frame binds a name to *itself* (the `env_define` the `MakeClosure`
    // self-name path installs) is a local recursive helper — `defseq`'s `--loop`,
    // a hand-written named loop. A tail call to that name can re-invoke this very
    // arm without resolving the callee through the env or any dispatch (the binding
    // is an immutable letrec slot — no late-binding/epoch concern, unlike a global
    // `defn`, which is *not* self-bound in a captured frame and so never matches
    // here). `compile_arm` turns such calls into [`Node::SelfCall`].
    let self_name: Option<Symbol> = match cl.env {
        Some(e) if !heap.is_global(e) => heap.env_frame_self_name(e, id),
        _ => None,
    };
    // `defn` tail self-calls use the same `Inst::SelfCall` inline frame-reset path as
    // letrec. The in-flight call's Arc owns the compiled arm, so it runs the current
    // compiled version even if the global is redefined; new callers see the new version.
    let defn_name: Option<Symbol> = if cl.env.is_none() { cl.name } else { None };
    // Snapshot every arm's shape + body (cloning ends the `cl` borrow), then compile
    // each via [`compile_arm`]. An arm is VM-eligible when its body — and every real
    // `&optional` default form — is core vocabulary; otherwise that arm defers
    // (`compiled: None`). Ineligible arms are still recorded so `arm_for` selection
    // stays faithful to `select_arm` (variadic/exact overlap — see ArmSpec).
    struct Src {
        required: Vec<Symbol>,
        optionals: Vec<(Symbol, Value)>, // name + default form (`Nil` = nil-default)
        rest: Option<Symbol>,
        body: Vec<Value>,
    }
    let arms_src: Vec<Src> = cl
        .arms
        .iter()
        .map(|a| Src {
            required: a.params.clone(),
            optionals: a.optionals.clone(),
            rest: a.rest,
            body: a.body.clone(),
        })
        .collect();
    let mut specs: Vec<ArmSpec> = Vec::with_capacity(arms_src.len());
    for s in arms_src {
        let nrequired = s.required.len();
        let noptional = s.optionals.len();
        let has_rest = s.rest.is_some();
        let compiled =
            compile_arm(heap, &s.required, &s.optionals, s.rest, &s.body, enclosing.clone(), self_name, defn_name)
                .map(Arc::new);
        specs.push(ArmSpec { nrequired, noptional, has_rest, compiled });
    }
    // Nothing to gain if no arm compiled (and a wholly-`None` entry would just mask
    // the tree-walker on every call) — defer the closure.
    if specs.iter().all(|s| s.compiled.is_none()) {
        None
    } else {
        Some(CompiledClosure { arms: specs })
    }
}

/// A stable cache key for closure `id`, or `None` if it can't be safely cached /
/// VM-run (ADR-076 §2c(a)). A **RUNTIME** closure (top-level / promoted) is keyed
/// by its own handle `.0`, which is stable for the closure's life. A **LOCAL**
/// closure's handle index is recycled by the collector, so it's keyed instead by
/// the handle of its first body form — but only when that form lives in the
/// immovable RUNTIME code region. A LOCAL closure whose body was built from movable
/// LOCAL forms (e.g. conased by `eval`/quasiquote) has no stable key *and* would
/// put movable handles in the cached `Node` tree, so it's left to the tree-walker.
fn cache_key(heap: &Heap, id: ClosureId) -> Option<VmCacheKey> {
    match id.region() {
        value::RUNTIME | value::PRELUDE => Some(VmCacheKey::Runtime(id.0)),
        value::LOCAL => {
            // Key on the first arm's first body form. Require an allocated RUNTIME
            // handle so the key is both stable and collision-free (immediates and
            // interned symbols are shared, so they'd alias unrelated closures).
            let first = heap.closure(id).arms.first()?.body.first().copied()?;
            match first {
                Value::Pair(p) if p.region() != value::LOCAL => Some(VmCacheKey::LocalBody(p.0)),
                _ => None,
            }
        }
        _ => None, // any other region (e.g. a blob/shared handle) — not VM-cached.
    }
}

/// The compiled body for closure `id`, compiling-and-caching on first use. Keyed by
/// [`cache_key`] so a local-capturing closure is found by its RUNTIME body code,
/// not its recycled LOCAL handle. `None` (ineligible) is cached too — but only when
/// the closure *has* a stable key; an unkeyable closure simply defers each call
/// (cheap: a region check + a body-handle peek).
/// The per-call hot path: resolve `id`'s `argc` arm, cloning **only** the
/// `Arc<CompiledArm>` (not the enclosing `CompiledClosure`). On a cache hit
/// (the overwhelmingly common case — a recursive or repeated callee) this is a
/// single `vm_cache_arm` lookup + one arm clone. A miss compiles + caches the
/// closure once, then resolves the arm. `None` = no VM arm for `argc` (defer to
/// the tree-walker), identical to `compiled_for(..).and_then(|c| c.arm_for(argc))`.
fn compiled_arm_for(heap: &Heap, id: ClosureId, argc: usize) -> Option<Arc<CompiledArm>> {
    let key = cache_key(heap, id)?;
    if let Some(hit) = heap.vm_cache_arm(key, argc) {
        return hit;
    }
    // Cold: compile + cache the closure once, then take the arm.
    let compiled = compile_closure(heap, id).map(Arc::new);
    heap.vm_cache_put(key, compiled.clone());
    compiled.and_then(|cc| cc.arm_for(argc).cloned())
}

// ===================== executor (Node → value) =====================

/// Resolve a [`Step`] to a value, running a `Tail` to completion. In value
/// positions the step is always `Done` (sub-nodes compile with `tail = false`);
/// this also makes a stray tail safe rather than a panic. A `Tail` carries its own
/// callee env (Stage 2c), so `force` needs no ambient env.
fn force(heap: &mut Heap, step: Step) -> LispResult {
    match step {
        Step::Done(v) => Ok(v),
        Step::Tail { compiled, args, genv } => vm_apply(heap, compiled, &args, genv),
    }
}

/// Int×Int-only fast path for `prim2_inline_exec`: just the fixnum arithmetic,
/// no type dispatch, no allocation.  Marked `#[inline(always)]` because it is
/// tiny (one `match` arm per op) — LLVM constant-folds `op` at each call site
/// in `prim2_inline_exec` (itself always-inlined), emitting a single checked op
/// or compare per instruction variant.  Float, BigInt, overflow, Cons, and Div
/// all return `None`; the caller falls through to `prim_apply`.
#[inline(always)]
fn prim2_int_fast(op: PrimOp, a: i64, b: i64) -> Option<Value> {
    match op {
        PrimOp::Add  => a.checked_add(b).map(Value::Int),
        PrimOp::Sub  => a.checked_sub(b).map(Value::Int),
        PrimOp::Mul  => a.checked_mul(b).map(Value::Int),
        PrimOp::Lt   => Some(Value::Bool(a < b)),
        PrimOp::Le   => Some(Value::Bool(a <= b)),
        PrimOp::Eq   => Some(Value::Bool(a == b)),
        PrimOp::Rem  => a.checked_rem(b).map(Value::Int),
        PrimOp::Quot => a.checked_div(b).map(Value::Int),
        // Cons needs heap alloc; Div may return Float — both handled by prim_apply.
        // VectorRef needs the heap (slab index) and its operands aren't (Int, Int);
        // handled directly in prim2_inline_exec.
        PrimOp::Cons | PrimOp::Div | PrimOp::VectorRef => None,
    }
}

/// The inline fast path for a [`Node::Prim2`] (perf #1): handle the `(Int, Int)`
/// case of `op` directly, returning `Ok(Some(v))` when done inline, or `Ok(None)`
/// to defer to the real `%`-primitive — for any non-`(Int, Int)` operands (float
/// coercion, structural `=`, bignum operands, the canonical type errors), the
/// division edges, **and the i64-overflow cases**, which the native now resolves
/// by promoting to a bignum (ADR bignums) rather than erroring. Needs no heap:
/// the inline result is a scalar, so nothing is allocated and no GC can intervene.
fn prim_apply(op: PrimOp, x: Value, y: Value) -> Result<Option<Value>, LispError> {
    let (a, b) = match (x, y) {
        (Value::Int(a), Value::Int(b)) => (a, b),
        _ => return Ok(prim_apply_float(op, x, y)),
    };
    let v = match op {
        // On i64 overflow, defer (`Ok(None)`): the native `prim_add`/etc. redo
        // the op in BigInt and demote, so a too-big result becomes a `BigInt`
        // instead of an `E0041`.
        PrimOp::Add => match a.checked_add(b) {
            Some(r) => Value::Int(r),
            None => return Ok(None),
        },
        PrimOp::Sub => match a.checked_sub(b) {
            Some(r) => Value::Int(r),
            None => return Ok(None),
        },
        PrimOp::Mul => match a.checked_mul(b) {
            Some(r) => Value::Int(r),
            None => return Ok(None),
        },
        PrimOp::Lt => Value::Bool(a < b),
        PrimOp::Le => Value::Bool(a <= b),
        PrimOp::Eq => Value::Bool(a == b),
        // Division family: handle the clean integer case inline, and **defer**
        // (`Ok(None)`) every edge — div-by-zero, the `i64::MIN / -1` overflow,
        // and (`%div` only) a non-exact quotient that the native returns as a
        // Float — so the native owns those exact results and error messages.
        PrimOp::Rem => match a.checked_rem(b) {
            Some(r) => Value::Int(r),
            None => return Ok(None),
        },
        // `%div` returns an Int only when it divides evenly (matching `prim_div`);
        // a remainder means a Float result, which the native builds.
        PrimOp::Div => match (a.checked_rem(b), a.checked_div(b)) {
            (Some(0), Some(q)) => Value::Int(q),
            _ => return Ok(None),
        },
        PrimOp::Quot => match a.checked_div(b) {
            Some(q) => Value::Int(q),
            None => return Ok(None),
        },
        // Handled in the exec arm (they need `&mut Heap` / the heap); never reach here.
        PrimOp::Cons | PrimOp::VectorRef => return Ok(None),
    };
    Ok(Some(v))
}

/// The float fast path of [`prim_apply`] (ADR-096): both operands `Int`/`Float`
/// with at least one `Float` — exactly the shapes `num_bin`/`prim_lt`'s float
/// arms handle with a plain `f64` op after an exact `i64 as f64` coercion.
/// Everything else (`BigInt` operands, structural `=` on floats, `rem`/`quot`'s
/// numeric edges, division by zero) returns `None` so the real native owns the
/// result and the error messages.
fn prim_apply_float(op: PrimOp, x: Value, y: Value) -> Option<Value> {
    let (a, b) = match (x, y) {
        (Value::Float(a), Value::Float(b)) => (a, b),
        (Value::Int(a), Value::Float(b)) => (a as f64, b),
        (Value::Float(a), Value::Int(b)) => (a, b as f64),
        _ => return None,
    };
    Some(match op {
        PrimOp::Add => Value::Float(a + b),
        PrimOp::Sub => Value::Float(a - b),
        PrimOp::Mul => Value::Float(a * b),
        PrimOp::Lt => Value::Bool(a < b),
        PrimOp::Le => Value::Bool(a <= b),
        // `%div`: the native errors on a zero denominator — defer that edge
        // (a NaN/inf denominator is not zero, so it stays inline, matching the
        // native's plain `a / b`).
        PrimOp::Div if b != 0.0 => Value::Float(a / b),
        // `=` is structural (the native owns float equality), `rem`/`quot` take
        // the numeric-tower path, and zero-denominator `%div` errors — defer.
        _ => return None,
    })
}

/// Guard-checked inline path shared by all three `Prim2` bytecode handlers.
/// Returns `Ok(Some(v))` when the operation completed inline (caller pushes `v`),
/// `Ok(None)` when the guard is stale or the operand shape needs the native
/// (overflow, BigInt, float-not-matched), and `Err` on a type/arithmetic error.
/// Handles `Cons` inline here (it allocates, so it needs `&mut Heap`).
#[inline(always)]
fn prim2_inline_exec(
    heap: &mut Heap,
    op: PrimOp,
    map: [u8; 2],
    swapped: bool,
    head: Symbol,
    guard: &AtomicU64,
    x: Value,
    y: Value,
) -> Result<Option<Value>, LispError> {
    let cur = heap.global_epoch();
    // The map the *head* itself resolves to (`resolve_prim`'s natural arg-map). For a
    // `(op Const Local)` fusion (`swapped`), the instruction's `map` was inverted so the
    // inline operand pick stays correct (`emit_node`), so un-invert it before comparing —
    // otherwise revalidation never matches and the arm silently slow-paths forever after
    // the first epoch bump (a `def`). Non-swapped instructions compare `map` directly.
    let head_map = if swapped {
        [1 - map[0] as usize, 1 - map[1] as usize]
    } else {
        [map[0] as usize, map[1] as usize]
    };
    let inlinable = guard.load(Ordering::Relaxed) == cur || {
        match resolve_prim(heap, head) {
            Some((op2, m2)) if op2 == op && m2 == head_map => {
                guard.store(cur, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    };
    if !inlinable {
        return Ok(None);
    }
    // Int×Int fast path: `prim2_int_fast` is tiny and #[inline(always)] — LLVM
    // constant-folds `op` here, emitting one checked op or compare per handler,
    // with no function call and without bloating exec_chunk via full prim_apply.
    // (`VectorRef`/`Cons` never have `(Int, Int)` operands, so they skip this and
    // are handled on the cold path below — keeping this hot path branch-free of
    // them.)
    if let (Value::Int(a), Value::Int(b)) = (x, y) {
        if let Some(v) = prim2_int_fast(op, a, b) {
            crate::perf_bump!(prim2_inline);
            return Ok(Some(v));
        }
        // Int overflow, Div, or Cons with Int operands → fall through to prim_apply.
    }
    // Float, BigInt, overflow, Cons, Div, VectorRef — the cold, type-coercing
    // path (not inlined, so it stays out of exec_chunk's instruction footprint).
    match prim_apply(op, x, y)? {
        Some(v) => {
            crate::perf_bump!(prim2_inline);
            Ok(Some(v))
        }
        None if op == PrimOp::Cons => {
            crate::perf_bump!(prim2_inline);
            Ok(Some(heap.alloc_pair(x, y)))
        }
        // `vector-ref`: a dense O(1) slab read. Inline only the in-bounds
        // `(Vector, Int)` case; non-vector / non-int / out-of-range defer
        // (`Ok(None)`) to the native, which owns the exact bounds + type errors.
        None if op == PrimOp::VectorRef => {
            if let (Value::Vector(id), Value::Int(n)) = (x, y) {
                if n >= 0 && (n as usize) < heap.vector(id).len() {
                    crate::perf_bump!(prim2_inline);
                    return Ok(Some(heap.vector(id)[n as usize]));
                }
            }
            Ok(None)
        }
        None => Ok(None), // overflow or other deferred edge → fallback
    }
}

/// Slow-path dispatch shared by all three `Prim2` bytecode handlers.
/// Operands are already rooted at `[save]` and `[save+1]`; this function looks
/// up `head`, dispatches, truncates back to `save`, and returns the result.
/// Marked `inline(never)` to keep the cold path out of the hot dispatch loop.
#[inline(never)]
fn prim2_dispatch_rooted(
    heap: &mut Heap,
    head: Symbol,
    save: usize,
    pos: Option<Pos>,
    genv: EnvRoot,
) -> Result<Value, LispError> {
    crate::perf_bump!(prim2_fallback);
    let cur_env = heap.read_root_env(genv);
    let callee = match heap.env_get(cur_env, head) {
        Some(c) => c,
        None => {
            heap.truncate_roots(save);
            return Err(tag_pos(crate::eval::unbound_error(heap, head), pos));
        }
    };
    let sa = heap.root_at(save);
    let sb = heap.root_at(save + 1);
    let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa, sb]);
    let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
    heap.truncate_roots(save);
    result.map_err(|e| tag_pos(e, pos))
}

/// Walk a compiled `Node` tree, rewriting every embedded movable handle
/// (a `Const` literal or a `MakeClosure` `fn_rest`) in place through `f`. The crux of
/// the live-arm fixup: a RUNTIME compaction evacuates the code region, but the `Arc`'d
/// node trees of the **executing** arms are off the GC root graph (and held by
/// `&Node` on the Rust stack, so the `Arc` can't be swapped). `runtime_collect` walks
/// the live arms (registered in `Heap::live_vm_arms`) and calls this with `f` =
/// `flush_rt_value` so their handles point into the compacted region. Atoms and child
/// structure are untouched; only `ConstVal::Handle` bits move.
fn rewrite_node(node: &Node, f: &mut dyn FnMut(Value) -> Value) {
    match node {
        Node::Const(cv) => cv.rewrite(f),
        Node::Local(_) | Node::Global(_) | Node::GlobalIc { .. } => {}
        Node::If(a, b, c) => {
            rewrite_node(a, f);
            rewrite_node(b, f);
            rewrite_node(c, f);
        }
        Node::Do(ns) | Node::Vector(ns) => {
            for n in ns.iter() {
                rewrite_node(n, f);
            }
        }
        Node::Map(pairs) => {
            for (k, v) in pairs.iter() {
                rewrite_node(k, f);
                rewrite_node(v, f);
            }
        }
        Node::Call { callee, args, .. } => {
            rewrite_node(callee, f);
            for a in args.iter() {
                rewrite_node(a, f);
            }
        }
        Node::SelfCall { args, .. } => {
            for a in args.iter() {
                rewrite_node(a, f);
            }
        }
        Node::LetBind { binds, body } => {
            for (_, n) in binds.iter() {
                rewrite_node(n, f);
            }
            rewrite_node(body, f);
        }
        Node::MakeClosure { fn_rest, captures, self_name: _ } => {
            fn_rest.rewrite(f);
            for (_, n) in captures.iter() {
                rewrite_node(n, f);
            }
        }
        Node::Prim2 { a, b, .. } => {
            rewrite_node(a, f);
            rewrite_node(b, f);
        }
        Node::Prim1 { a, .. } => rewrite_node(a, f),
    }
}

/// Rewrite every movable handle embedded in a live compiled arm — its body plus each
/// real `&optional` default form, and the bytecode `chunk` if present (its `Const`s
/// and `MakeClosure` `fn_rest` are separate handle copies that must move too). Called
/// by `runtime_collect` per registered live arm.
pub fn rewrite_arm_handles(arm: &CompiledArm, f: &mut dyn FnMut(Value) -> Value) {
    rewrite_node(&arm.body, f);
    for d in arm.optional_defaults.iter() {
        if let Some(n) = d {
            rewrite_node(n, f);
        }
    }
    if let Some(chunk) = &arm.chunk {
        rewrite_chunk(chunk, f);
    }
}

/// Rewrite every movable handle a [`Chunk`] embeds — a `Const` literal and a
/// `MakeClosure`'s `fn_rest` — in place through `f`, the bytecode counterpart of
/// [`rewrite_node`]. (Capture-source values are computed at run time from
/// `Local`/`Global` reads, not embedded, so they carry no handle.)
fn rewrite_chunk(chunk: &Chunk, f: &mut dyn FnMut(Value) -> Value) {
    for inst in chunk.code.iter() {
        match inst {
            Inst::Const(cv) => cv.rewrite(f),
            Inst::MakeClosure { fn_rest, .. } => fn_rest.rewrite(f),
            _ => {}
        }
    }
}

/// Execute one node in **value position** — operands, call arguments, literal
/// elements, binding right-hand sides: the overwhelmingly common case. Returns
/// the value directly — no [`Step`] is built and no [`force`] unwrap runs. A
/// `Call` reached here was compiled `tail = false`, so [`exec_call`]'s step is
/// always `Done` (and a stray `Tail` is still resolved safely by [`force`]).
fn exec_value(
    heap: &mut Heap,
    node: &Node,
    frame_base: usize,
    genv: EnvRoot,
) -> LispResult {
    match node {
        Node::Const(cv) => Ok(cv.load()),
        // Slot read — depth 0: the callee's own frame. (Deeper depths arrive with
        // the full compiler; the slice only binds params.)
        Node::Local(i) => Ok(heap.root_at(frame_base + i)),
        Node::Global(s) => match heap.env_get(heap.read_root_env(genv), *s) {
            Some(v) => Ok(v),
            None => Err(crate::eval::unbound_error(heap, *s)),
        },
        Node::GlobalIc { sym, site } => {
            let env = heap.read_root_env(genv);
            // The IC engages only when free names resolve through the process
            // global (same gate as the call-site IC): a captured-env frame can
            // shadow the symbol, and differs per closure instance.
            if heap.is_global(env) {
                let epoch = heap.global_epoch();
                if let Some(v) = heap.vm_global_ic_probe(*site, *sym, epoch) {
                    crate::perf_bump!(global_ic_hit);
                    return Ok(v);
                }
                crate::perf_bump!(global_ic_miss);
                return match heap.env_get(env, *sym) {
                    Some(v) => {
                        // Never cache a dynamic symbol — `binding` rebinds it
                        // without bumping the epoch (a later `defdyn` of a cached
                        // symbol bumps it, so the stale entry self-invalidates).
                        if !value::is_dynamic(*sym) {
                            heap.vm_global_ic_put(*site, *sym, epoch, v);
                        }
                        Ok(v)
                    }
                    None => Err(crate::eval::unbound_error(heap, *sym)),
                };
            }
            match heap.env_get(env, *sym) {
                Some(v) => Ok(v),
                None => Err(crate::eval::unbound_error(heap, *sym)),
            }
        }
        Node::If(cond, then, els) => {
            let c = exec_value(heap, cond, frame_base, genv)?;
            if crate::eval::truthy(c) {
                exec_value(heap, then, frame_base, genv)
            } else {
                exec_value(heap, els, frame_base, genv)
            }
        }
        Node::Do(nodes) => {
            if nodes.is_empty() {
                return Ok(Value::Nil);
            }
            let last = nodes.len() - 1;
            for n in &nodes[..last] {
                exec_value(heap, n, frame_base, genv)?; // for effect
            }
            exec_value(heap, &nodes[last], frame_base, genv)
        }
        Node::Vector(elems) => {
            // Evaluate each element, keeping the results on the operand stack so a
            // collection during a later element relocates them in place (mirrors the
            // `Call` arg loop); then build a fresh vector. `save` is truncated on
            // every path, including errors.
            let save = heap.roots_len();
            for e in elems.iter() {
                match exec_value(heap, e, frame_base, genv) {
                    Ok(v) => heap.push_root(v),
                    Err(err) => {
                        heap.truncate_roots(save);
                        return Err(err);
                    }
                }
            }
            let n = elems.len();
            let mut vals = Vec::with_capacity(n);
            for k in 0..n {
                vals.push(heap.root_at(save + k));
            }
            heap.truncate_roots(save);
            Ok(heap.alloc_vector(vals))
        }
        Node::Map(entries) => {
            // Same operand-stack discipline as `Vector`: each key then value is
            // pushed (so a collection mid-build relocates them), then a fresh map is
            // built from the relocated pairs.
            let save = heap.roots_len();
            for (kn, vn) in entries.iter() {
                for node in [kn, vn] {
                    match exec_value(heap, node, frame_base, genv) {
                        Ok(v) => heap.push_root(v),
                        Err(err) => {
                            heap.truncate_roots(save);
                            return Err(err);
                        }
                    }
                }
            }
            let n = entries.len();
            let mut pairs = Vec::with_capacity(n);
            for i in 0..n {
                pairs.push((heap.root_at(save + 2 * i), heap.root_at(save + 2 * i + 1)));
            }
            heap.truncate_roots(save);
            Ok(heap.map_from_pairs(pairs))
        }
        Node::LetBind { binds, body } => {
            // Value-position `let` (an argument/operand): same slot discipline as
            // the tail flavor in `exec_node`, body in value position.
            for (slot, rhs) in binds.iter() {
                let v = exec_value(heap, rhs, frame_base, genv)?;
                heap.set_root_at(frame_base + slot, v);
            }
            exec_value(heap, body, frame_base, genv)
        }
        Node::MakeClosure { fn_rest, captures, self_name } => {
            // Build the captured env: a flat snapshot of the enclosing lexicals
            // (parent = the process global, so true globals + dynamics still resolve
            // live and late-bound). No `captures` source is a call, so evaluating
            // them runs no safepoint — the fresh `frame` and the (immovable) node
            // fields stay valid until `make_closure` consumes them below. With no
            // captures *and* no self-name the closure is global-capturing
            // (`env == None`); a self-name needs a frame to bind into.
            let env = if captures.is_empty() && self_name.is_none() {
                heap.global()
            } else {
                let frame = heap.new_env(Some(heap.global()));
                for (name, src) in captures.iter() {
                    let v = exec_value(heap, src, frame_base, genv)?;
                    heap.env_define(frame, *name, v);
                }
                frame
            };
            let closure = crate::eval::make_closure(heap, None, fn_rest.load(), env)?;
            // Direct `letrec` self-recursion: bind the binder name to the closure
            // we just built, in the closure's own captured env. The recursive call
            // then resolves through that env (uncached — a local-capturing frame
            // isn't `is_global`, so neither inline cache engages). This makes the
            // env contain the closure while the closure captures the env — the same
            // cycle the tree-walker's `letrec` builds, handled by the tracing GC.
            if let Some(name) = self_name {
                heap.env_define(env, *name, closure);
            }
            Ok(closure)
        }
        Node::SelfCall { .. } => {
            // Emitted only in tail position (`compile_node`'s `if tail` guard), so it
            // is always handled by `exec_node`, never reached here in value position.
            unreachable!("Node::SelfCall is tail-only — exec_node handles it");
        }
        Node::Call { callee, args, tail, pos, site } => {
            let step = exec_call(heap, callee, args, *tail, *pos, *site, frame_base, genv)?;
            force(heap, step)
        }
        Node::Prim1 { op, a, head, guard, pos } => {
            let pos = *pos;
            let tag = |e: LispError| match pos {
                Some(p) => e.or_pos(p),
                None => e,
            };
            let sa = exec_value(heap, a, frame_base, genv).map_err(tag)?;
            // Inline only while `head` still resolves to `op` (epoch-guarded, as
            // in `Prim2`). The inline cases read a slab cell and run no further
            // eval, so the operand needs no rooting here.
            let cur = heap.global_epoch();
            let inlinable = if guard.load(Ordering::Relaxed) == cur {
                true
            } else {
                match resolve_prim1(heap, *head) {
                    Some(op2) if op2 == *op => {
                        guard.store(cur, Ordering::Relaxed);
                        true
                    }
                    _ => false,
                }
            };
            if inlinable {
                match (op, sa) {
                    (PrimOp1::First, Value::Pair(p)) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(heap.pair(p).0);
                    }
                    (PrimOp1::Rest, Value::Pair(p)) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(heap.pair(p).1);
                    }
                    (PrimOp1::First | PrimOp1::Rest, Value::Nil) => {
                        crate::perf_bump!(prim1_inline);
                        return Ok(Value::Nil);
                    }
                    _ => {} // vectors/ranges/type errors → the native owns them
                }
            }
            crate::perf_bump!(prim1_fallback);
            // Fallback: a general call on the surface operator (rooted across
            // the dispatch, which can collect).
            let save = heap.roots_len();
            heap.push_root(sa);
            let cur_env = heap.read_root_env(genv);
            let callee = match heap.env_get(cur_env, *head) {
                Some(c) => c,
                None => {
                    heap.truncate_roots(save);
                    return Err(tag(crate::eval::unbound_error(heap, *head)));
                }
            };
            let sa = heap.root_at(save);
            let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa]);
            let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
            heap.truncate_roots(save);
            result.map_err(tag)
        }
        Node::Prim2 { op, a, b, map, head, guard, pos, broot } => {
            let pos = *pos;
            let tag = |e: LispError| match pos {
                Some(p) => e.or_pos(p),
                None => e,
            };
            // Evaluate operands in source order. `a`'s value is rooted across
            // `b`'s eval only when `b` can reach a safepoint (`broot` — see the
            // field doc); the common pure-leaf shape runs root-free, since the
            // inline path below touches no safepoint either. The fallback
            // dispatch roots both regardless. `save` is always truncated back.
            let save = heap.roots_len();
            let sa = match exec_value(heap, a, frame_base, genv) {
                Ok(v) => v,
                Err(e) => return Err(tag(e)),
            };
            if *broot {
                heap.push_root(sa);
            }
            let sb = match exec_value(heap, b, frame_base, genv) {
                Ok(v) => v,
                Err(e) => {
                    heap.truncate_roots(save);
                    return Err(tag(e));
                }
            };
            // Re-read `a` post-collection (a no-op unless it was rooted), then
            // route to the primitive's argument order. `b` ran no further eval,
            // so its value is current as-is.
            let sa = if *broot { heap.root_at(save) } else { sa };
            let src = [sa, sb];
            let x = src[map[0] as usize];
            let y = src[map[1] as usize];
            // Inline only while `head` still resolves to `op` (epoch-guarded). A
            // redefinition bumps `global_epoch`, forcing one re-validate; if it no
            // longer maps to the primitive we drop to the general fallback below.
            let cur = heap.global_epoch();
            let inlinable = if guard.load(Ordering::Relaxed) == cur {
                true
            } else {
                match resolve_prim(heap, *head) {
                    Some((op2, m2)) if op2 == *op && m2 == [map[0] as usize, map[1] as usize] => {
                        guard.store(cur, Ordering::Relaxed);
                        true
                    }
                    _ => false,
                }
            };
            if inlinable {
                match prim_apply(*op, x, y) {
                    Ok(Some(v)) => {
                        crate::perf_bump!(prim2_inline);
                        heap.truncate_roots(save);
                        return Ok(v);
                    }
                    // `prim_apply` is heap-less, so it always defers `cons`
                    // (which allocates) — inline it here, off the numeric ops'
                    // hot path. It accepts any operands: never defers on shape.
                    Ok(None) if *op == PrimOp::Cons => {
                        crate::perf_bump!(prim2_inline);
                        let v = heap.alloc_pair(x, y);
                        heap.truncate_roots(save);
                        return Ok(v);
                    }
                    Ok(None) => {} // non-inline operand shape → defer to the real primitive
                    Err(e) => {
                        heap.truncate_roots(save);
                        return Err(tag(e));
                    }
                }
            }
            crate::perf_bump!(prim2_fallback);
            // Fallback: call the surface operator on the source-order operands,
            // exactly as the generic call path would — covers a redefined
            // operator and every non-inline operand shape, with identical
            // semantics. Root both operands first (the dispatch can collect);
            // `sa` may already hold the slot at `save`.
            if !*broot {
                heap.push_root(sa);
            }
            heap.push_root(sb);
            let cur_env = heap.read_root_env(genv);
            let callee = match heap.env_get(cur_env, *head) {
                Some(c) => c,
                None => {
                    heap.truncate_roots(save);
                    return Err(tag(crate::eval::unbound_error(heap, *head)));
                }
            };
            let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa, sb]);
            let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
            heap.truncate_roots(save);
            result.map_err(tag)
        }
    }
}

/// The combination executor for the surviving `Node` path ([`exec_value`] — used by
/// `push_frame`'s `&optional` defaults and top-level `run`). Resolves the callee
/// through the call-site IC, evaluates the arguments onto the operand stack, and
/// dispatches; the returned [`Step`] is forced in value position. (The bytecode
/// engine uses its own `Inst::Call` path in [`exec_chunk`].)
#[allow(clippy::too_many_arguments)]
fn exec_call(
    heap: &mut Heap,
    callee: &Node,
    args: &[Node],
    tail: bool,
    pos: Option<Pos>,
    site: u32,
    frame_base: usize,
    genv: EnvRoot,
) -> Result<Step, LispError> {
    // Tag an error with this combination's source position if it doesn't
    // already carry one — so the *innermost* failing call wins (mirrors the
    // tree-walker's `or_form_pos`); a sub-call that already tagged itself is
    // left untouched. `None` (a promoted RUNTIME body) is a no-op.
    let tag = |e: LispError| match pos {
        Some(p) => e.or_pos(p),
        None => e,
    };
    // Resolve the callee — through this site's inline cache when it has one
    // (ADR-096). A hit skips the `env_get` walk entirely and may carry the
    // VM fast path (the callee's compiled arm + captured env); a miss
    // resolves normally and installs the entry, stamped with `probe_epoch`
    // (read *before* the resolution, so an arg-eval `def` below can't be
    // attributed to this resolution). Engages only when the body's free
    // names resolve through the process global (`is_global`): a
    // local-capturing closure's captured frames could shadow the symbol,
    // and they differ per closure *instance* while the site is shared.
    let probe_epoch = heap.global_epoch();
    let mut fast: Option<(Arc<CompiledArm>, EnvId)> = None;
    let cv: Value;
    'resolve: {
        if site != NO_SITE {
            if let Node::Global(sym) = callee {
                if heap.is_global(heap.read_root_env(genv)) {
                    let argc = args.len() as u32;
                    if let Some((v, payload)) =
                        heap.vm_call_ic_probe(site, *sym, argc, probe_epoch)
                    {
                        crate::perf_bump!(call_ic_hit);
                        cv = v;
                        fast = payload;
                        break 'resolve;
                    }
                    crate::perf_bump!(call_ic_miss);
                    // Miss: resolve (exactly what `exec_value` on the callee
                    // would do), then install. A *dynamic* symbol is never
                    // cached — a `binding` re-binds it without bumping the
                    // epoch, so a cached resolution would bypass it. (A
                    // later `defdyn` of a cached symbol bumps the epoch, so
                    // the entry self-invalidates and the re-install refuses.)
                    let env = heap.read_root_env(genv);
                    let v = match heap.env_get(env, *sym) {
                        Some(v) => v,
                        None => return Err(tag(crate::eval::unbound_error(heap, *sym))),
                    };
                    if !value::is_dynamic(*sym) {
                        let arm = match v {
                            // Cache the VM fast path only for a callee
                            // `dispatch` would run on the VM directly: a
                            // non-passthrough closure with a compiled arm
                            // for this argc. Everything else caches just
                            // the value (still skips the lookup walk).
                            Value::Fn(id)
                                if crate::eval::passthrough_arm(heap, id, args.len())
                                    .is_none() =>
                            {
                                compiled_arm_for(heap, id, args.len()).map(|arm| {
                                    let cenv =
                                        heap.closure(id).env.unwrap_or_else(|| heap.global());
                                    (arm, cenv)
                                })
                            }
                            _ => None,
                        };
                        fast = arm.clone();
                        heap.vm_call_ic_put(
                            site,
                            crate::core::heap::CallIcEntry {
                                sym: *sym,
                                argc,
                                epoch: probe_epoch,
                                callee: v,
                                arm,
                            },
                        );
                    }
                    cv = v;
                    break 'resolve;
                }
            }
        }
        // No IC for this site/shape: evaluate the callee node as before.
        cv = exec_value(heap, callee, frame_base, genv).map_err(tag)?;
    }
    // Evaluate each argument, keeping the callee + results on the operand
    // stack so a collection during a later argument's eval relocates them in
    // place (mirrors `eval::eval_arguments`). `save` is this call's region;
    // it is always truncated back, including on the error path.
    let save = heap.roots_len();
    heap.push_root(cv);
    for a in args.iter() {
        match exec_value(heap, a, frame_base, genv) {
            Ok(v) => heap.push_root(v),
            Err(e) => {
                heap.truncate_roots(save);
                return Err(tag(e));
            }
        }
    }
    // Re-read post-collection from the (relocated) operand stack.
    let callee_v = heap.root_at(save);
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(args.len());
    for k in 0..args.len() {
        argv.push(heap.root_at(save + 1 + k));
    }
    // The IC fast path: run the cached compiled arm directly, skipping
    // `dispatch`'s passthrough probe + body-cache lookup + env read —
    // but only if the global epoch is *still* `probe_epoch`. An arg's
    // eval can `def` (new resolution next call — but THIS call correctly
    // uses the pre-args callee, which is `callee_v`, rooted) or fire a
    // RUNTIME compaction (which rewrites the rooted `callee_v` in place
    // but NOT the un-registered `fast` arm's node tree or its env
    // handle) — either bumps the epoch, so the stale fast path is
    // dropped and the rooted callee takes the generic path below.
    if let Some((arm, cenv)) = fast {
        if heap.global_epoch() == probe_epoch {
            let result = if tail {
                Ok(Step::Tail { compiled: arm, args: argv, genv: cenv })
            } else {
                vm_apply(heap, arm, &argv, cenv).map(Step::Done)
            };
            heap.truncate_roots(save);
            return result.map_err(tag);
        }
    }
    // The *current* env (read fresh post-collection) is what a native callee
    // runs in; a VM-eligible closure callee instead runs in its own captured
    // env, which `dispatch` reads off the closure.
    let cur_env = heap.read_root_env(genv);
    let result = dispatch(heap, callee_v, argv, tail, cur_env);
    heap.truncate_roots(save);
    result.map_err(tag)
}

/// Restores the `capture_top_level` flag on drop — so the gate is reset even if the
/// guarded tree-walker `apply` *panics* (caught by `run_one`'s `catch_unwind`). The
/// manual save/restore it replaces leaked a `false` flag on a panic until the next
/// `vm_run_bc` entry re-stamped it.
struct CaptureTopGuard(bool);
impl Drop for CaptureTopGuard {
    fn drop(&mut self) {
        crate::process::set_capture_top_level(self.0);
    }
}

/// Dispatch a call whose callee and `argv` are already evaluated. A VM-eligible
/// closure of matching arity runs on the VM (or, in tail position, returns a
/// `Tail` for the trampoline); everything else (natives, multi-arm / ineligible
/// closures, arity mismatches) defers to the tree-walker via `eval::apply`.
fn dispatch(
    heap: &mut Heap,
    callee: Value,
    argv: SmallVec<[Value; 4]>,
    tail: bool,
    genv: EnvId,
) -> Result<Step, LispError> {
    let mut cur_callee = callee;
    let mut cur_argv = argv;
    // Outer `'apply` loop: mirrors the TW's `'dispatch` loop (eval/mod.rs). Each
    // iteration runs the passthrough-redirect inner loop, then checks for `apply`
    // unfolding. On unfold, `cur_callee`/`cur_argv` are rewritten and the outer
    // loop continues so passthrough can redirect the unfolded callee (e.g.
    // `(apply + '(1 2))` unfolds to `(+ 1 2)`, then passthrough elides `+`).
    // On no-unfold, `break` falls through to the VM/TW dispatch below.
    //
    // O(1) stack: no new Rust frame per `apply` iteration — the unfolding and
    // re-dispatch all happen inside this single `dispatch` call, then `vm_apply`
    // (or a `Step::Tail` trampoline) handles the real callee.
    'apply: loop {
        // Thin-wrapper passthrough redirect (ADR-069), mirroring `eval`'s `'dispatch`
        // loop: a pure pass-through prelude op (`(< n 2)` → `<` whose 2-arg arm is
        // `(%lt n 2)`, etc.) redirects straight to its inner `%native` on remapped
        // args — so the hot loop reaches `call_native` directly instead of re-entering
        // `apply_closure` (a frame alloc + param binds + a body eval) for every
        // arithmetic/comparison op. Late-binding safe: it reads the *live* closure and
        // re-resolves the inner head each call (a symbol lookup — no GC, so `cur_argv`
        // stays valid). Looped for chained passthroughs.
        loop {
            let id = match cur_callee {
                Value::Fn(id) => id,
                _ => break,
            };
            let Some((head, map)) = crate::eval::passthrough_arm(heap, id, cur_argv.len()) else {
                break;
            };
            let cl_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
            // VM inner-head resolution: a direct `env_get` for a symbol head (no GC, so
            // `cur_argv` stays valid), else the head value itself. The shared
            // `passthrough_redirect_ok` then gates the redirect (callable inner only),
            // counts the reduction, and honours the deadline.
            let inner = match head {
                Value::Sym(s) => heap.env_get(cl_env, s),
                other => Some(other),
            };
            let Some(inner) = inner else { break };
            // A redirect back to the *same* closure is direct self-recursion
            // (`(defn hog () (hog))`), not a thin wrapper: looping it here would spin
            // un-preemptibly (this redirect path has no captureable safepoint). Break
            // so it dispatches as a normal call → its VM arm, whose loop-top reduction
            // check preempts it (ADR-100 §8.1).
            if matches!(inner, Value::Fn(iid) if iid.0 == id.0) {
                break;
            }
            if !crate::eval::passthrough_redirect_ok(inner)? {
                break;
            }
            let mut next: SmallVec<[Value; 4]> = SmallVec::with_capacity(map.len());
            for &i in &map {
                next.push(cur_argv[i]);
            }
            cur_callee = inner;
            cur_argv = next;
        }
        // `apply` unfolding: `(apply real arg... list)` → `(real arg... ...list)`.
        // Mirrors the TW's inline unfolding (eval/mod.rs `while let Native … "apply"`).
        // After unfolding, `continue 'apply` re-runs passthrough on the real callee.
        // If the callee is not `apply`, or arity < 2, break and dispatch normally.
        if let Value::Native(id) = cur_callee {
            if heap.native(id).name == "apply" && cur_argv.len() >= 2 {
                let list = cur_argv.pop().expect("cur_argv non-empty (len >= 2, checked)");
                let real = cur_argv.remove(0);
                cur_argv.extend(heap.seq_items(list)?);
                cur_callee = real;
                continue 'apply;
            }
        }
        break;
    }
    // A VM-eligible closure of matching arity runs on the VM (or yields a tail
    // call for the trampoline); a native or non-passthrough/ineligible callee goes
    // to the tree-walker via `eval::apply` (which is just `call_native` for a
    // native — cheap).
    if let Value::Fn(id) = cur_callee {
        // Resolve the arm cloning only the `Arc<CompiledArm>` (not the enclosing
        // `CompiledClosure`) — one fewer Arc clone per call on the hot path.
        if let Some(arm) = compiled_arm_for(heap, id, cur_argv.len()) {
            // Run the callee in *its own* captured env (Stage 2c): a
            // global-capturing closure (`env == None`) resolves to the process
            // global as before, while a local-capturing one resolves its free
            // vars in the env it closed over. `genv` (the caller's env) is only
            // for natives below.
            let callee_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
            if tail {
                return Ok(Step::Tail { compiled: arm, args: cur_argv, genv: callee_env });
            }
            return Ok(Step::Done(vm_apply(heap, arm, &cur_argv, callee_env)?));
        }
        // A closure with no VM-eligible arm for this argc — a true defer to the
        // tree-walker. Native frames created by the tree-walker can't be captured
        // by the state-capture machinery; gate off so any `receive` inside blocks
        // the worker (§7.4 dirty-scheduler carve-out) instead of attempting a
        // state-capture that can't cross the native boundary.
        crate::perf_bump!(tw_defer);
        let _guard = CaptureTopGuard(crate::process::set_capture_top_level(false));
        let result = crate::eval::apply(heap, cur_callee, &cur_argv, genv);
        return Ok(Step::Done(result?));
    }
    Ok(Step::Done(crate::eval::apply(heap, cur_callee, &cur_argv, genv)?))
}

/// Push a fresh activation frame for `arm` onto `Heap::roots`: required args, then
/// `&optional` slots (the provided arg, or nil if missing), then the `&` rest list
/// (the trailing args conased into a fresh list), then nil for the `let`/`letrec`
/// binders — `nslots` values total. Selection guarantees `args.len() >= nrequired`.
/// No eval runs here (the rest is a plain `list_from_slice`), so no collection can
/// happen between reading `args` and rooting the slots.
fn push_frame(
    heap: &mut Heap,
    arm: &CompiledArm,
    args: &[Value],
    genv: EnvRoot,
) -> Result<(), LispError> {
    let base = heap.roots_len();
    // Pre-allocate the whole frame as nil: every slot (params, optionals, rest, and
    // the body's `let`/`letrec` binders) must exist before anything writes it via
    // `set_root_at` — including a real `&optional` default whose body may bind its
    // own `let` slots.
    for _ in 0..arm.nslots {
        heap.push_root(Value::Nil);
    }
    // Consume ALL provided args into their (now-rooted) slots FIRST, before any
    // default is evaluated: a default's eval can collect, which would strand the
    // still-live `args` slice (LOCAL handles) if it were read afterwards.
    for i in 0..arm.nrequired {
        heap.set_root_at(base + i, args.get(i).copied().unwrap_or(Value::Nil));
    }
    // Provided optionals are a left-to-right prefix of `args`; the remainder are
    // missing and take their defaults below.
    let provided_opt = args.len().saturating_sub(arm.nrequired).min(arm.noptional);
    for j in 0..provided_opt {
        heap.set_root_at(base + arm.nrequired + j, args[arm.nrequired + j]);
    }
    if let Some(rslot) = arm.rest_slot {
        let start = (arm.nrequired + arm.noptional).min(args.len());
        let rest = heap.list_from_slice(&args[start..]);
        heap.set_root_at(base + rslot, rest);
    }
    // Missing optionals take their default, left-to-right (so a later default sees an
    // earlier one). `None` is a nil-default — the slot is already nil. A real default
    // evaluates against the frame: earlier params/optionals are filled and rooted;
    // its own slot and later slots are still nil (the compiler bound it after the
    // default, so the default can't name itself).
    for j in provided_opt..arm.noptional {
        if let Some(node) = &arm.optional_defaults[j] {
            let v = exec_value(heap, node, base, genv)?;
            heap.set_root_at(base + arm.nrequired + j, v);
        }
    }
    Ok(())
}

/// Run a chunked closure arm and its whole chain of chunked calls on the explicit
/// bytecode frame stack ([`vm_run_bc`]) — the sole VM executor since ADR-100 Stage 5
/// (the `Node`-walking trampoline was retired with the bytecode default). Every
/// `CompiledArm` from `compile_arm` has a chunk, so this always routes to the driver;
/// `vm_run_bc` does the per-frame live-arm registration + the runaway frame guard.
/// Callers: `dispatch` (non-tail VM-closure branch), `exec_call`'s IC fast path, and
/// `force` (a tail `Step`). The tree-walker (`BROOD_VM=0`) is the remaining fallback.
fn vm_apply(heap: &mut Heap, compiled0: Arc<CompiledArm>, args: &[Value], genv0: EnvId) -> LispResult {
    // `top_level = false`: this is a nested run (the process-body driver is
    // `run_process_body`), so it does no loop-top preempt/kill capture — only the
    // body driver does. A `receive` suspend that surfaces here re-raises (§8.1).
    match vm_run_bc(heap, compiled0, args, genv0, None, false)? {
        VmOutcome::Done(v) => Ok(v),
        // A `receive` suspended inside this VM run — but this run is **nested** under a
        // native (a `map`/`try`/`binding`/`%isolate` callback that re-entered the VM via
        // `dispatch`/`apply_value`), so its continuation can't be returned as a value.
        // This is the native-nested case (ADR-100 §8.1): discard the captured inner
        // frames (their roots were left intact for a top-level resume — unwind them to
        // the entry mark) and re-raise the `Control::Suspend` so the enclosing native
        // re-raises it untouched. The *outer* `vm_run_bc` then re-runs this native's
        // `Inst::Call` on resume — correct because the only shape that occurs has no
        // irreversible side effect before the `receive`. (A native-nested receive that
        // *would* repeat a side effect is gated off this path by `capture_top_level()`
        // and blocks its worker instead — the §7.4 dirty carve-out.)
        VmOutcome::Suspended(s) => {
            let deadline = s.deadline;
            heap.truncate_roots(s.entry_roots);
            heap.truncate_env_roots(s.entry_env);
            heap.live_arm_truncate(s.entry_arms);
            Err(LispError::suspend(deadline))
        }
        // `top_level = false` ⇒ no loop-top capture, so a nested run never preempts or
        // self-kills; these are produced only by the body driver (`run_process_body`).
        VmOutcome::Preempted(_) | VmOutcome::Killed => {
            unreachable!("a nested vm_apply run does no loop-top preempt/kill capture")
        }
    }
}

/// Run a green process's body thunk on the bytecode driver as the **top-level**
/// state-capture run (ADR-100 §8.3) — the entry `run_one` uses. A `None` `resume`
/// starts the body fresh (resolving `body`'s 0-arg compiled arm); a `Some` replays a
/// parked continuation. Unlike [`apply_value`]/[`vm_apply`], a
/// `Suspended`/`Preempted`/`Killed` outcome is **returned** (the scheduler parks /
/// re-enqueues / retires it) rather than re-raised — this is the body driver, so its
/// continuation is the process's continuation. A body with a compiled 0-arg arm runs on
/// the capture driver; one without (vanishingly rare) tree-walks on the worker thread
/// and its `receive`s block (the §7.4 dirty carve-out).
pub(crate) fn run_process_body(
    heap: &mut Heap,
    body: Value,
    resume: Option<Suspended>,
) -> Result<VmOutcome, LispError> {
    match resume {
        // Resume: the continuation's own `cur.arm` drives; `arm0`/`genv0` are ignored
        // by the resume branch, so pass a (cheap) clone of it as the placeholder.
        Some(s) => {
            let arm = s.cur.arm.clone();
            vm_run_bc(heap, arm, &[], EnvId::GLOBAL, Some(s), true)
        }
        // Fresh: run the 0-arg body. A VM-eligible body (the overwhelming case — every
        // body in the whole suite is) runs on the capture driver, so its `receive`s
        // capture + migrate. A body that defers to the tree-walker (no compiled 0-arg
        // arm — vanishingly rare; zero across the suite) has no reified frame stack to
        // capture, so it runs tree-walked **on this worker thread** and its `receive`s
        // **block** the worker (the dirty-scheduler carve-out, §7.4); it returns Done/
        // Err and never suspends. Either way: no coroutine.
        None => match body {
            Value::Fn(id) if compiled_arm_for(heap, id, 0).is_some() => {
                let arm = compiled_arm_for(heap, id, 0).expect("just checked is_some");
                let cenv = heap.closure(id).env.unwrap_or_else(|| heap.global());
                vm_run_bc(heap, arm, &[], cenv, None, true)
            }
            Value::Fn(_) => crate::eval::apply(heap, body, &[], EnvId::GLOBAL).map(VmOutcome::Done),
            _ => Err(LispError::type_err("process body must be a function")),
        },
    }
}

// ===================== bytecode stepping engine (ADR-100, Stage 1) =====================
//
// The first slice of the stepping-VM endgame: a compiled arm's `Node` body is also
// lowered to a flat **bytecode** `Chunk` — a linear instruction stream over the
// **same** operand stack (`Heap::roots`) the `Node` interpreter uses, run by a
// single non-recursive loop (`exec_chunk`). Stage 1 lowers only a **call-free,
// handle-free** subset (leaf/control/prim/let/collection nodes); anything else
// makes `compile_chunk` return `None` and the arm keeps running on `exec_node`.
//
// Why this shape: the endgame (concurrency-v2.md §7) needs a process's continuation
// to be relocatable heap data rather than a native Rust stack. Reifying the operand
// state was already done (it lives on `Heap::roots`); this reifies the *control*
// state (the instruction pointer) for a single arm. Later stages added `Call`/`Return`
// as explicit frame push/pop — so the cross-arm call stack is data too, and corosensei
// is gone (ADR-100 §8.4). The driver stays bit-identical to the `Node` interpreter,
// guarded by the differential test.

/// One bytecode instruction. A **stack machine**: each instruction pushes/pops on
/// the operand region of `Heap::roots` that sits just above the activation frame's
/// slots (`base..base+nslots`). Frame slots are read/written by absolute index
/// (`Local`/`SetLocal`); everything else is push/pop. The semantics of each arm
/// mirror the matching [`Node`] case in `exec_value`/`exec_node` exactly.
enum Inst {
    /// Push a constant. Stage 1 only embeds **atoms** (`compile_chunk` defers any
    /// body carrying a movable RUNTIME handle), so no in-place handle rewrite is
    /// needed for a chunk — unlike the `Node` tree (`rewrite_node`).
    Const(ConstVal),
    /// Push frame slot `base + i`.
    Local(usize),
    /// Push a free reference resolved through the env (no inline cache).
    Global(Symbol),
    /// Push a free reference in value position, with the global-read inline cache.
    GlobalIc { sym: Symbol, site: u32 },
    /// Discard the top operand (a non-final `do` form's value).
    Pop,
    /// Pop the top operand into frame slot `base + i` (a `let`/`letrec` binder).
    SetLocal(usize),
    /// Unconditional jump: set the instruction pointer to this index.
    Jump(usize),
    /// Pop the top operand; if falsy, jump to this index (an `if`'s else target).
    JumpIfFalse(usize),
    /// Pop `n` operands and push a fresh vector built from them.
    MakeVector(usize),
    /// Pop `2*n` operands (key, value, …) and push a fresh map.
    MakeMap(usize),
    /// Inlined 1-ary primitive (`first`/`rest`): replace the top operand with the
    /// result, or fall back to a general call on `head`. Mirrors `Node::Prim1`.
    Prim1 { op: PrimOp1, head: Symbol, guard: AtomicU64, pos: Option<Pos> },
    /// Inlined 2-ary primitive (`+`/`<`/`=`/`cons`/…): replace the top two operands
    /// with the result, or fall back to a general call on `head`. Mirrors
    /// `Node::Prim2`. (No `broot`: both operands are already rooted on the operand
    /// stack, so the stack machine never needs the explicit-root dance.)
    Prim2 { op: PrimOp, map: [u8; 2], head: Symbol, guard: AtomicU64, pos: Option<Pos> },
    /// Fused Prim2 where both operands are frame locals — reads `slot_a` and `slot_b`
    /// directly, bypassing 2 intermediate root-stack pushes. The inline fast path
    /// pushes only the result; the fallback pushes both slots before dispatching.
    Prim2SlotSlot { op: PrimOp, map: [u8; 2], slot_a: usize, slot_b: usize, head: Symbol, guard: AtomicU64, pos: Option<Pos> },
    /// Fused Prim2 where operand A is a frame local and B is a literal integer.
    /// Covers the very common `(+ slot 1)` / `(- slot 1)` / `(< slot k)` shape.
    /// Uses i64 directly (not ConstVal) so this variant stays under MakeClosure's
    /// size, avoiding Inst enum bloat that would widen every instruction.
    /// `swapped` records that the operands came from `(op Const Local)` — the fusion
    /// stored the *local* as `slot_a` and the *const* as `int_b` (with an inverted `map`
    /// so the inline path still routes correctly). The slow-path dispatch fallback calls
    /// the user `head`, which needs the **original** call order `(head Const Local)`, so
    /// it must un-swap. `false` for the `(op Local Const)` case (already in order).
    Prim2SlotInt { op: PrimOp, map: [u8; 2], slot_a: usize, int_b: i64, swapped: bool, head: Symbol, guard: AtomicU64, pos: Option<Pos> },
    /// A combination. The callee then each argument have been pushed (operand layout
    /// `[.., callee, arg0 .. arg_{argc-1}]` — callee resolved *before* the args, the
    /// tree-walker's order). A **non-tail** call to a chunked VM arm becomes a frame
    /// push (`ChunkExit::Call`); a tail call/self-call reuses the frame; a native /
    /// tree-walked callee runs inline and its value is pushed.
    ///
    /// `site`/`head` drive the **call-site inline cache** (ADR-096, Stage 5): when the
    /// head is a free global (`head = Some(sym)`, `site != NO_SITE`) and the frame
    /// resolves frees through the process global, the cached `(arm, env)` for
    /// `(site, sym, argc, epoch)` is used directly — skipping `dispatch`'s
    /// passthrough probe + `compiled_arm_for`. The callee is still resolved in-order
    /// (the pushed value), so eval order is unchanged and a `def` bumping the epoch
    /// invalidates the entry (the in-order callee then takes the generic path).
    Call { argc: usize, tail: bool, pos: Option<Pos>, site: u32, head: Option<Symbol> },
    /// Direct `letrec` self-tail-call (always tail position): args have been pushed;
    /// returns a `Step::SelfTail` so the trampoline re-enters this arm in place.
    /// Mirrors `Node::SelfCall`.
    SelfCall { argc: usize },
    /// Build a closure (`(fn …)` evaluated inside a compiled body). The `names`'
    /// capture values have been pushed (in order) by preceding leaf instructions;
    /// this binds them into a fresh captured env, builds the closure from `fn_rest`,
    /// and (for a direct `letrec` self-ref) late-binds `self_name` to it. Mirrors
    /// `Node::MakeClosure` / its `exec_value` arm exactly. `fn_rest` is a movable
    /// RUNTIME handle — rewritten in place by [`rewrite_chunk`].
    MakeClosure { fn_rest: ConstVal, names: Box<[Symbol]>, self_name: Option<Symbol> },
}

/// A compiled-to-bytecode arm body: a flat instruction stream evaluated by
/// [`exec_chunk`], leaving the body's single value on top of the operand stack.
/// `Send + Sync` (its `Inst`s hold only atoms, symbols, indices, and atomics), so it
/// rides in the `Arc<CompiledArm>` cached in a `Send` `Heap`.
pub struct Chunk {
    code: Vec<Inst>,
}

/// Lower a compiled `Node` body to a [`Chunk`], or `None` if it uses any node
/// outside Stage 1's vocabulary (`Call`/`SelfCall`/`MakeClosure`, or a `Const` with
/// a movable RUNTIME handle). `None` is always safe — the arm runs on `exec_node`.
fn compile_chunk(body: &Node) -> Option<Chunk> {
    let mut code = Vec::new();
    emit_node(body, &mut code)?;
    Some(Chunk { code })
}

/// Recursively emit `node` into `code`, leaving its value on the operand stack.
/// Returns `None` (aborting the whole chunk) on any unsupported node.
fn emit_node(node: &Node, code: &mut Vec<Inst>) -> Option<()> {
    match node {
        // A fresh `ConstVal` cloned from the node's (atoms inline; a movable RUNTIME
        // handle is re-encoded). Chunk handles are rewritten in place under a RUNTIME
        // compaction by `rewrite_chunk` (registered via the arm's `has_runtime_handles`).
        Node::Const(cv) => code.push(Inst::Const(ConstVal::new(cv.load()))),
        Node::Local(i) => code.push(Inst::Local(*i)),
        Node::Global(s) => code.push(Inst::Global(*s)),
        Node::GlobalIc { sym, site } => code.push(Inst::GlobalIc { sym: *sym, site: *site }),
        Node::If(cond, then, els) => {
            emit_node(cond, code)?;
            let j_else = code.len();
            code.push(Inst::JumpIfFalse(0)); // backpatched
            emit_node(then, code)?;
            let j_end = code.len();
            code.push(Inst::Jump(0)); // backpatched
            let else_ip = code.len();
            emit_node(els, code)?;
            let end_ip = code.len();
            code[j_else] = Inst::JumpIfFalse(else_ip);
            code[j_end] = Inst::Jump(end_ip);
        }
        Node::Do(nodes) => {
            if nodes.is_empty() {
                code.push(Inst::Const(ConstVal::Atom(Value::Nil)));
            } else {
                let last = nodes.len() - 1;
                for n in &nodes[..last] {
                    emit_node(n, code)?;
                    code.push(Inst::Pop); // evaluated for effect
                }
                emit_node(&nodes[last], code)?;
            }
        }
        Node::LetBind { binds, body } => {
            for (slot, rhs) in binds.iter() {
                emit_node(rhs, code)?;
                code.push(Inst::SetLocal(*slot));
            }
            emit_node(body, code)?;
        }
        Node::Vector(elems) => {
            for e in elems.iter() {
                emit_node(e, code)?;
            }
            code.push(Inst::MakeVector(elems.len()));
        }
        Node::Map(entries) => {
            for (k, v) in entries.iter() {
                emit_node(k, code)?;
                emit_node(v, code)?;
            }
            code.push(Inst::MakeMap(entries.len()));
        }
        Node::Prim1 { op, a, head, guard, pos } => {
            emit_node(a, code)?;
            code.push(Inst::Prim1 {
                op: *op,
                head: *head,
                guard: AtomicU64::new(guard.load(Ordering::Relaxed)),
                pos: *pos,
            });
        }
        Node::Prim2 { op, a, b, map, head, guard, pos, broot: _ } => {
            // Snapshot the guard epoch; each push site creates its own AtomicU64
            // (AtomicU64 is not Copy so we can't reuse a single binding).
            let gv = guard.load(Ordering::Relaxed);
            // Fuse when operands are frame locals or integer literals: avoids
            // the 2 intermediate root-stack pushes the generic path needs.
            // Only integer constants are fused (keeps Prim2SlotInt below
            // MakeClosure's size, so the Inst enum doesn't grow).
            let fused = match (&**a, &**b) {
                (Node::Local(sa), Node::Local(sb)) => {
                    code.push(Inst::Prim2SlotSlot {
                        op: *op, map: *map, slot_a: *sa, slot_b: *sb,
                        head: *head, guard: AtomicU64::new(gv), pos: *pos,
                    });
                    true
                }
                (Node::Local(sa), Node::Const(cv)) => {
                    if let Value::Int(n) = cv.load() {
                        code.push(Inst::Prim2SlotInt {
                            op: *op, map: *map, slot_a: *sa, int_b: n, swapped: false,
                            head: *head, guard: AtomicU64::new(gv), pos: *pos,
                        });
                        true
                    } else { false }
                }
                (Node::Const(cv), Node::Local(sb)) => {
                    if let Value::Int(n) = cv.load() {
                        // Slot goes to src[0], const to src[1] — invert the map. `swapped`
                        // so the dispatch fallback restores the original `(op Const Local)`
                        // order when it calls the user `head` (the inline path uses `map`).
                        let new_map = [1u8 - map[0], 1u8 - map[1]];
                        code.push(Inst::Prim2SlotInt {
                            op: *op, map: new_map, slot_a: *sb, int_b: n, swapped: true,
                            head: *head, guard: AtomicU64::new(gv), pos: *pos,
                        });
                        true
                    } else { false }
                }
                _ => false,
            };
            if !fused {
                emit_node(a, code)?;
                emit_node(b, code)?;
                code.push(Inst::Prim2 {
                    op: *op, map: *map, head: *head,
                    guard: AtomicU64::new(gv), pos: *pos,
                });
            }
        }
        Node::Call { callee, args, tail, pos, site } => {
            // Callee first, then each arg (the order `exec_call` evaluates them). When
            // the head is a free global, carry its symbol + `site` so the call-site IC
            // can cache the resolved arm (Stage 5); the callee is still pushed and
            // resolved in-order, so the IC is a pure cache.
            let head = if let Node::Global(s) = &**callee { Some(*s) } else { None };
            emit_node(callee, code)?;
            for a in args.iter() {
                emit_node(a, code)?;
            }
            code.push(Inst::Call { argc: args.len(), tail: *tail, pos: *pos, site: *site, head });
        }
        Node::SelfCall { args, pos: _ } => {
            for a in args.iter() {
                emit_node(a, code)?;
            }
            code.push(Inst::SelfCall { argc: args.len() });
        }
        Node::MakeClosure { fn_rest, captures, self_name } => {
            // Capture sources are leaf reads (an enclosing lexical → `Local`, or a
            // global → `Global`), so emitting them is safepoint-free; their values
            // land on the operand stack in `captures` order and `MakeClosure` binds
            // them to the matching names. A fresh `ConstVal` re-encodes `fn_rest`
            // (rewritten in place by `rewrite_chunk` under a compaction).
            for (_, src) in captures.iter() {
                emit_node(src, code)?;
            }
            let names: Box<[Symbol]> = captures.iter().map(|(name, _)| *name).collect();
            code.push(Inst::MakeClosure {
                fn_rest: ConstVal::new(fn_rest.load()),
                names,
                self_name: *self_name,
            });
        }
    }
    Some(())
}

/// Tag an error with `pos` if it doesn't already carry one (innermost wins),
/// matching the `Node` interpreter's `or_pos` discipline.
#[inline]
fn tag_pos(e: LispError, pos: Option<Pos>) -> LispError {
    match pos {
        Some(p) => e.or_pos(p),
        None => e,
    }
}

/// Run a [`Chunk`] frame from `*ip`, returning a [`ChunkExit`] to the driver
/// ([`vm_run_bc`]). `*ip` is **resumed and updated in place**, so after a non-tail
/// `Call` returns `ChunkExit::Call`, the driver re-enters here at the instruction
/// after the call once the callee frame completes. The operand stack (`Heap::roots`
/// above `base + nslots`) carries intermediate values; frame slots live at `base..`;
/// `genv` is the captured-env root. On error, returns `Err` without unwinding the
/// operand stack — the driver unwinds every frame's roots back to entry.
///
/// Stage 4: a **non-tail** `Call` to a chunked VM arm returns `ChunkExit::Call` so
/// the driver **pushes a frame** instead of recursing natively; a non-tail call to a
/// native or tree-walked arm is run here (via `dispatch`) and its value pushed. A
/// **tail** `Call`/`SelfCall` returns `Tail`/`SelfTail` so the driver reuses the
/// frame (TCO). A single pass to the next call/return is bounded by the chunk length.
fn exec_chunk(
    heap: &mut Heap,
    arm: &CompiledArm,
    ip: &mut usize,
    base: usize,
    genv: EnvRoot,
    capture: bool,
) -> Result<ChunkExit, LispError> {
    let chunk = arm.chunk.as_ref().expect("exec_chunk: arm has no chunk");
    // Back-edge tiering counter (jit only): counts `SelfCall` iterations so a hot
    // self-tail loop can be handed to the driver to tier (it would otherwise be a single
    // arm entry that never reaches the per-entry threshold). Checked every
    // `BACKEDGE_TIER_INTERVAL` iterations to keep the inline loop tight.
    #[cfg(feature = "jit")]
    let mut back_edges: u32 = 0;
    while *ip < chunk.code.len() {
        let inst = &chunk.code[*ip];
        *ip += 1;
        match inst {
            Inst::Const(cv) => {
                let v = cv.load();
                heap.push_root(v);
            }
            Inst::Local(i) => {
                let v = heap.root_at(base + i);
                heap.push_root(v);
            }
            Inst::Global(s) => {
                let env = heap.read_root_env(genv);
                match heap.env_get(env, *s) {
                    Some(v) => heap.push_root(v),
                    None => return Err(crate::eval::unbound_error(heap, *s)),
                }
            }
            Inst::GlobalIc { sym, site } => {
                let env = heap.read_root_env(genv);
                let v = if heap.is_global(env) {
                    let epoch = heap.global_epoch();
                    if let Some(v) = heap.vm_global_ic_probe(*site, *sym, epoch) {
                        crate::perf_bump!(global_ic_hit);
                        v
                    } else {
                        crate::perf_bump!(global_ic_miss);
                        match heap.env_get(env, *sym) {
                            Some(v) => {
                                if !value::is_dynamic(*sym) {
                                    heap.vm_global_ic_put(*site, *sym, epoch, v);
                                }
                                v
                            }
                            None => return Err(crate::eval::unbound_error(heap, *sym)),
                        }
                    }
                } else {
                    match heap.env_get(env, *sym) {
                        Some(v) => v,
                        None => return Err(crate::eval::unbound_error(heap, *sym)),
                    }
                };
                heap.push_root(v);
            }
            Inst::Pop => {
                let n = heap.roots_len();
                heap.truncate_roots(n - 1);
            }
            Inst::SetLocal(slot) => {
                let n = heap.roots_len();
                let v = heap.root_at(n - 1);
                heap.truncate_roots(n - 1);
                heap.set_root_at(base + slot, v);
            }
            Inst::Jump(t) => *ip = *t,
            Inst::JumpIfFalse(t) => {
                let n = heap.roots_len();
                let c = heap.root_at(n - 1);
                heap.truncate_roots(n - 1);
                if !crate::eval::truthy(c) {
                    *ip = *t;
                }
            }
            Inst::MakeVector(nelem) => {
                // Same discipline as `exec_value`'s `Node::Vector`: read the elements
                // (already on the operand stack), truncate, then build.
                let n = heap.roots_len();
                let start = n - nelem;
                let mut vals = Vec::with_capacity(*nelem);
                for k in 0..*nelem {
                    vals.push(heap.root_at(start + k));
                }
                heap.truncate_roots(start);
                let v = heap.alloc_vector(vals);
                heap.push_root(v);
            }
            Inst::MakeMap(npairs) => {
                let n = heap.roots_len();
                let start = n - 2 * npairs;
                let mut pairs = Vec::with_capacity(*npairs);
                for i in 0..*npairs {
                    pairs.push((heap.root_at(start + 2 * i), heap.root_at(start + 2 * i + 1)));
                }
                heap.truncate_roots(start);
                let v = heap.map_from_pairs(pairs);
                heap.push_root(v);
            }
            Inst::Prim1 { op, head, guard, pos } => {
                let pos = *pos;
                let n = heap.roots_len();
                let sa = heap.root_at(n - 1);
                let cur = heap.global_epoch();
                let inlinable = if guard.load(Ordering::Relaxed) == cur {
                    true
                } else {
                    match resolve_prim1(heap, *head) {
                        Some(op2) if op2 == *op => {
                            guard.store(cur, Ordering::Relaxed);
                            true
                        }
                        _ => false,
                    }
                };
                if inlinable {
                    match (op, sa) {
                        (PrimOp1::First, Value::Pair(p)) => {
                            crate::perf_bump!(prim1_inline);
                            let v = heap.pair(p).0;
                            heap.truncate_roots(n - 1);
                            heap.push_root(v);
                            continue;
                        }
                        (PrimOp1::Rest, Value::Pair(p)) => {
                            crate::perf_bump!(prim1_inline);
                            let v = heap.pair(p).1;
                            heap.truncate_roots(n - 1);
                            heap.push_root(v);
                            continue;
                        }
                        (PrimOp1::First | PrimOp1::Rest, Value::Nil) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::Nil);
                            continue;
                        }
                        _ => {}
                    }
                }
                crate::perf_bump!(prim1_fallback);
                let cur_env = heap.read_root_env(genv);
                let callee = match heap.env_get(cur_env, *head) {
                    Some(c) => c,
                    None => return Err(tag_pos(crate::eval::unbound_error(heap, *head), pos)),
                };
                let sa = heap.root_at(n - 1);
                let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa]);
                let result = dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
                heap.truncate_roots(n - 1);
                match result {
                    Ok(v) => heap.push_root(v),
                    Err(e) => return Err(tag_pos(e, pos)),
                }
            }
            Inst::Prim2 { op, map, head, guard, pos } => {
                let n = heap.roots_len();
                let sa = heap.root_at(n - 2);
                let sb = heap.root_at(n - 1);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)? {
                    Some(v) => { heap.truncate_roots(n - 2); heap.push_root(v); }
                    None => {
                        // Operands already rooted at n-2 and n-1.
                        let v = prim2_dispatch_rooted(heap, *head, n - 2, *pos, genv)?;
                        heap.push_root(v);
                    }
                }
            }
            Inst::Prim2SlotSlot { op, map, slot_a, slot_b, head, guard, pos } => {
                let sa = heap.root_at(base + slot_a);
                let sb = heap.root_at(base + slot_b);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                let v = match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)? {
                    Some(v) => v,
                    None => {
                        let save = heap.roots_len();
                        heap.push_root(sa);
                        heap.push_root(sb);
                        prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                    }
                };
                heap.push_root(v);
            }
            Inst::Prim2SlotInt { op, map, slot_a, int_b, swapped, head, guard, pos } => {
                let sa = heap.root_at(base + slot_a);
                let sb = Value::Int(*int_b);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                let v = match prim2_inline_exec(heap, *op, *map, *swapped, *head, guard, x, y)? {
                    Some(v) => v,
                    None => {
                        // Dispatch to the user `head` in the ORIGINAL call order. For the
                        // `(op Const Local)` fusion (`swapped`) that's `[const, local]` =
                        // `[sb, sa]`; otherwise `[sa, sb]`. (The inline path above used the
                        // map; this slow path must reconstruct the source order — a
                        // mismatch silently mis-ordered non-commutative ops, e.g.
                        // `(/ 24 x)` ran as `(/ x 24)`.)
                        let save = heap.roots_len();
                        let (first, second) = if *swapped { (sb, sa) } else { (sa, sb) };
                        heap.push_root(first);
                        heap.push_root(second);
                        prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                    }
                };
                heap.push_root(v);
            }
            Inst::Call { argc, tail, pos, site, head } => {
                let pos = *pos;
                // Operand layout: [.., callee, arg0 .. arg_{argc-1}] (callee emitted
                // first, then each arg — the same order `exec_call` evaluates them).
                let n = heap.roots_len();
                let callee = heap.root_at(n - argc - 1);
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(*argc);
                for k in 0..*argc {
                    argv.push(heap.root_at(n - argc + k));
                }
                let cur_env = heap.read_root_env(genv);
                // Call-site IC (Stage 5): if the head is a free global resolving through
                // the process global, reuse the cached `(arm, env)` for this site/argc —
                // skipping `dispatch`'s passthrough probe + `compiled_arm_for`. The
                // callee was already resolved in-order (`callee` above), so this only
                // caches the *arm*; the epoch guard drops a stale entry after any `def`.
                let mut fast: Option<(Arc<CompiledArm>, EnvId)> = None;
                if *site != NO_SITE {
                    if let Some(sym) = head {
                        if heap.is_global(cur_env) {
                            let epoch = heap.global_epoch();
                            if let Some((_v, payload)) =
                                heap.vm_call_ic_probe(*site, *sym, *argc as u32, epoch)
                            {
                                crate::perf_bump!(call_ic_hit);
                                fast = payload;
                            } else if !value::is_dynamic(*sym) {
                                crate::perf_bump!(call_ic_miss);
                                // Cache the in-order-resolved `callee`'s arm (only a
                                // non-passthrough closure with a compiled arm for this
                                // argc gets the VM fast path; everything else caches the
                                // value alone and still takes `dispatch`).
                                let arm = match callee {
                                    Value::Fn(id)
                                        if crate::eval::passthrough_arm(heap, id, *argc)
                                            .is_none() =>
                                    {
                                        compiled_arm_for(heap, id, *argc).map(|arm| {
                                            let cenv = heap
                                                .closure(id)
                                                .env
                                                .unwrap_or_else(|| heap.global());
                                            (arm, cenv)
                                        })
                                    }
                                    _ => None,
                                };
                                fast = arm.clone();
                                heap.vm_call_ic_put(
                                    *site,
                                    crate::core::heap::CallIcEntry {
                                        sym: *sym,
                                        argc: *argc as u32,
                                        epoch,
                                        callee,
                                        arm,
                                    },
                                );
                            }
                        }
                    }
                }
                // Inline fast-path: IC hit for the exact same arm, same captured env, no
                // optional/rest params, and GC is not yet due. Covers the common
                // `(defn f (x) … (f …))` self-tail pattern (which uses `Inst::Call` via
                // a global, unlike `letrec` self-recursion which emits `Inst::SelfCall`).
                // This is the main speedup for loop/collatz/fib/reduce.
                //
                // We check the inline condition here — using borrows only — before the
                // `match fast` below consumes `argv` and `fast`. If the check passes we
                // reset the frame and `continue` the inner loop without ever returning to
                // `vm_run_bc`. If it doesn't, we fall through to the normal dispatch path.
                //
                // GC guard: `argv` was read from roots just above, with no allocation in
                // between, so the values are still valid. We skip the inline if GC is due
                // so the outer loop can collect (and can't have stale off-heap SmallVec).
                if *tail {
                    if let Some((ref compiled, cenv)) = fast {
                        if std::ptr::eq(compiled.as_ref(), arm)
                            && arm.noptional == 0
                            && arm.rest_slot.is_none()
                            && cur_env == cenv
                            && !heap.gc_due()
                        {
                            crate::perf_bump!(self_tail);
                            heap.truncate_roots(base + arm.nslots);
                            for i in 0..arm.nslots {
                                heap.set_root_at(base + i, Value::Nil);
                            }
                            for i in 0..arm.nrequired {
                                heap.set_root_at(base + i, argv[i]);
                            }
                            *ip = 0;
                            if let Some(used) = crate::core::alloc::soft_limit_hit() {
                                return Err(crate::eval::memory_limit_error(used));
                            }
                            if capture {
                                if crate::process::capture_hard_kill_pending() {
                                    return Ok(ChunkExit::Killed);
                                }
                                if crate::process::tick_capture() {
                                    return Ok(ChunkExit::Preempt);
                                }
                            } else {
                                crate::process::tick();
                            }
                            if crate::process::deadline_exceeded() {
                                return Err(crate::eval::deadline_error());
                            }
                            continue;
                        }
                    }
                }
                // IC hit with a VM arm → skip `dispatch` entirely; else resolve with
                // `tail = true` so a VM-closure callee comes back as `Step::Tail` (the
                // resolved arm + args + env, **un-run**) and a native / tree-walked
                // callee comes back executed as `Step::Done(value)`.
                let step = match fast {
                    Some((arm, cenv)) => Step::Tail { compiled: arm, args: argv, genv: cenv },
                    None => match dispatch(heap, callee, argv, true, cur_env) {
                        Ok(s) => s,
                        Err(e) if e.is_control() => {
                            // State-capture suspend (ADR-100 §8): a clean `receive`
                            // raised `Control::Suspend` through the `%receive` native.
                            // Rewind `ip` to re-run THIS call on resume (re-scan the
                            // mailbox); the callee + args are still on the operand stack
                            // (the `Err` path never truncated them), so the re-run reads
                            // them back. Hand the driver a `Suspend` to capture the
                            // continuation. Default-off builds never produce the signal.
                            *ip -= 1;
                            let deadline = match &e.control {
                                Some(crate::error::Control::Suspend { deadline }) => *deadline,
                                None => None,
                            };
                            return Ok(ChunkExit::Suspend { deadline });
                        }
                        Err(e) => return Err(tag_pos(e, pos)),
                    },
                };
                if *tail {
                    // Tail position: hand the call to the driver, which reuses this
                    // frame (TCO). Leftover operands are dropped by the driver
                    // (truncate to `base`).
                    return Ok(match step {
                        Step::Tail { compiled, args, genv } => {
                            ChunkExit::Tail { arm: compiled, args, genv }
                        }
                        Step::Done(v) => ChunkExit::Done(v),
                    });
                }
                match step {
                    // Non-tail call to a chunked VM arm: drop callee+args and hand the
                    // driver a frame to **push** (no native recursion).
                    Step::Tail { compiled, args, genv } => {
                        heap.truncate_roots(n - argc - 1);
                        return Ok(ChunkExit::Call { arm: compiled, args, genv });
                    }
                    // Native / tree-walked callee already ran: push its value and continue.
                    Step::Done(v) => {
                        heap.truncate_roots(n - argc - 1);
                        heap.push_root(v);
                    }
                }
            }
            Inst::SelfCall { argc } => {
                crate::perf_bump!(self_tail);
                // Direct `letrec` self-tail-call: inline the frame reset and all
                // safepoints so we stay inside this `while` loop instead of
                // round-tripping through `vm_run_bc` on every iteration. Critical for
                // tight tail-recursive loops (loop/collatz/fib): eliminates one Rust
                // call-return and a `SmallVec` construction per iteration.
                //
                // Safety ordering: GC runs first (args still rooted on the operand
                // stack), then args are read (relocated values used), then the frame is
                // reset. No collection fires after the args leave the root stack.
                if !crate::process::macro_block_active() && heap.gc_due() {
                    heap.collect(&mut [], &mut []);
                }
                let n = heap.roots_len();
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(*argc);
                for k in 0..*argc {
                    argv.push(heap.root_at(n - argc + k));
                }
                // Reset frame in place (same as the old outer-loop SelfTail handler).
                heap.truncate_roots(base + arm.nslots);
                for i in 0..arm.nslots {
                    heap.set_root_at(base + i, Value::Nil);
                }
                for i in 0..arm.nrequired {
                    heap.set_root_at(base + i, argv[i]);
                }
                *ip = 0;
                if let Some(used) = crate::core::alloc::soft_limit_hit() {
                    return Err(crate::eval::memory_limit_error(used));
                }
                if capture {
                    if crate::process::capture_hard_kill_pending() {
                        return Ok(ChunkExit::Killed);
                    }
                    if crate::process::tick_capture() {
                        // Frame already reset; driver captures the continuation as-is.
                        return Ok(ChunkExit::Preempt);
                    }
                } else {
                    crate::process::tick();
                }
                if crate::process::deadline_exceeded() {
                    return Err(crate::eval::deadline_error());
                }
                // Back-edge tiering: periodically hand a hot self-tail loop to the driver
                // so it can tier. The frame is already reset (ip=0, args in slots), so the
                // driver re-enters this same arm at ip 0. We exit only when there's a
                // reason to: native code is installed (run it — it loops internally), or
                // the arm is still untried (drive `jit_tier`'s counter toward the
                // threshold). While QUEUED (compile in flight) or BAILED we stay inline —
                // no round-trips — just an atomic load every `BACKEDGE_TIER_INTERVAL`.
                #[cfg(feature = "jit")]
                {
                    const BACKEDGE_TIER_INTERVAL: u32 = 256;
                    back_edges = back_edges.wrapping_add(1);
                    if back_edges % BACKEDGE_TIER_INTERVAL == 0 {
                        let code = arm.jit_code.load(std::sync::atomic::Ordering::Acquire);
                        let installed =
                            !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED;
                        if installed || code.is_null() {
                            return Ok(ChunkExit::SelfTail);
                        }
                    }
                }
                // Stay in the inner dispatch loop — no function-call round-trip.
                continue;
            }
            Inst::MakeClosure { fn_rest, names, self_name } => {
                // Mirrors `exec_value`'s `Node::MakeClosure`. The capture values are on
                // the operand stack (pushed by preceding leaf insts — safepoint-free,
                // and alloc here never collects mid-pass), so building the env and the
                // closure is collection-free; `env` stays valid until `make_closure`
                // consumes it. With no captures *and* no self-name the closure is
                // global-capturing; a self-name needs a frame to late-bind into.
                let ncap = names.len();
                let n = heap.roots_len();
                let env = if ncap == 0 && self_name.is_none() {
                    heap.global()
                } else {
                    let frame = heap.new_env(Some(heap.global()));
                    for i in 0..ncap {
                        let v = heap.root_at(n - ncap + i);
                        heap.env_define(frame, names[i], v);
                    }
                    frame
                };
                heap.truncate_roots(n - ncap); // drop the capture values
                let closure = crate::eval::make_closure(heap, None, fn_rest.load(), env)?;
                // Direct `letrec` self-recursion: bind the binder name to the closure
                // in its own captured env (the env↔closure cycle the tracing GC owns).
                if let Some(name) = self_name {
                    heap.env_define(env, *name, closure);
                }
                heap.push_root(closure);
            }
        }
    }
    // The body's value is the lone operand left above the frame.
    let n = heap.roots_len();
    Ok(ChunkExit::Done(heap.root_at(n - 1)))
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
struct BcFrame {
    arm: Arc<CompiledArm>,
    ip: usize,
    base: usize,
    env: EnvRoot,
    env_base: usize,
    arm_slot: usize,
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
    pub(crate) deadline: Option<std::time::Instant>,
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

/// The bytecode driver (ADR-100 Stage 4): run a chunked arm and the **entire chain of
/// chunked calls it makes** on one explicit frame stack, with no native recursion per
/// Brood call. A non-tail call to a chunked arm pushes a frame; a tail call reuses the
/// current frame (TCO); a self-tail-call resets it in place; `Done` pops. Calls to
/// natives / tree-walked arms run inline via `dispatch` (leaves w.r.t. this stack).
/// Every frame's slots live on `Heap::roots` and its env on `Heap::env_roots`, so one
/// safepoint at the loop top relocates the whole stack in place; each frame registers
/// its arm in `live_vm_arms` (hot-reload compaction rewrites every in-flight chunk).
///
/// This is what makes a paused process's continuation **relocatable heap data** — the
/// prerequisite for migrating a running process (concurrency-v2.md §7). `resume` drives
/// state capture (§8, the engine that replaced corosensei): `None` starts `arm0` fresh;
/// `Some(s)` replays a previously [`VmOutcome::Suspended`] continuation from the
/// `%receive` call it parked at, re-entering the loop with `s`'s frame stack (and the
/// operand stack it left on the heap) intact — on **any** worker, no coroutine. A clean
/// `receive` suspend returns `Ok(VmOutcome::Suspended(..))` *without unwinding* (the roots
/// must survive for the resume). The driver runs directly on the worker thread; the
/// continuation lives entirely in `s`, no native stack involved.
fn vm_run_bc(
    heap: &mut Heap,
    arm0: Arc<CompiledArm>,
    args0: &[Value],
    genv0: EnvId,
    resume: Option<Suspended>,
    top_level: bool,
) -> Result<VmOutcome, LispError> {
    crate::perf_bump!(vm_apply);
    // Keep the GC-block depth consistent for any nested native / tree-walked sub-call
    // (their own `stack_overflow_check` reads it). The driver itself doesn't recurse
    // per Brood call — runaway non-tail recursion is caught by `MAX_BC_FRAMES` below,
    // not the native-stack byte guard.
    let _gc_block = crate::process::GcBlockGuard::enter();
    // Publish this driver's `top_level` to the `receive` gate (restored on exit, so the
    // innermost driver wins): a top-level receive captures, a native-nested one blocks.
    struct TopLevelGuard(bool);
    impl Drop for TopLevelGuard {
        fn drop(&mut self) {
            crate::process::set_capture_top_level(self.0);
        }
    }
    let _top_guard = TopLevelGuard(crate::process::set_capture_top_level(top_level));
    // Loop-top **preempt/kill capture** is done only by the *top-level* body driver
    // (`run_process_body`) of a capture-mode green process (ADR-100 §8). A nested
    // `vm_apply` run (a `map`/`try`/`binding` native callback) is NOT top-level: it
    // can't capture a `Preempted`/`Killed` across the native boundary, so it uses the
    // normal `tick`; a `receive` suspend that surfaces there blocks the worker instead
    // (the dirty-scheduler carve-out, §7.4) rather than re-running the native.
    let capture = top_level && crate::process::in_capture_run();

    // Entry marks for a one-shot unwind on error (truncate every frame's roots / env
    // roots / live-arm registrations back to where the driver started). Carried in the
    // `Suspended` so a resumed run still unwinds to the *original* entry on a later error.
    let entry_roots;
    let entry_env;
    let entry_arms;
    // The currently-executing frame is held in locals (not the Vec) so a tail/self
    // loop mutates registers, not the stack — only a non-tail call pushes a `BcFrame`.
    let mut frames: Vec<BcFrame>;
    let mut cur_arm;
    let mut cur_env_base;
    let mut cur_env;
    let mut cur_base;
    let mut cur_arm_slot;
    let mut cur_ip;
    // Fresh start (vs. resuming a parked continuation) — the JIT tiering hook fires only
    // on a fresh arm activation, never mid-receive resume.
    let fresh;
    match resume {
        // Resume a parked continuation: its frame stack + operand roots are still on
        // the heap (the suspend didn't unwind), so restore the registers and re-enter
        // the loop at the `%receive` `Inst::Call` it rewound to — no fresh frame push.
        Some(s) => {
            entry_roots = s.entry_roots;
            entry_env = s.entry_env;
            entry_arms = s.entry_arms;
            frames = s.frames;
            let cur = s.cur;
            cur_arm = cur.arm;
            cur_ip = cur.ip;
            cur_base = cur.base;
            cur_env = cur.env;
            cur_env_base = cur.env_base;
            cur_arm_slot = cur.arm_slot;
            fresh = false;
        }
        // Fresh start: push `arm0`'s activation frame.
        None => {
            entry_roots = heap.roots_len();
            entry_env = heap.env_roots_len();
            entry_arms = heap.live_arm_len();
            frames = Vec::new();
            cur_arm = arm0;
            cur_env_base = heap.env_roots_len();
            cur_env = heap.root_env(genv0);
            cur_base = heap.roots_len();
            cur_arm_slot = if cur_arm.has_runtime_handles {
                heap.live_arm_push(cur_arm.clone())
            } else {
                usize::MAX
            };
            if let Err(e) = push_frame(heap, &cur_arm, args0, cur_env) {
                heap.truncate_roots(entry_roots);
                heap.truncate_env_roots(entry_env);
                heap.live_arm_truncate(entry_arms);
                return Err(e);
            }
            cur_ip = 0usize;
            fresh = true;
        }
    }
    let unwind = |heap: &mut Heap| {
        heap.truncate_roots(entry_roots);
        heap.truncate_env_roots(entry_env);
        heap.live_arm_truncate(entry_arms);
    };

    // JIT tiering hook (ADR-101 1b): on a fresh arm activation whose frame is now set up
    // at `roots[cur_base..]`, give the JIT a chance to run it natively. `Done` (0) → the
    // result is in `roots[cur_base]`; unwind the frame and return it. `deopt`/`preempt`
    // (1/2) or not-hot/out-of-subset (None) → fall through to the interpreter loop with
    // the frame intact (for a preempt the slots hold the partial loop state, so the VM —
    // which preempts at its own loop-top since the budget is already spent — resumes from
    // exactly there). Only the int subset is ever compiled; everything else stays here.
    // JIT tiering (ADR-101 1b): try the native code whenever an arm is (re)entered at
    // ip 0 — a fresh activation, a non-tail call's callee, or a tail call's reused frame.
    // `try_jit` flags such an entry; the check runs at the loop top and produces a
    // `ChunkExit` that flows through the *same* handling as the interpreter's output, so
    // a JIT `Done`/`Tail` retires/reuses the frame identically to the VM. A re-entry via
    // tail call thus re-tiers the callee, and an arm *ending* in a tail call tiers too.
    #[cfg(feature = "jit")]
    let mut try_jit = fresh;
    #[cfg(not(feature = "jit"))]
    let _ = fresh; // silence unused warning when the JIT is off

    loop {
        // Per-iteration safepoint / preemption / deadline — relocates every frame's
        // slots and env in place (all on `Heap::roots`/`env_roots`). Mirrors the
        // `Node` trampoline's loop top.
        if !crate::process::macro_block_active() && heap.gc_due() {
            heap.collect(&mut [], &mut []);
        }
        if let Some(used) = crate::core::alloc::soft_limit_hit() {
            unwind(heap);
            return Err(crate::eval::memory_limit_error(used));
        }
        if capture {
            // State-capture preemption/kill (ADR-100 §8.1), in place of the coroutine
            // yield: the frame boundary is the safepoint. A pending hard `:kill` stops
            // now (no capture — the process is retired and its heap dropped); a hit
            // reduction budget captures the continuation so `run_one` re-enqueues it
            // (on any worker — live migration). Both fire only at this clean loop top.
            if crate::process::capture_hard_kill_pending() {
                return Ok(VmOutcome::Killed);
            }
            if crate::process::tick_capture() {
                let cur = BcFrame {
                    arm: cur_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                };
                return Ok(VmOutcome::Preempted(Suspended {
                    frames,
                    cur,
                    entry_roots,
                    entry_env,
                    entry_arms,
                    deadline: None,
                }));
            }
        } else {
            crate::process::tick();
        }
        if crate::process::deadline_exceeded() {
            unwind(heap);
            return Err(crate::eval::deadline_error());
        }

        // Either run the arm natively (if it's flagged for a tier check) or interpret it.
        // Both yield a `Result<ChunkExit, _>` handled uniformly below.
        let exit = {
            #[cfg(feature = "jit")]
            {
                if try_jit {
                    try_jit = false;
                    match jit_tier(&cur_arm, heap, cur_base, cur_env) {
                        // Done: result in `roots[cur_base]` → the `Done` arm retires it.
                        Some(0) => Ok(ChunkExit::Done(heap.root_at(cur_base))),
                        // A JIT'd call/global errored — propagate the parked error.
                        Some(3) => Err(jit_take_error()
                            .expect("JIT error outcome without a parked error")),
                        // A JIT'd tail call: dispatch the staged callee+args → reuse the
                        // frame (`Tail`) or a finished native callee (`Done`).
                        Some(4) => jit_dispatch_tail(heap, cur_base, &cur_arm, cur_env),
                        // 1 (deopt) / 2 (preempt) / None (not hot / out of subset): run the
                        // arm on the VM with the frame intact (`cur_ip` is still 0).
                        _ => exec_chunk(heap, &cur_arm, &mut cur_ip, cur_base, cur_env, capture),
                    }
                } else {
                    exec_chunk(heap, &cur_arm, &mut cur_ip, cur_base, cur_env, capture)
                }
            }
            #[cfg(not(feature = "jit"))]
            {
                exec_chunk(heap, &cur_arm, &mut cur_ip, cur_base, cur_env, capture)
            }
        };
        match exit {
            Ok(ChunkExit::Done(v)) => {
                // Retire the current frame, then either finish or hand `v` to the caller.
                heap.truncate_roots(cur_base);
                heap.truncate_env_roots(cur_env_base);
                if cur_arm_slot != usize::MAX {
                    heap.live_arm_truncate(cur_arm_slot);
                }
                match frames.pop() {
                    None => return Ok(VmOutcome::Done(v)),
                    Some(caller) => {
                        cur_arm = caller.arm;
                        cur_ip = caller.ip;
                        cur_base = caller.base;
                        cur_env = caller.env;
                        cur_env_base = caller.env_base;
                        cur_arm_slot = caller.arm_slot;
                        // The result lands where the caller pushed the callee — its
                        // operand stack continues seamlessly past the call site.
                        heap.push_root(v);
                    }
                }
            }
            Ok(ChunkExit::Call { arm, args, genv }) => {
                if frames.len() + 1 > MAX_BC_FRAMES {
                    unwind(heap);
                    return Err(crate::eval::stack_depth_error(frames.len()));
                }
                // Suspend the caller (resume at the already-advanced `cur_ip`) and
                // switch the registers to the callee. `exec_chunk` already dropped the
                // callee+args operands, so the callee's frame starts at `roots_len()`.
                let caller_arm = std::mem::replace(&mut cur_arm, arm);
                frames.push(BcFrame {
                    arm: caller_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                });
                cur_env_base = heap.env_roots_len();
                cur_env = heap.root_env(genv);
                cur_base = heap.roots_len();
                cur_arm_slot = if cur_arm.has_runtime_handles {
                    heap.live_arm_push(cur_arm.clone())
                } else {
                    usize::MAX
                };
                if let Err(e) = push_frame(heap, &cur_arm, &args, cur_env) {
                    unwind(heap);
                    return Err(e);
                }
                // The callee frame is set up at `roots[cur_base..]` with `cur_ip = 0`; flag
                // it for a tier check at the loop top (the dominant Brood→Brood path). A
                // native `Done`/`Tail`/error is then handled by the shared arms above —
                // identical to the old inline call-site tiering, minus the duplication.
                cur_ip = 0;
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                }
            }
            Ok(ChunkExit::Tail { arm, args, genv }) => {
                crate::perf_bump!(tail_call);
                // Reuse the current frame for the tail callee (TCO): re-root its env,
                // rebuild its slots in place. Same discipline as the `Node` trampoline.
                heap.truncate_env_roots(cur_env_base);
                cur_env = heap.root_env(genv);
                heap.truncate_roots(cur_base);
                if cur_arm_slot != usize::MAX {
                    heap.live_arm_set(cur_arm_slot, arm.clone());
                } else if arm.has_runtime_handles {
                    cur_arm_slot = heap.live_arm_push(arm.clone());
                }
                if let Err(e) = push_frame(heap, &arm, &args, cur_env) {
                    unwind(heap);
                    return Err(e);
                }
                cur_arm = arm;
                cur_ip = 0;
                // The tail callee occupies a fresh frame at ip 0 — give it a tier check
                // too (whether the tail call came from the VM or a JIT'd arm). This is what
                // lets mutually-recursive arms reached only via tail calls run natively.
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                }
            }
            Ok(ChunkExit::SelfTail) => {
                // Back-edge tiering: a hot self-tail loop handed itself back so we can
                // tier it. The frame is already reset in place (ip=0, the iteration's args
                // in slots) and the operand stack is at the frame top, so we simply
                // re-enter the *same* arm with a tier check — `jit_tier` counts toward the
                // threshold while untried, then runs the installed native code (which
                // loops internally). No frame rebuild; `cur_arm`/`cur_base`/`cur_env` hold.
                cur_ip = 0;
                #[cfg(feature = "jit")]
                {
                    try_jit = true;
                }
            }
            Ok(ChunkExit::Killed) => {
                // Hard kill fired at the inline SelfCall safepoint.
                return Ok(VmOutcome::Killed);
            }
            Ok(ChunkExit::Preempt) => {
                // Reduction budget exhausted at the inline SelfCall safepoint. The frame
                // is already reset (ip=0 inside exec_chunk); capture and re-enqueue.
                let cur = BcFrame {
                    arm: cur_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                };
                return Ok(VmOutcome::Preempted(Suspended {
                    frames,
                    cur,
                    entry_roots,
                    entry_env,
                    entry_arms,
                    deadline: None,
                }));
            }
            Ok(ChunkExit::Suspend { deadline }) => {
                // A clean `receive` parked (ADR-100 §8). `exec_chunk` rewound `cur_ip`
                // to the suspending `%receive` `Inst::Call` and left the callee + args
                // on the operand stack, so the captured continuation replays straight
                // from there. Capture the whole frame stack as `Suspended` and return
                // it WITHOUT unwinding — the operand stack and frame slots must survive
                // on the heap for the resume (a collection while parked relocates them
                // in place; the saved `base`/`env_base` indices stay valid).
                let cur = BcFrame {
                    arm: cur_arm,
                    ip: cur_ip,
                    base: cur_base,
                    env: cur_env,
                    env_base: cur_env_base,
                    arm_slot: cur_arm_slot,
                };
                return Ok(VmOutcome::Suspended(Suspended {
                    frames,
                    cur,
                    entry_roots,
                    entry_env,
                    entry_arms,
                    deadline,
                }));
            }
            Err(e) => {
                unwind(heap);
                return Err(e);
            }
        }
    }
}

// ===================== entry =====================

/// Compile-then-run a resolved top-level `form` — the VM entry the form loops use
/// when `vm_enabled()`. A form built from the core vocabulary runs on the VM (an
/// empty lexical scope: no locals at top level); anything else defers to the
/// tree-walker. `env` is the process's global/root env.
pub fn run(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    let mut scope = Scope::new();
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
                body: node,
                // Top-level forms run via `exec_value` below, not the bytecode loop
                // (Stage 1 bytecode is reached only through `vm_apply`); no chunk.
                chunk: None,
                has_runtime_handles,
                jit_code: AtomicPtr::new(std::ptr::null_mut()),
                jit_calls: AtomicU32::new(0),
                compile_epoch: AtomicU64::new(0),
            });
            let arm_slot = if arm.has_runtime_handles {
                heap.live_arm_push(arm.clone())
            } else {
                usize::MAX
            };
            let env_base = heap.env_roots_len();
            let genv = heap.root_env(env);
            let base = heap.roots_len();
            for _ in 0..scope.max {
                heap.push_root(Value::Nil);
            }
            let r = exec_value(heap, &arm.body, base, genv);
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
    if vm_enabled() {
        apply_value(heap, callee, args, genv)
    } else {
        crate::eval::apply(heap, callee, args, genv)
    }
}

// ===================== JIT lowering (ADR-101 Stage 1) =====================
//
// Lower a chunked arm to native code via Cranelift, co-located here because it reads
// the private `Inst`/`Chunk` bytecode. Stage-1 Step A: the **straight-line int subset**
// — `Const`(Int), `Local`, `Prim2`(Add/Sub/Mul) — keeping operands in SSA registers
// (the operand stack is virtualised at compile time, so `roots` never grows) and
// touching `Heap::roots` only to read frame slots and box the result. Any other `Inst`
// (control flow, calls, non-int prims, globals) makes lowering **bail** (`None`) — the
// arm stays on the VM. Control flow + the self-loop + deopt come next.

#[cfg(feature = "jit")]
static JIT_ARM_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Frame slots reserved for the JIT to **spill call-result handles** that must survive
/// a later call's safepoint — the two-call-recursion shape (`fib`'s
/// `(+ (fib …) (fib …))`, bintree's `check`) where the first call's result is a heap
/// handle sitting in a register below the second call. Spilling it into a GC-visible
/// frame slot (rather than bailing to the VM) lets the arm lower. Reserved iff the arm
/// has ≥2 non-tail calls — the only shape that can leave a handle below a call. The VM
/// never references these slots; `push_frame` nil-inits them like any other. Computed
/// identically at arm construction (to size the frame) and in `jit_lower_arm` (to place
/// spills); if the predicate ever under-counts, the lowering bails safely rather than
/// corrupting. `0` under `--without-jit`, so that build's frames are unchanged.
#[cfg(feature = "jit")]
fn jit_spill_reserve(code: &[Inst]) -> usize {
    let non_tail_calls = code
        .iter()
        .filter(|i| matches!(i, Inst::Call { tail: false, .. }))
        .count();
    if non_tail_calls < 2 {
        return 0;
    }
    // Reserve **only** for arms that are actually JIT-lowerable — every opcode in the
    // integer subset `jit_lower_arm` accepts. The reserve adds a frame slot that the VM
    // nil-inits on every activation, so reserving for an arm that never lowers (a prelude
    // function with out-of-subset ops — `send`/`receive`/`spawn` machinery, string/map
    // work — which the JIT can't compile anyway) is pure dead weight on the interpreter
    // path. Blanket-reserving every ≥2-non-tail-call arm regressed `spawn` ~1.9× (20 000
    // procs paying bloated prelude frames), even under `BROOD_VM=0`. Gating on the subset
    // keeps the reserve on `fib`-shaped arms (which lower and win) and off everything else.
    if !chunk_in_jit_subset(code) {
        return 0;
    }
    // Reserve **only** for the arms that actually lower (and benefit) — see the matching
    // gate in `jit_lower_arm`. A two-call arm that walks a data structure (a `VectorRef`
    // (`nth`) or `first`/`rest`, like bintree's `check`) is *not* lowered (it regresses),
    // so reserve nothing: the spill slots are dead frame weight on the VM path otherwise,
    // and that frame bloat slows the arm's *interpreted* execution too (e.g. 20 000
    // short `fib`s in `spawn`, which don't all reach native) — the cost that made the
    // earlier flat `RESERVE = 4` regress `spawn` ~1.8× regardless of the JIT.
    let walks = code.iter().any(|i| {
        matches!(i, Inst::Prim1 { .. })
            || matches!(i,
                Inst::Prim2 { op: PrimOp::VectorRef, .. }
                | Inst::Prim2SlotSlot { op: PrimOp::VectorRef, .. }
                | Inst::Prim2SlotInt { op: PrimOp::VectorRef, .. })
    });
    if walks {
        return 0;
    }
    // A pure-arithmetic two-call recursion (`fib`'s `(+ (fib …) (fib …))`) spills exactly
    // one live call-result handle: the first call's result waits in a slot across the
    // second call's safepoint, and the `+` of the two results reduces back to an int (so
    // chains like `(+ (f a) (f b) (f c))` still need only one). One slot; arms that would
    // need more bail at translation (`spill_next >= reserve`) and run on the VM.
    1
}
#[cfg(not(feature = "jit"))]
fn jit_spill_reserve(_code: &[Inst]) -> usize {
    0
}

/// True iff every opcode in `code` is in the integer JIT subset — i.e. `jit_lower_arm`
/// could lower this arm (modulo the handle-spill, which is what the reserve enables).
/// Mirrors `jit_lower_arm`'s pre-bail check; the two must stay in sync. Used by
/// [`jit_spill_reserve`] so only genuinely-lowerable arms get spill frame slots.
#[cfg(feature = "jit")]
fn chunk_in_jit_subset(code: &[Inst]) -> bool {
    let in_subset_op = |op: &PrimOp| {
        matches!(
            op,
            PrimOp::Add
                | PrimOp::Sub
                | PrimOp::Mul
                | PrimOp::Lt
                | PrimOp::Le
                | PrimOp::Eq
                | PrimOp::Rem
                | PrimOp::Quot
                | PrimOp::Div
                | PrimOp::Cons
                | PrimOp::VectorRef
        )
    };
    code.iter().all(|inst| match inst {
        Inst::Const(cv) => matches!(cv.load(), Value::Int(_)),
        Inst::Local(_)
        | Inst::Jump(_)
        | Inst::JumpIfFalse(_)
        | Inst::SelfCall { .. }
        | Inst::Pop
        | Inst::SetLocal(_)
        | Inst::Global(_)
        | Inst::GlobalIc { .. }
        | Inst::Prim1 { .. }
        | Inst::Call { .. } => true,
        Inst::Prim2 { op, .. } | Inst::Prim2SlotSlot { op, .. } | Inst::Prim2SlotInt { op, .. } => {
            in_subset_op(op)
        }
        _ => false,
    })
}

/// Compile `arm`'s chunk to a native `extern "C" fn(heap: *mut Heap, base: i64) -> i64`
/// for the Step-A int subset, or `None` to bail to the VM. The compiled fn reads its
/// frame slots from `roots[base..]`, computes in registers, **boxes the result into
/// `roots[base]`**, and returns `0` (Done) or `1` (deopt — an operand wasn't an `Int`).
/// The returned pointer is valid for the life of `jit` (its module owns the code).
#[cfg(feature = "jit")]
pub(crate) fn jit_lower_arm(jit: &mut crate::jit::Jit, arm: &CompiledArm) -> Option<*const u8> {
    use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_BOOL, TAG_INT, TAG_PAIR};
    use cranelift_codegen::ir::{
        condcodes::IntCC, types, AbiParam, BlockArg, InstBuilder, MemFlags, StackSlotData,
        StackSlotKind,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
    use cranelift_module::{Linkage, Module};
    use std::sync::atomic::Ordering;

    let chunk = arm.chunk.as_ref()?;
    let code = &chunk.code;
    let len = code.len();
    const STRIDE: i64 = std::mem::size_of::<Value>() as i64;
    // Handle-spill scratch: `[spill_base, spill_base + reserve)` are the frame slots
    // reserved (above the compiler's slot ceiling) for spilling call-result handles
    // that must survive a later call's safepoint. `reserve` matches what arm
    // construction added to `nslots`, so `spill_base` is exactly the old `scope.max`.
    let reserve = jit_spill_reserve(code);
    let spill_base = arm.nslots - reserve;
    let mut spill_next = 0usize;
    // Return-via-roots writes/reads the result at `roots[base]` (slot 0), and the VM hooks
    // read it back the same way — both require slot 0 to exist. A 0-slot arm (a 0-arg,
    // 0-local fn like `(defn k () 7)`) has `base == roots_len`, so `roots[base]` is out of
    // bounds. Such arms are trivial; bail and let the VM run them.
    if arm.nslots == 0 {
        return None;
    }

    // ---- Pre-bail on any out-of-subset instruction (so we never half-build) ----
    // Subset: Const(Int) / Local / Prim2{,SlotSlot,SlotInt}(Add,Sub,Mul,Lt,Le,Eq) /
    // Jump / JumpIfFalse / SelfCall. The fused `Prim2Slot*` variants are what
    // `emit_node` actually produces for the common `(- i 1)` / `(+ acc i)` loop body
    // (operands that are frame locals or int literals), so the JIT must lower them or
    // it never fires on real compiled code. (Call, Global, the division family, etc.
    // come in later increments.)
    let in_subset_op = |op: &PrimOp| {
        matches!(
            op,
            PrimOp::Add
                | PrimOp::Sub
                | PrimOp::Mul
                | PrimOp::Lt
                | PrimOp::Le
                | PrimOp::Eq
                | PrimOp::Rem
                | PrimOp::Quot
                | PrimOp::Div
                | PrimOp::Cons
                | PrimOp::VectorRef
        )
    };
    for inst in code {
        match inst {
            Inst::Const(cv) if matches!(cv.load(), Value::Int(_)) => {}
            Inst::Local(_)
            | Inst::Jump(_)
            | Inst::JumpIfFalse(_)
            | Inst::SelfCall { .. }
            | Inst::Pop
            | Inst::SetLocal(_)
            // A free-global read (a call's callee, or a value-position global) — resolved
            // live by `brood_rt_global`.
            | Inst::Global(_)
            | Inst::GlobalIc { .. }
            // `first`/`rest` (PrimOp1 is only First/Rest, both lowered).
            | Inst::Prim1 { .. } => {}
            // A Brood→Brood call. Non-tail → `brood_rt_call_slow` (runs the callee inline,
            // result back as a handle). Tail → staged on `roots` + outcome 4, the driver
            // dispatches it and reuses this frame (TCO), then re-tiers the callee.
            Inst::Call { .. } => {}
            // `cons` is supported as the generic `Prim2` and the `(Local, Local)`-fused
            // `Prim2SlotSlot`; the rarer `(_, Const)`-fused `Prim2SlotInt{Cons}` bails at
            // translation (it would need const-operand word materialisation).
            Inst::Prim2 { op, .. }
            | Inst::Prim2SlotSlot { op, .. }
            | Inst::Prim2SlotInt { op, .. }
                if in_subset_op(op) => {}
            _ => {
                return None;
            }
        }
    }

    // ---- Body-weight gate for arms ending in a tail call (jit-tier2.md §6.2). ----
    // A **tail** call returns to the driver (outcome 4) to dispatch the callee and reuse
    // the frame — a per-hop native↔driver round-trip the self-recursive `SelfCall` loop
    // avoids (it loops inside native). That round-trip only pays off once the arm does
    // enough work to outweigh it. Benchmarking mutual recursion puts the crossover at
    // ~3 "work" ops: a 2-op `(if (= n 0) … (g (- n 1)))` ping/pong loop *regresses* ~7%
    // (the native body is too small to amortize the round-trip), a 3-op body is ~neutral,
    // a 5-op body gains ~12%. So an arm containing a tail call must have **≥ 4 work
    // instructions** (arithmetic/list prims + nested non-tail calls) to lower; a thinner
    // one stays on the VM — same speed, no regression. Arms with no tail call are
    // unaffected (no round-trip): a tiny `SelfCall` int loop still tiers (~27× win).
    const TAIL_CALL_MIN_WORK: usize = 4;
    if code.iter().any(|i| matches!(i, Inst::Call { tail: true, .. })) {
        let work = code
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    Inst::Prim1 { .. }
                        | Inst::Prim2 { .. }
                        | Inst::Prim2SlotSlot { .. }
                        | Inst::Prim2SlotInt { .. }
                        | Inst::Call { tail: false, .. }
                )
            })
            .count();
        if work < TAIL_CALL_MIN_WORK {
            return None;
        }
    }

    // ---- Benefit gate for two-call recursion (the handle-spill shape). ----
    // Lowering an arm with ≥2 non-tail calls only pays off when the call/dispatch cost it
    // removes (native-to-native linking, `jit_dispatch_call`) dominates the body — pure
    // integer recursion like `fib`'s `(+ (fib …) (fib …))` (and so `pfib`). When the body
    // also *walks a data structure* — a `VectorRef` (`nth`) or `first`/`rest`, as in
    // bintree's `check` — the per-call linking cost (frame nil-init + the handle-arg copies
    // + the helper calls) outweighs the saving and the arm *regresses* vs the VM's IC'd
    // call path. Such arms stay on the VM (no spill lowering): measured neutral, not slower.
    let non_tail_calls = code
        .iter()
        .filter(|i| matches!(i, Inst::Call { tail: false, .. }))
        .count();
    if non_tail_calls >= 2
        && code.iter().any(|i| {
            matches!(i, Inst::Prim1 { .. })
                || matches!(i,
                    Inst::Prim2 { op: PrimOp::VectorRef, .. }
                    | Inst::Prim2SlotSlot { op: PrimOp::VectorRef, .. }
                    | Inst::Prim2SlotInt { op: PrimOp::VectorRef, .. })
        })
    {
        return None;
    }

    // ---- Block leaders: ip 0, every jump target, the inst after a jump, the `len`
    // "done" block. ----
    let mut is_leader = vec![false; len + 1];
    is_leader[0] = true;
    is_leader[len] = true; // the implicit Done block
    for (ip, inst) in code.iter().enumerate() {
        match inst {
            Inst::Jump(t) | Inst::JumpIfFalse(t) => {
                is_leader[*t] = true;
                if ip + 1 <= len {
                    is_leader[ip + 1] = true;
                }
            }
            // SelfCall jumps back to the loop header (block 0); the inst after it
            // (if any) starts a new (unreachable) block boundary.
            Inst::SelfCall { .. } => {
                if ip + 1 <= len {
                    is_leader[ip + 1] = true;
                }
            }
            _ => {}
        }
    }

    // ---- Operand-stack depth at each leader (abstract interp; all subset stack
    // values are 64-bit-wide, and a comparison `I8` is always consumed by the
    // `JumpIfFalse` in its own block, so it never crosses a boundary). ----
    let mut depth: Vec<Option<i32>> = vec![None; len + 1];
    let mut work = vec![(0usize, 0i32)];
    while let Some((ip, d)) = work.pop() {
        if depth[ip].is_some() {
            continue;
        }
        depth[ip] = Some(d);
        let (mut cur, mut j) = (d, ip);
        loop {
            if j == len {
                break;
            }
            match &code[j] {
                Inst::Jump(t) => {
                    work.push((*t, cur));
                    break;
                }
                Inst::JumpIfFalse(t) => {
                    cur -= 1; // pop the condition
                    work.push((*t, cur));
                    work.push((j + 1, cur));
                    break;
                }
                Inst::SelfCall { argc } => {
                    // Pops argc new args, jumps back to the loop header (block 0).
                    work.push((0, cur - *argc as i32));
                    break;
                }
                Inst::Const(_) | Inst::Local(_) => cur += 1,
                // A global read pushes its resolved value.
                Inst::Global(_) | Inst::GlobalIc { .. } => cur += 1,
                // A **tail** call is terminal — control never falls through it (the arm
                // returns via the driver), so end the walk here. Leaving it as a fall-
                // through would propagate a bogus depth into whatever instruction follows
                // (dead code, or a sibling leader), corrupting that block's param count.
                Inst::Call { tail: true, .. } => break,
                // A non-tail call pops the callee + `argc` args and pushes one result: net `-argc`.
                Inst::Call { argc, .. } => cur -= *argc as i32,
                // Fused prims read their operands from frame slots / a literal, not the
                // operand stack: net push of 1 (unlike the generic `Prim2`'s pop-2-push-1).
                Inst::Prim2SlotSlot { .. } | Inst::Prim2SlotInt { .. } => cur += 1,
                Inst::Prim2 { .. } => cur -= 1, // pop 2, push 1
                // `first`/`rest`: pop the list operand, push the car/cdr — net 0.
                Inst::Prim1 { .. } => {}
                // `let`/`do` plumbing: a binder stores the top into a frame slot, a
                // non-final `do` form discards it — both pop one.
                Inst::Pop | Inst::SetLocal(_) => cur -= 1,
                _ => break, // unreachable (pre-bailed)
            }
            j += 1;
            if is_leader[j] {
                work.push((j, cur));
                break;
            }
        }
    }

    let m = jit.module();
    let ptr_ty = m.target_config().pointer_type();
    let mut sig = m.make_signature();
    sig.params.push(AbiParam::new(ptr_ty)); // heap
    sig.params.push(AbiParam::new(types::I64)); // base (frame index into roots)
    sig.returns.push(AbiParam::new(types::I64)); // outcome: 0 = Done, 1 = deopt, 2 = preempt
    let seq = JIT_ARM_SEQ.fetch_add(1, Ordering::Relaxed);
    let id = m.declare_function(&format!("brood_jit_arm_{seq}"), Linkage::Export, &sig).ok()?;
    let mut rb_sig = m.make_signature();
    rb_sig.params.push(AbiParam::new(ptr_ty));
    rb_sig.returns.push(AbiParam::new(ptr_ty));
    let rb_id = m.declare_function("brood_rt_roots_base", Linkage::Import, &rb_sig).ok()?;
    // brood_rt_tick(heap) -> u8  (nonzero = the process should yield)
    let mut tick_sig = m.make_signature();
    tick_sig.params.push(AbiParam::new(ptr_ty));
    tick_sig.returns.push(AbiParam::new(types::I8));
    let tick_id = m.declare_function("brood_rt_tick", Linkage::Import, &tick_sig).ok()?;
    // The handle ops, by-value with an out-pointer (a `Value` is 24 bytes → no register-pair
    // return): brood_rt_cons(heap, out, car0,car1,car2, cdr0,cdr1,cdr2);
    // brood_rt_{car,cdr}(heap, out, w0,w1,w2). They write the result `Value` to `*out`.
    let mut car_sig = m.make_signature();
    car_sig.params.push(AbiParam::new(ptr_ty)); // heap
    car_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    for _ in 0..3 {
        car_sig.params.push(AbiParam::new(types::I64)); // the operand's 3 words
    }
    let car_id = m.declare_function("brood_rt_car", Linkage::Import, &car_sig).ok()?;
    let cdr_id = m.declare_function("brood_rt_cdr", Linkage::Import, &car_sig).ok()?;
    let mut cons_sig = m.make_signature();
    cons_sig.params.push(AbiParam::new(ptr_ty)); // heap
    cons_sig.params.push(AbiParam::new(ptr_ty)); // out
    for _ in 0..6 {
        cons_sig.params.push(AbiParam::new(types::I64)); // car 3 words + cdr 3 words
    }
    let cons_id = m.declare_function("brood_rt_cons", Linkage::Import, &cons_sig).ok()?;
    // brood_rt_gc_safepoint(heap): collect if due (bounds the nursery for cons loops).
    let mut sp_sig = m.make_signature();
    sp_sig.params.push(AbiParam::new(ptr_ty));
    let sp_id = m.declare_function("brood_rt_gc_safepoint", Linkage::Import, &sp_sig).ok()?;
    // The Brood→Brood call ABI. brood_rt_push(heap, w0,w1,w2): stage one operand `Value`
    // onto `roots`. brood_rt_global(heap, out, sym) -> status: resolve a free global into
    // `*out`. brood_rt_call_slow(heap, out, argc) -> status: dispatch the staged call into
    // `*out`. Status 0 = ok, nonzero = error parked for the arm to propagate.
    let mut push_sig = m.make_signature();
    push_sig.params.push(AbiParam::new(ptr_ty)); // heap
    for _ in 0..3 {
        push_sig.params.push(AbiParam::new(types::I64)); // the operand's 3 words
    }
    let push_id = m.declare_function("brood_rt_push", Linkage::Import, &push_sig).ok()?;
    let mut glob_sig = m.make_signature();
    glob_sig.params.push(AbiParam::new(ptr_ty)); // heap
    glob_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    glob_sig.params.push(AbiParam::new(types::I32)); // sym (interned u32)
    glob_sig.returns.push(AbiParam::new(types::I64)); // status
    let glob_id = m.declare_function("brood_rt_global", Linkage::Import, &glob_sig).ok()?;
    // brood_rt_global_ic(heap, out, sym, site) -> status: as above but through the
    // per-site global inline cache (no `env_get` walk on a cache hit).
    let mut globic_sig = m.make_signature();
    globic_sig.params.push(AbiParam::new(ptr_ty)); // heap
    globic_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    globic_sig.params.push(AbiParam::new(types::I32)); // sym
    globic_sig.params.push(AbiParam::new(types::I32)); // site
    globic_sig.returns.push(AbiParam::new(types::I64)); // status
    let globic_id = m
        .declare_function("brood_rt_global_ic", Linkage::Import, &globic_sig)
        .ok()?;
    let mut callslow_sig = m.make_signature();
    callslow_sig.params.push(AbiParam::new(ptr_ty)); // heap
    callslow_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    callslow_sig.params.push(AbiParam::new(types::I32)); // argc (u32)
    callslow_sig.returns.push(AbiParam::new(types::I64)); // status
    let callslow_id =
        m.declare_function("brood_rt_call_slow", Linkage::Import, &callslow_sig).ok()?;
    // brood_rt_vector_ref(heap, out, vec 3 words, idx 3 words) -> status: bounds-checked
    // slab read into `*out` (0 = ok, 1 = deopt for non-vector / non-int / out-of-range).
    let mut vref_sig = m.make_signature();
    vref_sig.params.push(AbiParam::new(ptr_ty)); // heap
    vref_sig.params.push(AbiParam::new(ptr_ty)); // out: *mut Value
    for _ in 0..6 {
        vref_sig.params.push(AbiParam::new(types::I64)); // vec 3 words + idx 3 words
    }
    vref_sig.returns.push(AbiParam::new(types::I64)); // status
    let vref_id = m.declare_function("brood_rt_vector_ref", Linkage::Import, &vref_sig).ok()?;

    let mut ctx = m.make_context();
    ctx.func.signature = sig;
    let mut fbctx = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
    let rb_ref = m.declare_func_in_func(rb_id, b.func);
    let tick_ref = m.declare_func_in_func(tick_id, b.func);
    let car_ref = m.declare_func_in_func(car_id, b.func);
    let cdr_ref = m.declare_func_in_func(cdr_id, b.func);
    let cons_ref = m.declare_func_in_func(cons_id, b.func);
    let sp_ref = m.declare_func_in_func(sp_id, b.func);
    let push_ref = m.declare_func_in_func(push_id, b.func);
    let glob_ref = m.declare_func_in_func(glob_id, b.func);
    let globic_ref = m.declare_func_in_func(globic_id, b.func);
    let callslow_ref = m.declare_func_in_func(callslow_id, b.func);
    let vref_ref = m.declare_func_in_func(vref_id, b.func);
    // Whether the arm allocates (`cons`) — gates the back-edge GC safepoint that bounds
    // the nursery. (`car`/`rest` don't allocate.)
    let has_cons = code.iter().any(|i| {
        matches!(
            i,
            Inst::Prim2 { op: PrimOp::Cons, .. }
                | Inst::Prim2SlotSlot { op: PrimOp::Cons, .. }
                | Inst::Prim2SlotInt { op: PrimOp::Cons, .. }
        )
    });

    // One Cranelift block per leader (with `depth` I64 params), plus entry/deopt. The
    // Done block (`ip == len`) takes **no** params: the result is returned via
    // `roots[base]` (each exit stores it there), so it can be a handle, not just an
    // `i64` block arg. Every other block carries its operand-stack depth as I64 params.
    let leader_block: Vec<Option<cranelift_codegen::ir::Block>> = (0..=len)
        .map(|ip| {
            if is_leader[ip] {
                let blk = b.create_block();
                let nparams = if ip == len { 0 } else { depth[ip].unwrap_or(0) };
                for _ in 0..nparams {
                    b.append_block_param(blk, types::I64);
                }
                Some(blk)
            } else {
                None
            }
        })
        .collect();
    let entry = b.create_block();
    let deopt = b.create_block();
    let preempt = b.create_block();
    // The error exit (outcome 3): a JIT'd call / global read raised an error (parked in
    // `JIT_PENDING_ERROR`). `vm_run_bc` takes it and propagates — unlike `deopt`, it does
    // **not** re-run the arm on the VM (which would repeat the call).
    let error = b.create_block();
    // The tail-call exit (outcome 4): a JIT'd arm ending in a **tail** call stages the
    // callee + args on `roots` (above the frame top) and returns here. `vm_run_bc` reads
    // the staged operands, dispatches the callee with `tail = true`, and reuses this
    // frame for it (TCO) — never growing the native stack. Only conditionally reached
    // (an arm with no tail call leaves it dead, DCE'd), like `deopt`/`preempt`/`error`.
    let tailcall = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let heap = b.block_params(entry)[0];
    let base = b.block_params(entry)[1];
    // `roots_base` is a **Variable**, not a fixed SSA value: a Brood→Brood call's staging
    // pushes (and the callee's own frames) may reallocate `roots`, so the base is re-fetched
    // after each call (`def_var` below). For a call-free arm it keeps its single entry
    // definition (no phi, no reload) — the int/cons subset is unaffected. Helpers read it
    // via `b.use_var(rb_var)`.
    let rb_var = b.declare_var(ptr_ty);
    let call = b.ins().call(rb_ref, &[heap]);
    b.def_var(rb_var, b.inst_results(call)[0]);
    // A scratch `Value`-sized stack slot the handle / call / global ops write their result
    // into (the out-pointer ABI). One per arm, reused: each result is read straight back
    // into registers before the next op.
    let out_slot = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, STRIDE as u32, 3));
    b.ins().jump(leader_block[0].unwrap(), &[]);

    let is_i64 = |b: &FunctionBuilder, v: cranelift_codegen::ir::Value| {
        b.func.dfg.value_type(v) == types::I64
    };
    // Box an `Op::Int`'s register value into a whole-`Value`'s `(tag_byte, payload_i64)`.
    // An `i64` arithmetic/const/slot value → `Value::Int` (`TAG_INT`, payload as-is). The
    // only *non*-`i64` `Op::Int` is a comparison result (`<`/`<=`/`=`, an `i8` 0/1), and a
    // Brood comparison yields `true`/`false`, **not** the integers 0/1 — so it boxes as a
    // `Value::Bool` (`TAG_BOOL`, the `i8` zero-extended to the payload word). Both payload
    // forms are `i64`, so a materialised operand (a return, a binder, a self-call/call arg)
    // stores / passes correctly. (Without this, returning `(< a b)` produced `Value::Int 1`
    // instead of `true`.)
    let box_scalar = |b: &mut FunctionBuilder,
                      v: cranelift_codegen::ir::Value|
     -> (u8, cranelift_codegen::ir::Value) {
        if b.func.dfg.value_type(v) == types::I64 {
            (TAG_INT, v)
        } else {
            (TAG_BOOL, b.ins().uextend(types::I64, v))
        }
    };
    // Load frame slot `k` as an unboxed `i64`, tag-checking `Int` first: a non-`Int`
    // operand branches to `deopt` (the VM then runs the arm, where the inline path
    // handles the real shape). Leaves `b` switched to the post-check block. Used by
    // `Local` and the fused `Prim2Slot*` operands alike.
    let load_slot_int = |b: &mut FunctionBuilder, k: i64| -> cranelift_codegen::ir::Value {
        let roots_base = b.use_var(rb_var);
        let idx = b.ins().iadd_imm(base, k);
        let off = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(roots_base, off);
        let tag = b.ins().load(types::I8, MemFlags::new(), addr, 0);
        let is_int = b.ins().icmp_imm(IntCC::Equal, tag, TAG_INT as i64);
        let cont = b.create_block();
        b.ins().brif(is_int, cont, &[], deopt, &[]);
        b.switch_to_block(cont);
        b.ins().load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32)
    };
    // `map` reorders the two operands into the primitive's `(x, y)` argument order —
    // e.g. `>` is `%lt` with `map = [1, 0]` (operands swapped), so the JIT must apply
    // it or `(> a b)` would compute `a < b`. `m == 0` picks the first source, else the
    // second. (`emit_node` only ever produces `[0,1]` or `[1,0]` for these prims.)
    let pick = |s0, s1, m: u8| if m == 0 { s0 } else { s1 };
    // Emit `op` on two unboxed `i64` operands already in `(x, y)` order. Add/Sub/Mul use
    // the overflow-checked Cranelift ops and branch to `deopt` on signed overflow — the
    // VM's inline path defers an overflowing `i64` op to the native, which promotes to a
    // BigInt (ADR bignums), so deopting here keeps the JIT bit-identical to the VM
    // instead of silently wrapping. Comparisons yield an `I8` 0/1. Leaves `b` switched
    // to the post-check block for the arithmetic ops.
    let emit_arith = |b: &mut FunctionBuilder,
                      op: PrimOp,
                      x: cranelift_codegen::ir::Value,
                      y: cranelift_codegen::ir::Value|
     -> Option<cranelift_codegen::ir::Value> {
        let checked = |b: &mut FunctionBuilder, r: cranelift_codegen::ir::Value, ov| {
            let cont = b.create_block();
            b.ins().brif(ov, deopt, &[], cont, &[]);
            b.switch_to_block(cont);
            r
        };
        Some(match op {
            PrimOp::Add => {
                let (r, ov) = b.ins().sadd_overflow(x, y);
                checked(b, r, ov)
            }
            PrimOp::Sub => {
                let (r, ov) = b.ins().ssub_overflow(x, y);
                checked(b, r, ov)
            }
            PrimOp::Mul => {
                let (r, ov) = b.ins().smul_overflow(x, y);
                checked(b, r, ov)
            }
            PrimOp::Lt => b.ins().icmp(IntCC::SignedLessThan, x, y),
            PrimOp::Le => b.ins().icmp(IntCC::SignedLessThanOrEqual, x, y),
            PrimOp::Eq => b.ins().icmp(IntCC::Equal, x, y),
            // Integer division family (`rem`/`quot`/`%div`). Cranelift's `sdiv`/`srem`
            // *trap* on a zero divisor and on the `i64::MIN / -1` overflow, so both must
            // be guarded → deopt before the op (the VM's inline path defers exactly these
            // edges to the native, matching). `%div` additionally yields an `Int` only on
            // an exact quotient — a nonzero remainder means a `Float` the native builds,
            // so deopt then too. Operand order is already `(x, y)` (map applied).
            PrimOp::Rem | PrimOp::Quot | PrimOp::Div => {
                let zero = b.ins().iconst(types::I64, 0);
                let div0 = b.ins().icmp(IntCC::Equal, y, zero);
                let c0 = b.create_block();
                b.ins().brif(div0, deopt, &[], c0, &[]);
                b.switch_to_block(c0);
                // (x == i64::MIN) && (y == -1) — the one signed-division overflow.
                let min = b.ins().iconst(types::I64, i64::MIN);
                let neg1 = b.ins().iconst(types::I64, -1);
                let x_min = b.ins().icmp(IntCC::Equal, x, min);
                let y_m1 = b.ins().icmp(IntCC::Equal, y, neg1);
                let ov = b.ins().band(x_min, y_m1);
                let c1 = b.create_block();
                b.ins().brif(ov, deopt, &[], c1, &[]);
                b.switch_to_block(c1);
                match op {
                    PrimOp::Rem => b.ins().srem(x, y),
                    PrimOp::Quot => b.ins().sdiv(x, y),
                    PrimOp::Div => {
                        // Exact only: a nonzero remainder → Float → deopt to the native.
                        let r = b.ins().srem(x, y);
                        let inexact = b.ins().icmp(IntCC::NotEqual, r, zero);
                        let c2 = b.create_block();
                        b.ins().brif(inexact, deopt, &[], c2, &[]);
                        b.switch_to_block(c2);
                        b.ins().sdiv(x, y)
                    }
                    _ => unreachable!(),
                }
            }
            PrimOp::Cons => return None, // allocates — never in the JIT subset
            PrimOp::VectorRef => return None, // heap slab read — not lowered; out of subset
        })
    };

    // ---- The hybrid operand model. ----
    //
    // A logical operand-stack entry is either an unboxed `i64` in an SSA register
    // (`Int` — an arithmetic/const/comparison result, the fast path that keeps tight
    // numeric loops register-resident), or a reference to a frame slot `Slot(k)` whose
    // `Value` lives in `roots[base+k]` — read lazily, type unknown. A `Slot` is the only
    // way a *handle* (a Pair, etc.) can sit on the operand stack: handles must stay in
    // `roots` so the moving collector can see and relocate them (a handle in a register
    // would go stale across a safepoint). Consumers that need an `i64` (arithmetic, a
    // branch condition, a block-arg) materialise a `Slot` with a tag-checked load; ones
    // that move a whole `Value` (a binder, a self-call arg, the return) copy the 16-byte
    // slot verbatim, so a handle round-trips untouched.
    // A third form, `Handle(w0,w1,w2)`, holds a freshly-produced `Value` (a `cons` pair, a
    // `car`/`cdr` result) as its three 24-byte words in registers. It's **transient** —
    // produced and consumed within a block (stored to a slot by a self-call/binder, returned,
    // or tag-checked back to an int), never crossing the loop back-edge live, which is the
    // only safepoint — so the moving GC never sees a handle in a register.
    #[derive(Clone, Copy)]
    enum Op {
        Int(cranelift_codegen::ir::Value),
        Slot(usize),
        Handle(
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
            cranelift_codegen::ir::Value,
        ),
    }
    let done_block = leader_block[len]?;
    // Store an unboxed scalar `Op::Int` value into frame slot `k`, boxing it as `Int` or
    // (for a comparison `i8`) `Bool` via `box_scalar`.
    let store_int = |b: &mut FunctionBuilder, k: i64, v: cranelift_codegen::ir::Value| {
        let (tag_byte, payload) = box_scalar(b, v);
        let roots_base = b.use_var(rb_var);
        let idx = b.ins().iadd_imm(base, k);
        let off = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(roots_base, off);
        let tag = b.ins().iconst(types::I8, tag_byte as i64);
        b.ins().store(MemFlags::new(), tag, addr, 0);
        b.ins().store(MemFlags::new(), payload, addr, PAYLOAD_OFFSET as i32);
    };
    // Copy the whole `Value` from frame slot `src` to slot `dst` (handle-safe — moves the
    // bytes verbatim, no interpretation). A `Value` is `STRIDE` bytes (`#[repr(C, u8)]`):
    // it must copy **every** i64 word, not just tag+payload — `Value::Pid { node, id }`
    // (and any future 2-word-payload variant) carries `id` in the third word at offset 16,
    // which a tag+payload-only copy would drop and corrupt.
    let copy_value = |b: &mut FunctionBuilder, src: i64, dst: i64| {
        let roots_base = b.use_var(rb_var);
        let saddr = {
            let i = b.ins().iadd_imm(base, src);
            let o = b.ins().imul_imm(i, STRIDE);
            b.ins().iadd(roots_base, o)
        };
        let daddr = {
            let i = b.ins().iadd_imm(base, dst);
            let o = b.ins().imul_imm(i, STRIDE);
            b.ins().iadd(roots_base, o)
        };
        let mut off = 0i32;
        while (off as i64) < STRIDE {
            let w = b.ins().load(types::I64, MemFlags::new(), saddr, off);
            b.ins().store(MemFlags::new(), w, daddr, off);
            off += 8;
        }
    };
    // Read an operand as its three `Value` words `[w0, w1, w2]` — for a self-call arg, a
    // binder, a return, or a `cons`/`car`/`cdr` operand. An `Int` boxes to `[Int-tag, v, 0]`
    // (the third word is irrelevant to an Int); a `Slot` loads all three; a `Handle` is
    // already those registers. No tag-check — this moves a whole `Value` verbatim.
    let read_words = |b: &mut FunctionBuilder, op: Op| -> [cranelift_codegen::ir::Value; 3] {
        match op {
            Op::Int(v) => {
                // Box as `Int` or (a comparison `i8`) `Bool`; both payloads are `i64`, so
                // the triple is a valid `[i64; 3]` whole `Value`.
                let (tag_byte, payload) = box_scalar(b, v);
                let tag = b.ins().iconst(types::I64, tag_byte as i64);
                let zero = b.ins().iconst(types::I64, 0);
                [tag, payload, zero]
            }
            Op::Slot(k) => {
                let roots_base = b.use_var(rb_var);
                let i = b.ins().iadd_imm(base, k as i64);
                let o = b.ins().imul_imm(i, STRIDE);
                let addr = b.ins().iadd(roots_base, o);
                let w0 = b.ins().load(types::I64, MemFlags::new(), addr, 0);
                let w1 = b.ins().load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32);
                let w2 = b.ins().load(types::I64, MemFlags::new(), addr, PAYLOAD_OFFSET as i32 + 8);
                [w0, w1, w2]
            }
            Op::Handle(w0, w1, w2) => [w0, w1, w2],
        }
    };
    // Store the three words of a `Value` into frame slot `dst`.
    let store_words = |b: &mut FunctionBuilder, dst: i64, w: [cranelift_codegen::ir::Value; 3]| {
        let roots_base = b.use_var(rb_var);
        let i = b.ins().iadd_imm(base, dst);
        let o = b.ins().imul_imm(i, STRIDE);
        let addr = b.ins().iadd(roots_base, o);
        b.ins().store(MemFlags::new(), w[0], addr, 0);
        b.ins().store(MemFlags::new(), w[1], addr, PAYLOAD_OFFSET as i32);
        b.ins().store(MemFlags::new(), w[2], addr, PAYLOAD_OFFSET as i32 + 8);
    };
    // Materialise an operand to an unboxed `i64`: a register value as-is, a tag-checked
    // load of a frame slot, or a tag-checked extract of a `Handle`'s payload (a `Handle`
    // used as a number — e.g. `(+ (first xs) 1)` — must be an `Int` at runtime or deopt).
    let as_int = |b: &mut FunctionBuilder, op: Op| -> cranelift_codegen::ir::Value {
        match op {
            Op::Int(v) => v,
            Op::Slot(k) => load_slot_int(b, k as i64),
            Op::Handle(w0, w1, _) => {
                let tagb = b.ins().band_imm(w0, 0xff);
                let is_int = b.ins().icmp_imm(IntCC::Equal, tagb, TAG_INT as i64);
                let cont = b.create_block();
                b.ins().brif(is_int, cont, &[], deopt, &[]);
                b.switch_to_block(cont);
                w1
            }
        }
    };
    // Store an operand into frame slot `dst`: an `Int` is boxed; a `Slot` is copied
    // verbatim; a `Handle` stores its three words (so a handle binder / self-call arg /
    // return keeps its type).
    let store_op = |b: &mut FunctionBuilder, dst: i64, op: Op| match op {
        Op::Int(v) => store_int(b, dst, v),
        Op::Slot(k) => copy_value(b, k as i64, dst),
        Op::Handle(w0, w1, w2) => store_words(b, dst, [w0, w1, w2]),
    };
    // Return-via-roots: place the single result in `roots[base]` and jump to the
    // param-less Done block. The result is a whole `Value`, so it can be a handle.
    let exit_done = |b: &mut FunctionBuilder, op: Op| {
        store_op(b, 0, op);
        b.ins().jump(done_block, &[]);
    };
    // Call a handle op (`brood_rt_{cons,car,cdr}`) with the out-pointer ABI: pass the
    // scratch slot's address + the operand words, then read the result `Value`'s three
    // words back into a `Handle`. The result rides in registers only until it's consumed
    // (a store / return) — no safepoint in between — so the GC never sees it.
    let call_handle = |b: &mut FunctionBuilder,
                       fref: cranelift_codegen::ir::FuncRef,
                       operands: &[cranelift_codegen::ir::Value]|
     -> Op {
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let mut args = Vec::with_capacity(operands.len() + 2);
        args.push(heap);
        args.push(out_addr);
        args.extend_from_slice(operands);
        b.ins().call(fref, &args);
        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
        let w1 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        Op::Handle(w0, w1, w2)
    };
    // `vector-ref` / inlined `nth`: a bounds-checked slab read via the runtime helper.
    // On status≠0 (non-vector / non-int / out-of-range) it branches to `deopt`, so the
    // VM owns the exact result (`vector-ref`'s error, `nth`'s default); otherwise the
    // element rides back as a `Handle`. The helper never allocates, so the handle is
    // safe to hold until its immediate consumer.
    let vector_ref = |b: &mut FunctionBuilder,
                      vec: [cranelift_codegen::ir::Value; 3],
                      idx: [cranelift_codegen::ir::Value; 3]|
     -> Op {
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let c = b.ins().call(
            vref_ref,
            &[heap, out_addr, vec[0], vec[1], vec[2], idx[0], idx[1], idx[2]],
        );
        let status = b.inst_results(c)[0];
        let cont = b.create_block();
        b.ins().brif(status, deopt, &[], cont, &[]);
        b.switch_to_block(cont);
        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
        let w1 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        Op::Handle(w0, w1, w2)
    };

    // Translate each leader block in ip order.
    for ip in 0..len {
        let Some(blk) = leader_block[ip] else { continue };
        b.switch_to_block(blk);
        let mut stack: Vec<Op> = b.block_params(blk).iter().map(|&v| Op::Int(v)).collect();
        let mut j = ip;
        loop {
            match &code[j] {
                Inst::Const(cv) => {
                    let Value::Int(n) = cv.load() else { return None };
                    stack.push(Op::Int(b.ins().iconst(types::I64, n)));
                }
                // Lazy: push a reference to the frame slot. Consumers tag-check it to an int
                // (arithmetic / a branch) or copy it whole (a binder / arg / return), so a
                // handle in the slot rides along untouched.
                Inst::Local(i) => stack.push(Op::Slot(*i)),
                // A free-global read (a call's callee, or a value-position global). A
                // `GlobalIc` resolves through the per-`site` global inline cache
                // (`brood_rt_global_ic` — a cached read on a process-global env, no `env_get`
                // walk per call; this is what keeps a hot recursive callee like `fib`
                // resolving itself cheaply). A bare `Global` (no site) falls back to
                // `brood_rt_global`. Late binding holds via the cache's epoch stamp; an
                // unbound symbol parks an error and exits via `error` (outcome 3). The
                // resolved value is an arbitrary `Value`, so it's a `Handle`.
                Inst::Global(_) | Inst::GlobalIc { .. } => {
                    let sym_val = match &code[j] {
                        Inst::Global(s) | Inst::GlobalIc { sym: s, .. } => *s,
                        _ => unreachable!(),
                    };
                    let sym = b.ins().iconst(types::I32, sym_val as i64);
                    let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
                    let c = match &code[j] {
                        Inst::GlobalIc { site, .. } => {
                            let site_v = b.ins().iconst(types::I32, *site as i64);
                            b.ins().call(globic_ref, &[heap, out_addr, sym, site_v])
                        }
                        _ => b.ins().call(glob_ref, &[heap, out_addr, sym]),
                    };
                    let status = b.inst_results(c)[0];
                    let cont = b.create_block();
                    b.ins().brif(status, error, &[], cont, &[]);
                    b.switch_to_block(cont);
                    let w0 = b.ins().stack_load(types::I64, out_slot, 0);
                    let w1 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
                    let w2 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
                    stack.push(Op::Handle(w0, w1, w2));
                }
                Inst::Call { argc, tail, .. } => {
                    let argc = *argc;
                    // The call is a safepoint (the callee runs arbitrary Brood and may GC).
                    // A live `Handle` left on the operand stack BELOW the call's own operands
                    // would be a heap pointer in a register across the collection → stale.
                    // `Slot`/`Int` are safe (a slot lives in `roots`, GC-visible; an int is
                    // not a handle). So **spill** each deeper `Handle` into a reserved frame
                    // slot (GC-visible, relocated correctly by the callee's safepoint) and
                    // replace it with that `Slot` — this is what lets two-call recursion
                    // (`(+ (fib …) (fib …))`, bintree `check`) lower instead of bailing. The
                    // store writes the handle's three words into the frame *before* any
                    // `brood_rt_push` (which may realloc `roots`), so the read-all-then-stage
                    // discipline below is preserved. Out of reserved slots → bail to the VM.
                    let below = stack.len().checked_sub(argc + 1)?;
                    for d in 0..below {
                        if matches!(stack[d], Op::Handle(..)) {
                            if spill_next >= reserve {
                                return None;
                            }
                            let slot = spill_base + spill_next;
                            spill_next += 1;
                            store_op(&mut b, slot as i64, stack[d]);
                            stack[d] = Op::Slot(slot);
                        }
                    }
                    // Pop callee + args (callee deepest), then read **every** operand's words
                    // into registers BEFORE staging — a `brood_rt_push` may reallocate
                    // `roots`, so no slot read may run after a push (it would use a stale
                    // base; this is the same read-all-then-store discipline as `SelfCall`).
                    let mut ops: Vec<Op> = Vec::with_capacity(argc + 1);
                    for _ in 0..(argc + 1) {
                        ops.push(stack.pop()?);
                    }
                    ops.reverse(); // ops[0] = callee, ops[1..] = args in source order
                    let mut worded: Vec<[cranelift_codegen::ir::Value; 3]> =
                        Vec::with_capacity(ops.len());
                    for &op in &ops {
                        worded.push(read_words(&mut b, op));
                    }
                    // Stage `[callee, arg0 .. arg_{argc-1}]` (the VM's `Inst::Call` layout
                    // that `brood_rt_call_slow` / `jit_dispatch_tail` read back).
                    for w in &worded {
                        b.ins().call(push_ref, &[heap, w[0], w[1], w[2]]);
                    }
                    if *tail {
                        // Tail position: the staged call *is* this arm's result (TCO). It
                        // ends the block — nothing may remain on the operand stack below it
                        // (a real tail call's stack is exactly `[callee, args]`). Return
                        // outcome 4; `vm_run_bc` dispatches the staged call with `tail =
                        // true` and reuses this frame, so the native stack never grows.
                        if !stack.is_empty() {
                            return None;
                        }
                        b.ins().jump(tailcall, &[]);
                        break;
                    }
                    // Non-tail: dispatch through the interpreter inline (a safepoint):
                    // result → `out_slot`, status in a register.
                    let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
                    let argc_v = b.ins().iconst(types::I32, argc as i64);
                    let c = b.ins().call(callslow_ref, &[heap, out_addr, argc_v]);
                    let status = b.inst_results(c)[0];
                    // The callee grew/relocated `roots`; re-fetch the base for later slots.
                    let rbc = b.ins().call(rb_ref, &[heap]);
                    let new_base = b.inst_results(rbc)[0];
                    b.def_var(rb_var, new_base);
                    let cont = b.create_block();
                    b.ins().brif(status, error, &[], cont, &[]);
                    b.switch_to_block(cont);
                    let w0 = b.ins().stack_load(types::I64, out_slot, 0);
                    let w1 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
                    let w2 = b.ins().stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
                    stack.push(Op::Handle(w0, w1, w2));
                }
                Inst::Pop => {
                    // A non-final `do` form, evaluated for effect: drop its value.
                    stack.pop()?;
                }
                Inst::SetLocal(i) => {
                    // A `let`/`letrec` binder → frame slot `i`. A `Slot` operand (possibly a
                    // handle) is copied verbatim; an `Int` is boxed as `Int`, a comparison
                    // `i8` as `Bool` (`store_op`/`box_scalar`). let-slots are scratch,
                    // distinct from the loop-carried param slots and dominated by this store,
                    // so a deopt's VM re-run recomputes the binding before any read sees a
                    // stale slot.
                    let op = stack.pop()?;
                    store_op(&mut b, *i as i64, op);
                }
                Inst::Prim1 { op, .. } => {
                    // `first`/`rest`: pop the list operand, tag-check it's a `Pair` (deopt to
                    // the VM otherwise — which handles `first`/`rest` of nil / a non-list /
                    // the type error), then read its car/cdr via the runtime op. The result
                    // is an arbitrary `Value`, so it's a `Handle`.
                    let operand = stack.pop()?;
                    let [w0, w1, w2] = read_words(&mut b, operand);
                    let tagb = b.ins().band_imm(w0, 0xff);
                    let is_pair = b.ins().icmp_imm(IntCC::Equal, tagb, TAG_PAIR as i64);
                    let cont = b.create_block();
                    b.ins().brif(is_pair, cont, &[], deopt, &[]);
                    b.switch_to_block(cont);
                    let fref = match op {
                        PrimOp1::First => car_ref,
                        PrimOp1::Rest => cdr_ref,
                    };
                    let h = call_handle(&mut b, fref, &[w0, w1, w2]);
                    stack.push(h);
                }
                Inst::Prim2 { op, map, .. } => {
                    // Operands were pushed in source order: `aa` (deeper) is source 0,
                    // `bb` (top) is source 1.
                    let (bb_op, aa_op) = (stack.pop()?, stack.pop()?);
                    if matches!(op, PrimOp::Cons) {
                        // `cons` takes any operands and allocates: car = source 0, cdr =
                        // source 1 (cons's `map` is `[0,1]`). Read each as words, alloc.
                        let car = read_words(&mut b, aa_op);
                        let cdr = read_words(&mut b, bb_op);
                        let h = call_handle(
                            &mut b,
                            cons_ref,
                            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
                        );
                        stack.push(h);
                    } else if matches!(op, PrimOp::VectorRef) {
                        // `(vector-ref v i)` / inlined `(nth v i)`: map is `[0,1]`, so
                        // source 0 (`aa`) is the vector, source 1 (`bb`) the index.
                        let vec = read_words(&mut b, aa_op);
                        let idx = read_words(&mut b, bb_op);
                        let h = vector_ref(&mut b, vec, idx);
                        stack.push(h);
                    } else {
                        // Arithmetic/comparison: materialise to int, apply `map`.
                        let aa = as_int(&mut b, aa_op);
                        let bb = as_int(&mut b, bb_op);
                        let x = pick(aa, bb, map[0]);
                        let y = pick(aa, bb, map[1]);
                        stack.push(Op::Int(emit_arith(&mut b, *op, x, y)?));
                    }
                }
                Inst::Prim2SlotSlot { op, map, slot_a, slot_b, .. } => {
                    if matches!(op, PrimOp::Cons) {
                        // `(cons slot_a slot_b)`: car = slot_a, cdr = slot_b (map `[0,1]`).
                        let car = read_words(&mut b, Op::Slot(*slot_a));
                        let cdr = read_words(&mut b, Op::Slot(*slot_b));
                        let h = call_handle(
                            &mut b,
                            cons_ref,
                            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
                        );
                        stack.push(h);
                    } else if matches!(op, PrimOp::VectorRef) {
                        // `(nth slot_a slot_b)`: source 0 = vector slot, source 1 = index
                        // slot (map `[0,1]`). Read each as a full `Value`, then slab-read.
                        let vec = read_words(&mut b, Op::Slot(*slot_a));
                        let idx = read_words(&mut b, Op::Slot(*slot_b));
                        let h = vector_ref(&mut b, vec, idx);
                        stack.push(h);
                    } else {
                        // Source 0 = slot_a, source 1 = slot_b (the VM's `[sa, sb]` order).
                        let sa = load_slot_int(&mut b, *slot_a as i64);
                        let sb = load_slot_int(&mut b, *slot_b as i64);
                        let x = pick(sa, sb, map[0]);
                        let y = pick(sa, sb, map[1]);
                        stack.push(Op::Int(emit_arith(&mut b, *op, x, y)?));
                    }
                }
                Inst::Prim2SlotInt { op, map, slot_a, int_b, .. } => {
                    // A const-index `vector-ref`/`nth` (`(nth v 0)`) fuses here; it's rare
                    // (matmul-style loops use a variable index) — bail to the VM rather than
                    // materialise the const operand as a `Value` word-triple.
                    if matches!(op, PrimOp::VectorRef) {
                        return None;
                    }
                    // Source 0 = slot_a, source 1 = the literal `int_b` (the fusion of
                    // `(Const, Local)` already inverted `map` so the slot is source 0).
                    let sa = load_slot_int(&mut b, *slot_a as i64);
                    let sb = b.ins().iconst(types::I64, *int_b);
                    let x = pick(sa, sb, map[0]);
                    let y = pick(sa, sb, map[1]);
                    stack.push(Op::Int(emit_arith(&mut b, *op, x, y)?));
                }
                Inst::Jump(t) => {
                    if *t == len {
                        // Jump straight to Done: return the single result via roots[base].
                        if stack.len() == 1 {
                            exit_done(&mut b, stack[0]);
                        } else {
                            // A reachable Done always leaves exactly one value, so a
                            // different stack height here means this block is **dead** — the
                            // bytecode compiler emits a jump-past-the-`else` after a branch
                            // that ended in a tail `SelfCall` (which never falls through), so
                            // it can't run. Terminate it by routing to `deopt`: never
                            // executes, and if the unreachability assumption were ever wrong
                            // it safely falls back to the VM rather than mis-returning. (This
                            // dead jump is why e.g. `collatz`'s `steps` arm wouldn't lower.)
                            b.ins().jump(deopt, &[]);
                        }
                    } else {
                        let args: Vec<BlockArg> =
                            stack.iter().map(|&op| BlockArg::Value(as_int(&mut b, op))).collect();
                        b.ins().jump(leader_block[*t]?, &args);
                    }
                    break;
                }
                Inst::SelfCall { argc } => {
                    // Tail self-call (loop back-edge): pop the argc new args and write them
                    // into frame slots `0..argc`. Read every arg's `Value` into registers
                    // FIRST, then store — an arg may reference a slot being overwritten
                    // (e.g. `(f b a)`), so a read-as-you-store would alias. The reads are
                    // safepoint-free, so even a handle's bits are safe in a register here.
                    let mut ops = Vec::with_capacity(*argc);
                    for _ in 0..*argc {
                        ops.push(stack.pop()?);
                    }
                    ops.reverse(); // ops[i] = the i-th positional arg → frame slot i
                    if !stack.is_empty() {
                        return None;
                    }
                    // Each arg becomes a list of (byte-offset, word) stores. An `Int` is
                    // boxed (tag at 0, payload at PAYLOAD_OFFSET — the third word is left
                    // alone, irrelevant to an Int). A `Slot` copies **every** word of the
                    // `Value` (tag/payload/…) so a handle — including a `Pid` whose `id` is
                    // the third word at offset 16 — moves intact.
                    let mut vals: Vec<Vec<(i32, cranelift_codegen::ir::Value)>> =
                        Vec::with_capacity(*argc);
                    for &op in &ops {
                        match op {
                            Op::Int(v) => {
                                // Box as `Int`, or (a comparison `i8`) `Bool` — a loop can
                                // carry a boolean arg.
                                let (tag_byte, payload) = box_scalar(&mut b, v);
                                let tag = b.ins().iconst(types::I8, tag_byte as i64);
                                vals.push(vec![(0, tag), (PAYLOAD_OFFSET as i32, payload)]);
                            }
                            Op::Slot(k) => {
                                let roots_base = b.use_var(rb_var);
                                let i = b.ins().iadd_imm(base, k as i64);
                                let o = b.ins().imul_imm(i, STRIDE);
                                let addr = b.ins().iadd(roots_base, o);
                                let mut words = Vec::new();
                                let mut off = 0i32;
                                while (off as i64) < STRIDE {
                                    words.push((off, b.ins().load(types::I64, MemFlags::new(), addr, off)));
                                    off += 8;
                                }
                                vals.push(words);
                            }
                            // A freshly-produced handle (cons/car/cdr result): its three
                            // words are already in registers — store all three.
                            Op::Handle(w0, w1, w2) => {
                                vals.push(vec![
                                    (0, w0),
                                    (PAYLOAD_OFFSET as i32, w1),
                                    (PAYLOAD_OFFSET as i32 + 8, w2),
                                ]);
                            }
                        }
                    }
                    let roots_base = b.use_var(rb_var);
                    for (i, words) in vals.iter().enumerate() {
                        let idx = b.ins().iadd_imm(base, i as i64);
                        let o = b.ins().imul_imm(idx, STRIDE);
                        let addr = b.ins().iadd(roots_base, o);
                        for &(off, w) in words {
                            b.ins().store(MemFlags::new(), w, addr, off);
                        }
                    }
                    // GC safepoint (cons-allocating arms only): bound the nursery over loop
                    // iterations. Placed here — args already stored to slots, operand stack
                    // empty — so no handle is live in a register across the collection; the
                    // collector relocates the frame slots in place, leaving `roots_base`
                    // valid. (`car`/`rest` don't allocate, so non-cons arms skip it.)
                    if has_cons {
                        b.ins().call(sp_ref, &[heap]);
                    }
                    // Preemption (ADR-027): poll the reduction budget on the back-edge. On
                    // yield, deopt to `preempt` (return 2) — the frame slots already hold the
                    // next iteration's args (in `roots`), so the driver resumes on the VM.
                    let tc = b.ins().call(tick_ref, &[heap]);
                    let yld = b.inst_results(tc)[0];
                    b.ins().brif(yld, preempt, &[], leader_block[0]?, &[]);
                    break;
                }
                Inst::JumpIfFalse(t) => {
                    let cond = stack.pop()?;
                    let tgt = leader_block[*t]?; // falsy → else
                    let fall = leader_block[j + 1]?; // truthy → fall-through
                    // A comparison `I8` branches; any other value is always truthy in Brood
                    // — but a `Slot` condition must still be tag-checked to an `Int` (a
                    // non-int local condition, e.g. `nil`/`false`, deopts to the VM, matching
                    // the eager-int behaviour the JIT had before lazy slots).
                    let brif_cond = match cond {
                        // A comparison result (`I8`) is the only thing we branch on.
                        Op::Int(v) if !is_i64(&b, v) => Some(v),
                        // Anything else is a raw value, always truthy in Brood — but a
                        // `Slot`/`Handle` is tag-checked to an `Int` first (a non-int local
                        // condition, e.g. `nil`/`false`, deopts to the VM, matching the
                        // eager-int behaviour the JIT had before lazy slots).
                        other => {
                            let _ = as_int(&mut b, other);
                            None
                        }
                    };
                    let args: Vec<BlockArg> =
                        stack.iter().map(|&op| BlockArg::Value(as_int(&mut b, op))).collect();
                    match brif_cond {
                        Some(c) => {
                            b.ins().brif(c, fall, &args, tgt, &args);
                        }
                        None => {
                            b.ins().jump(fall, &args);
                        }
                    }
                    break;
                }
                _ => return None,
            }
            j += 1;
            if j == len {
                // Fall off the end into Done: return the single result via roots[base].
                if stack.len() != 1 {
                    return None;
                }
                exit_done(&mut b, stack[0]);
                break;
            }
            if is_leader[j] {
                let args: Vec<BlockArg> =
                    stack.iter().map(|&op| BlockArg::Value(as_int(&mut b, op))).collect();
                b.ins().jump(leader_block[j]?, &args);
                break;
            }
        }
    }

    // Done block: the result was already stored into `roots[base]` by the exiting block
    // (return-via-roots, see `exit_done`), so this just signals normal completion.
    b.switch_to_block(done_block);
    let zero = b.ins().iconst(types::I64, 0);
    b.ins().return_(&[zero]);
    // Deopt: an operand wasn't an Int — return 1, the caller runs the arm on the VM.
    b.switch_to_block(deopt);
    let one = b.ins().iconst(types::I64, 1);
    b.ins().return_(&[one]);
    // Preempt: the reduction budget was spent at a back-edge — return 2. The frame slots
    // (in roots) hold the next iteration's args, so the driver resumes the arm on the VM.
    b.switch_to_block(preempt);
    let two = b.ins().iconst(types::I64, 2);
    b.ins().return_(&[two]);
    // Error: a JIT'd call / global read raised — return 3. The error is parked in
    // `JIT_PENDING_ERROR`; `vm_run_bc` takes it and propagates (no VM re-run).
    b.switch_to_block(error);
    let three = b.ins().iconst(types::I64, 3);
    b.ins().return_(&[three]);
    // Tail call: the callee + args are staged on `roots` — return 4. `vm_run_bc`
    // dispatches them with `tail = true` and reuses this frame (`jit_dispatch_tail`).
    b.switch_to_block(tailcall);
    let four = b.ins().iconst(types::I64, 4);
    b.ins().return_(&[four]);
    b.seal_all_blocks();
    b.finalize();

    m.define_function(id, &mut ctx).ok()?;
    m.clear_context(&mut ctx);
    m.finalize_definitions().ok()?;
    Some(m.get_finalized_function(id))
}

/// The background JIT compiler (ADR-101 1b). A single dedicated OS thread, lazily spawned,
/// is the **only** place arms are lowered: it owns the sole mutable access to the JIT
/// module via [`GLOBAL_JIT`](crate::jit::GLOBAL_JIT), so that lock is otherwise
/// uncontended. Worker threads never compile — they hand a hot arm here and keep running
/// the VM until the native pointer is installed.
///
/// This is the fix for the scheduler-starvation flake: compiling Cranelift IR is
/// CPU-bound work of unbounded-ish duration, and doing it inline on a worker thread (while
/// holding `GLOBAL_JIT`) stalls that worker — during a compile burst the whole pool
/// serializes on the lock, and any process waiting on a tight timer (`(after ms …)`,
/// monitor `:down` delivery) can miss its deadline. Moving compilation off the workers
/// decouples scheduler responsiveness from codegen entirely.
///
/// The channel is bounded so a pathological burst can't grow it without limit; on a full
/// queue the enqueue is dropped and the arm reset to "untried" (it re-tiers later). The
/// thread is detached and lives for the process; sends after a (theoretical) hangup are
/// swallowed.
#[cfg(feature = "jit")]
static JIT_COMPILER: std::sync::LazyLock<std::sync::mpsc::SyncSender<Arc<CompiledArm>>> =
    std::sync::LazyLock::new(|| {
        use std::sync::atomic::Ordering::Release;
        let (tx, rx) = std::sync::mpsc::sync_channel::<Arc<CompiledArm>>(256);
        std::thread::Builder::new()
            .name("brood-jit".into())
            .spawn(move || {
                // If codegen ever *panics* (a Cranelift verifier/finalize failure, e.g. an
                // unregistered `brood_rt_*` symbol, or any future lowering bug), don't let
                // the panic kill this thread — that would abandon the receiver, fill the
                // bounded queue, and silently disable the JIT process-wide while the program
                // ran on none the wiser. Catch it, mark the offending arm BAILED, and stop
                // compiling further (the module may be left half-mutated, so subsequent
                // compiles can't be trusted): the process keeps running, correctly, on the
                // interpreter. A single panic still prints once via the default hook — a
                // loud, actionable signal — but doesn't spam or crash.
                let mut codegen_poisoned = false;
                for arm in rx {
                    if codegen_poisoned {
                        arm.jit_code.store(crate::jit::BAILED, Release);
                        continue;
                    }
                    let mut jit = crate::jit::GLOBAL_JIT.lock().unwrap_or_else(|e| e.into_inner());
                    let lowered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        jit_lower_arm(&mut jit, &arm)
                    }));
                    drop(jit); // install the pointer outside the module lock
                    match lowered {
                        Ok(Some(ptr)) => arm.jit_code.store(ptr as *mut u8, Release),
                        Ok(None) => arm.jit_code.store(crate::jit::BAILED, Release),
                        Err(_) => {
                            codegen_poisoned = true;
                            arm.jit_code.store(crate::jit::BAILED, Release);
                        }
                    }
                }
            })
            .expect("spawn brood-jit compiler thread");
        tx
    });

/// Are all the arm chunk's inlined 2-ary primitives still bound to their native
/// implementations (ADR-096 §4.A epoch-guard, evaluated eagerly)? The JIT lowers
/// `+`/`<`/… to raw machine ops, which is sound only while the head symbol resolves to
/// the matching `%`-native (and arg-map). A `(def + …)` rebinds it; [`resolve_prim`]
/// reads the live global env, so this returns `false` for the redefined operator and
/// the arm must stay on the VM (which dispatches to the new definition). Non-prim
/// instructions can't be invalidated, so they pass. A chunkless arm passes here and is
/// bailed by [`jit_lower_arm`] instead.
#[cfg(feature = "jit")]
fn chunk_ops_all_native(heap: &Heap, arm: &CompiledArm) -> bool {
    let Some(chunk) = arm.chunk.as_ref() else { return true };
    chunk.code.iter().all(|inst| match inst {
        Inst::Prim2 { op, map, head, .. } | Inst::Prim2SlotSlot { op, map, head, .. } => {
            // These store the head's *natural* arg-map (what `resolve_prim` returns).
            matches!(
                resolve_prim(heap, *head),
                Some((o, m)) if o == *op && m == [map[0] as usize, map[1] as usize]
            )
        }
        Inst::Prim2SlotInt { op, map, head, swapped, .. } => {
            // A `(Const, Local)` fusion inverts the map so the slot is operand 0 (and sets
            // `swapped`). Un-invert before comparing to `resolve_prim`'s natural map —
            // otherwise a commutative `(op const local)` like `(* 3 m)` spuriously fails
            // this check and the whole (valid) arm is wrongly marked BAILED, never JITs.
            // Mirrors the revalidation in `prim2_inline_exec`.
            let want = if *swapped {
                [1 - map[0] as usize, 1 - map[1] as usize]
            } else {
                [map[0] as usize, map[1] as usize]
            };
            matches!(resolve_prim(heap, *head), Some((o, m)) if o == *op && m == want)
        }
        _ => true,
    })
}

#[cfg(feature = "jit")]
thread_local! {
    /// The env of the JIT'd arm currently executing. The compiled `fn(heap, base)`
    /// carries no env, but a Brood→Brood call needs the caller's env (to resolve a
    /// free-global callee and as the `genv` for a native callee), so [`jit_tier`] sets
    /// this around each native-arm entry. **Save/restored** there so it stays correct
    /// under re-entry — a JIT'd callee whose body itself enters another JIT'd arm.
    static JIT_CALL_ENV: std::cell::Cell<EnvRoot> =
        const { std::cell::Cell::new(EnvRoot::Stable(EnvId::GLOBAL)) };
    /// An error raised inside a JIT runtime callback (a callee that errored, an unbound
    /// global). The C ABI can't return a `Value` *and* an error, so the callback parks
    /// it here and returns a nonzero status; the compiled arm returns the error outcome
    /// (`3`) and [`vm_run_bc`] takes this to propagate.
    static JIT_PENDING_ERROR: std::cell::RefCell<Option<LispError>> =
        const { std::cell::RefCell::new(None) };
    /// Native-to-native call recursion depth (the linking fast path in
    /// [`jit_dispatch_call`]). A JIT'd arm's non-tail call to a JIT'd callee runs the
    /// callee on the **native** stack (one Rust frame chain per level), not the VM's
    /// heap-frame driver loop, so [`MAX_BC_FRAMES`] — which bounds that loop — does not
    /// bound it. Left unguarded, deep non-tail recursion (`(+ (f …) (f …))`) overflows
    /// the native stack and aborts. This counter caps the native depth and parks a
    /// [`stack_depth_error`](crate::eval::stack_depth_error) instead — the clean error
    /// the VM raises for the same runaway. Save/restored around each native entry.
    static JIT_NATIVE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Set while a native-recursion subtree has exceeded [`JIT_NATIVE_DEPTH`]'s cap and is
    /// being drained on the VM instead. [`jit_tier`] reads it and declines to run native
    /// (returns `None` → interpret), so the rest of the recursion stays in [`vm_run_bc`]'s
    /// heap-frame driver loop — bounded by [`MAX_BC_FRAMES`] on the *heap*, not the native
    /// stack. This keeps deep non-tail recursion working (as it does on the pure VM) rather
    /// than aborting. Save/restored around the over-cap dispatch in [`jit_dispatch_call`].
    static JIT_FORCE_VM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Take the error a JIT runtime callback parked (see [`JIT_PENDING_ERROR`]) — called by
/// [`vm_run_bc`] on the error outcome.
#[cfg(feature = "jit")]
pub(crate) fn jit_take_error() -> Option<LispError> {
    JIT_PENDING_ERROR.with(|c| c.borrow_mut().take())
}

/// Resolve free global `sym` in the executing JIT'd arm's env — the callee-loading
/// `Inst::Global`/`GlobalIc` lowering (and a global read in value position). Returns the
/// value, or parks an unbound error and returns `None`. Reads the *live* env each call,
/// so a `def` rebind is seen immediately (the same late binding as `Inst::Global`).
#[cfg(feature = "jit")]
pub(crate) fn jit_resolve_global(heap: &mut Heap, sym: Symbol) -> Option<Value> {
    let env = heap.read_root_env(JIT_CALL_ENV.with(|c| c.get()));
    match heap.env_get(env, sym) {
        Some(v) => Some(v),
        None => {
            let e = crate::eval::unbound_error(heap, sym);
            JIT_PENDING_ERROR.with(|c| *c.borrow_mut() = Some(e));
            None
        }
    }
}

/// Resolve free global `sym` through the per-`site` global inline cache — the JIT
/// equivalent of the VM's `Inst::GlobalIc`, sharing the same [`Heap::vm_global_ics`]
/// entries. On a process-global env, a cached value stamped at the current epoch is
/// returned without an `env_get` walk; a miss resolves once and fills the cache. This
/// is the difference between a hot recursive callee (`fib` resolving itself every call)
/// costing one cached read vs. a full name resolution per call — the cost that made
/// native-linked recursion regress `spawn` (millions of redundant `env_get`s). Late
/// binding holds via the epoch stamp (a `def` bumps the epoch → miss → re-resolve;
/// the JIT'd arm is invalidated by the same epoch). Dynamic vars are never cached.
#[cfg(feature = "jit")]
pub(crate) fn jit_resolve_global_ic(heap: &mut Heap, sym: Symbol, site: u32) -> Option<Value> {
    let env = heap.read_root_env(JIT_CALL_ENV.with(|c| c.get()));
    if heap.is_global(env) {
        let epoch = heap.global_epoch();
        if let Some(v) = heap.vm_global_ic_probe(site, sym, epoch) {
            crate::perf_bump!(global_ic_hit);
            return Some(v);
        }
        crate::perf_bump!(global_ic_miss);
        match heap.env_get(env, sym) {
            Some(v) => {
                if !value::is_dynamic(sym) {
                    heap.vm_global_ic_put(site, sym, epoch, v);
                }
                Some(v)
            }
            None => {
                let e = crate::eval::unbound_error(heap, sym);
                JIT_PENDING_ERROR.with(|c| *c.borrow_mut() = Some(e));
                None
            }
        }
    } else {
        match heap.env_get(env, sym) {
            Some(v) => Some(v),
            None => {
                let e = crate::eval::unbound_error(heap, sym);
                JIT_PENDING_ERROR.with(|c| *c.borrow_mut() = Some(e));
                None
            }
        }
    }
}

/// Run a JIT'd arm's **non-tail** Brood→Brood call. The callee + `argc` args are staged
/// on the operand stack (`roots`) in the VM's `Inst::Call` layout
/// (`[.., callee, arg0 .. arg_{argc-1}]`); this is the non-tail `Inst::Call` path lifted
/// out: read them, [`dispatch`] to completion, truncate the operands, return the result
/// (or park the error and return `None`). `tail = false` ⇒ `dispatch` runs the callee
/// fully — a VM closure via [`vm_apply`], a native/tree-walked callee inline — as a
/// **nested** (non-top-level) run, so it never preempts/suspends across the native
/// boundary (the §7.4 dirty carve-out), exactly like a builtin calling back into Brood.
#[cfg(feature = "jit")]
pub(crate) fn jit_dispatch_call(heap: &mut Heap, argc: usize) -> Option<Value> {
    use std::sync::atomic::Ordering::Acquire;
    // Cap on native-to-native recursion (see [`JIT_NATIVE_DEPTH`]). Past this many native
    // levels, drain the rest of the subtree on the VM (heap frames, [`MAX_BC_FRAMES`]) so
    // deep non-tail recursion keeps working instead of overflowing the native stack. 1 500
    // levels (~a few MB of the 16 MB worker stack) dwarfs any real call depth.
    const NATIVE_DEPTH_LIMIT: u32 = 1500;

    let n = heap.roots_len();
    let stage_base = n - argc - 1;
    let over_cap = JIT_NATIVE_DEPTH.with(|c| c.get()) >= NATIVE_DEPTH_LIMIT;

    // ---- Native-to-native call linking ----
    // If the callee is a Brood closure whose arm for this `argc` already carries installed,
    // epoch-current native code, call its native entry **directly** — replace the staged
    // `[callee, args]` with the callee's frame and invoke it — skipping the
    // `dispatch → vm_apply → vm_run_bc → jit_tier` chain the slow path walks (the per-call
    // overhead that dominates non-tail recursion: `fib`, `pfib`). The frame goes at
    // `stage_base`, exactly where the VM puts a callee frame, so this holds **no more roots
    // than the interpreter** — keeping the staged operands rooted *as well* (an earlier
    // design) cost ~2× the live roots per recursion level and regressed `spawn` ~1.8× from
    // the extra GC pressure across 20 000 process heaps.
    {
        let callee = heap.root_at(stage_base);
        if let Value::Fn(id) = callee {
            if let Some(arm) = compiled_arm_for(heap, id, argc) {
                let code = arm.jit_code.load(Acquire);
                let installed =
                    !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED;
                // `nslots > 0` mirrors `jit_lower_arm`'s return-via-`roots[base]` requirement.
                // `noptional == 0 && rest_slot.is_none()` keeps the inline frame setup trivial
                // and infallible (no default eval, which could GC/err) — the lowerable arms
                // (`fib`-shaped) all qualify. The epoch guard mirrors `jit_tier`: a `def` since
                // compilation invalidates the native code. Over the recursion cap → skip
                // (the slow path drains on the VM via `JIT_FORCE_VM`).
                if installed
                    && arm.nslots > 0
                    && arm.noptional == 0
                    && arm.rest_slot.is_none()
                    && !over_cap
                    && arm.compile_epoch.load(Acquire) == heap.global_epoch()
                {
                    let depth = JIT_NATIVE_DEPTH.with(|c| c.get());
                    let callee_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                    // Read the args, then replace the staged operands with the callee's frame
                    // at `stage_base`: nil-fill `nslots`, bind args into slots `[0, argc)`. No
                    // eval runs (no optionals), so no GC can strand the `argv` copy.
                    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                    for k in 0..argc {
                        argv.push(heap.root_at(stage_base + 1 + k));
                    }
                    heap.truncate_roots(stage_base);
                    for _ in 0..arm.nslots {
                        heap.push_root(Value::Nil);
                    }
                    for (k, &a) in argv.iter().enumerate() {
                        heap.set_root_at(stage_base + k, a);
                    }
                    let base = stage_base;
                    // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from
                    // `jit_lower_arm`, living for the process in `GLOBAL_JIT`; the frame is set
                    // up at `roots[base..]`.
                    let f: extern "C" fn(*mut Heap, i64) -> i64 =
                        unsafe { std::mem::transmute(code) };
                    let env_root = EnvRoot::Stable(callee_env);
                    let saved = JIT_CALL_ENV.with(|c| c.replace(env_root));
                    JIT_NATIVE_DEPTH.with(|c| c.set(depth + 1));
                    let outcome = f(heap as *mut Heap, base as i64);
                    JIT_NATIVE_DEPTH.with(|c| c.set(depth));
                    JIT_CALL_ENV.with(|c| c.set(saved));
                    match outcome {
                        // Done: the result is boxed in `roots[base]`. Take it, drop the frame.
                        0 => {
                            let result = heap.root_at(base);
                            heap.truncate_roots(stage_base);
                            return Some(result);
                        }
                        // Error: the callee parked it (unbound global, a deeper failure).
                        // PROPAGATE — never re-run, or an already-failed subtree re-errors at
                        // every unwinding level (quadratic; a hang on deep failing recursion).
                        3 => {
                            heap.truncate_roots(stage_base);
                            return None;
                        }
                        // deopt (1) / preempt (2) / tail (4): the native arm didn't complete.
                        // Re-run the callee on the VM. The args survive in the frame's param
                        // slots `[base, base+argc)` (GC-updated; params aren't overwritten by
                        // the arm body), so re-read them, drop the frame, and run via `vm_apply`
                        // — which handles deopt/preempt/tail correctly.
                        _ => {
                            let mut argv2: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                            for k in 0..argc {
                                argv2.push(heap.root_at(base + k));
                            }
                            heap.truncate_roots(stage_base);
                            return match vm_apply(heap, arm, &argv2, callee_env) {
                                Ok(v) => Some(v),
                                Err(e) => {
                                    JIT_PENDING_ERROR.with(|c| *c.borrow_mut() = Some(e));
                                    None
                                }
                            };
                        }
                    }
                }
            }
        }
    }

    // ---- Slow path ---- (re-read callee + args fresh: a fast-path fallthrough may have
    // run a collection that relocated the staged operands).
    let callee = heap.root_at(stage_base);
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
    for k in 0..argc {
        argv.push(heap.root_at(stage_base + 1 + k));
    }
    let env = heap.read_root_env(JIT_CALL_ENV.with(|c| c.get()));
    // Over the native cap: force this dispatch (and everything it recurses into) onto the
    // VM, so the remaining recursion drains through the bounded heap-frame loop rather than
    // the native stack. Restored after, so siblings above the cap go native again.
    let saved_force = if over_cap {
        Some(JIT_FORCE_VM.with(|c| c.replace(true)))
    } else {
        None
    };
    let result = match dispatch(heap, callee, argv, false, env) {
        Ok(Step::Done(v)) => Ok(v),
        // `tail = false` makes `dispatch` run a VM closure to a `Step::Done`; this arm is
        // defensive (a future `dispatch` tweak) and runs the tail callee to completion.
        Ok(Step::Tail { compiled, args, genv }) => vm_apply(heap, compiled, &args, genv),
        Err(e) => Err(e),
    };
    if let Some(prev) = saved_force {
        JIT_FORCE_VM.with(|c| c.set(prev));
    }
    match result {
        Ok(v) => {
            // Mirror the non-tail `Inst::Call`: drop callee+args, leaving the operand
            // stack back at the frame top. (On error we leave them — `vm_run_bc` unwinds
            // the whole frame to its entry mark.)
            heap.truncate_roots(n - argc - 1);
            Some(v)
        }
        Err(e) => {
            JIT_PENDING_ERROR.with(|c| *c.borrow_mut() = Some(e));
            None
        }
    }
}

/// Run a JIT'd arm's **tail** Brood→Brood call (outcome 4). The callee + `argc` args were
/// staged on `roots` *above the frame top* (`base + nslots`) in the VM's `Inst::Call`
/// layout (`[.., callee, arg0 .. arg_{argc-1}]`) — `argc` is recovered from the root
/// length since the JIT keeps its own operands in registers (so the frame top is always
/// exactly `base + nslots`). Unlike the non-tail path, the call *is* the arm's result
/// (TCO), so this resolves it with `tail = true` and hands [`vm_run_bc`] a [`ChunkExit`]
/// to **reuse** the current frame with — `Tail` for a VM-closure callee (run on the main
/// driver loop, keeping full preempt/suspend support), `Done` for an already-run
/// native/tree-walked callee. The native stack never grows: the driver's loop is the
/// trampoline. Mirrors the tail branch of the VM's `Inst::Call`.
#[cfg(feature = "jit")]
fn jit_dispatch_tail(
    heap: &mut Heap,
    base: usize,
    arm: &CompiledArm,
    env: EnvRoot,
) -> Result<ChunkExit, LispError> {
    let top = base + arm.nslots;
    let n = heap.roots_len();
    let argc = n - top - 1;
    let callee = heap.root_at(top);
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
    for k in 0..argc {
        argv.push(heap.root_at(top + 1 + k));
    }
    let env_id = heap.read_root_env(env);
    // `dispatch(.., tail = true, ..)` resolves a VM-closure callee to a `Step::Tail`
    // **without running it** (no native recursion) and runs a native/tree-walked callee
    // to a `Step::Done`. An error (incl. a control/suspend from a directly tail-called
    // suspending native — unreachable from surface `receive`, whose match closure puts
    // the arm out of subset) propagates; `vm_run_bc` unwinds the staged operands.
    let step = dispatch(heap, callee, argv, true, env_id)?;
    // Success: drop the staged operands. The driver next truncates to `base` and rebuilds
    // the frame for the callee (reuse), so leaving them would be harmless — but truncating
    // keeps the root stack tight if the callee turned out native (`Done`).
    heap.truncate_roots(top);
    Ok(match step {
        Step::Tail { compiled, args, genv } => ChunkExit::Tail { arm: compiled, args, genv },
        Step::Done(v) => ChunkExit::Done(v),
    })
}

/// Tiering entry (ADR-101 1b): on an arm invocation whose frame is already set up at
/// `roots[base..]`, decide whether to run the JIT'd code. Counts the call; once the arm
/// crosses the hotness threshold it is handed to the [background compiler](JIT_COMPILER)
/// **once** (a `null → QUEUED` CAS elects the single thread that enqueues it) and runs on
/// the VM meanwhile. When the native pointer is later installed, subsequent calls run it.
/// Returns `Some(outcome)` if JIT'd code ran (`0` = Done with the result in `roots[base]`,
/// `1` = deopt — an operand wasn't an `Int`, `2` = preempt — the back-edge budget was
/// spent), or `None` to run the arm on the VM (not hot yet, compile in flight, or out of
/// the JIT's subset). **Never blocks on compilation** — that's the whole point.
///
/// **Hot-reload safety (the epoch guard).** A JIT'd arm inlines its arithmetic operators
/// as raw machine ops, so it must be invalidated if a `def` rebinds one. The arm carries
/// the [`global_epoch`](Heap::global_epoch) it was compiled at; a `def` bumps that epoch.
/// Before each native entry we compare the two — on a mismatch the arm is reset to
/// untried, so the next call re-validates its operators ([`chunk_ops_all_native`]) and
/// either recompiles (the rebind was of some *other* global) or bails (the operator
/// itself was redefined, so it stays on the VM forever, dispatching to the new
/// definition). The check is per *activation*, not per loop iteration: a JIT'd arm
/// evaluates no Brood, so no `def` can land mid-run, and the redefinition therefore takes
/// effect at the next arm entry — the standard safepoint granularity for a JIT.
#[cfg(feature = "jit")]
pub(crate) fn jit_tier(
    arm: &Arc<CompiledArm>,
    heap: &mut Heap,
    base: usize,
    env: EnvRoot,
) -> Option<i64> {
    use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    const THRESHOLD: u32 = 8;

    // Draining an over-deep native-recursion subtree on the VM (see [`JIT_FORCE_VM`]):
    // interpret this arm so its recursion stays in the bounded heap-frame loop.
    if JIT_FORCE_VM.with(|c| c.get()) {
        return None;
    }
    let code = arm.jit_code.load(Acquire);
    if code == crate::jit::BAILED || code == crate::jit::QUEUED {
        return None; // out of subset, or compile in flight — run the VM
    }
    if code.is_null() {
        // Count the invocation; only enqueue once the arm is hot.
        if arm.jit_calls.fetch_add(1, Relaxed) + 1 < THRESHOLD {
            return None;
        }
        // Hot. Refuse to JIT an arm whose inlined operators are no longer native (a `def`
        // redefined one): mark it BAILED so it stays on the VM, where the operator's
        // epoch guard dispatches to the new definition. Otherwise record the epoch the
        // arm is being compiled at (the hot-reload guard, read on each native entry below)
        // and elect a single enqueuer via CAS (others see QUEUED and run the VM). A full
        // queue → back off: reset to untried so a later hot call re-attempts.
        if !chunk_ops_all_native(heap, arm) {
            arm.jit_code.store(crate::jit::BAILED, Release);
            return None;
        }
        arm.compile_epoch.store(heap.global_epoch(), Release);
        if arm
            .jit_code
            .compare_exchange(std::ptr::null_mut(), crate::jit::QUEUED, AcqRel, Acquire)
            .is_ok()
            && JIT_COMPILER.try_send(arm.clone()).is_err()
        {
            // The background compile queue is full (a burst of distinct hot arms — e.g.
            // thousands of short-lived green processes each tiering their own arm copy,
            // overwhelming the bounded channel). Reset to untried AND back the hotness
            // counter all the way off, so the arm runs on the VM for another THRESHOLD
            // calls before re-attempting — instead of re-validating (`chunk_ops_all_native`,
            // an `env_get`/`resolve_prim` per op) on *every* call while the queue stays
            // full. Measured: ~36M redundant re-validations in `spawn` (20 000 procs)
            // collapse to ~1/THRESHOLD of that. The arm still compiles once the queue
            // drains (a long-lived process re-reaches the threshold and re-enqueues).
            arm.jit_code.store(std::ptr::null_mut(), Release);
            arm.jit_calls.store(0, Relaxed);
        }
        return None;
    }
    // A real, installed code pointer. Hot-reload guard: if the global epoch moved since
    // the arm was compiled, some `def` happened — invalidate the arm (reset to untried)
    // and run the VM this activation. The next call re-tiers, re-validating operators and
    // recompiling at the new epoch, or bailing if one was genuinely redefined.
    if arm.compile_epoch.load(Acquire) != heap.global_epoch() {
        arm.jit_code.store(std::ptr::null_mut(), Release);
        arm.jit_calls.store(THRESHOLD, Release); // re-tier promptly (already proven hot)
        return None;
    }
    // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base) -> i64` produced by
    // `jit_lower_arm`, living in the process-lifetime GLOBAL_JIT module. The frame is set
    // up at `roots[base..]`; the JIT'd arm keeps its own operands in registers (the call
    // staging grows `roots` only transiently, popped before return), so `heap` stays
    // valid for the call.
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(code) };
    // Publish this arm's env for the call/global callbacks, save/restoring the previous
    // value so a JIT'd callee that re-enters another JIT'd arm nests correctly.
    let saved_env = JIT_CALL_ENV.with(|c| c.replace(env));
    let outcome = f(heap as *mut Heap, base as i64);
    JIT_CALL_ENV.with(|c| c.set(saved_env));
    Some(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bump a movable handle's index by `by`; leave atoms alone. Stands in for the
    // `runtime_collect` flush that relocates a handle into the compacted region.
    fn bump(v: Value, by: usize) -> Value {
        match v {
            Value::Str(id) => Value::Str(StrId::runtime(id.index() + by)),
            Value::Pair(id) => Value::Pair(PairId::runtime(id.index() + by)),
            other => other,
        }
    }

    // `Value` has no `PartialEq` (Brood equality is a structural function), so compare
    // a handle const by kind + index.
    fn str_idx(v: Value) -> usize {
        match v {
            Value::Str(id) => id.index(),
            other => panic!("expected a Str, got {:?}", std::mem::discriminant(&other)),
        }
    }
    fn pair_idx(v: Value) -> usize {
        match v {
            Value::Pair(id) => id.index(),
            other => panic!("expected a Pair, got {:?}", std::mem::discriminant(&other)),
        }
    }

    /// Regression: a swapped `(op Const Local)` `Prim2SlotInt` must keep inlining after an
    /// epoch bump. The fusion stores an *inverted* arg-map (so the inline operand pick is
    /// correct); `prim2_inline_exec` revalidates against the head's *natural* map, so the
    /// `swapped` call site must un-invert it. Before the fix it compared the inverted map,
    /// which never matched `resolve_prim`'s natural map — so every such prim silently fell
    /// to the slow path forever after the first `def` bumped the epoch.
    #[test]
    fn swapped_prim2slotint_reinlines_after_epoch_bump() {
        let mut interp = crate::Interp::new();
        let heap = &mut interp.heap;
        let minus = value::intern("-"); // natural map [0,1]; `(- 24 x)` fuses to [1,0] swapped
        // A stale guard (≠ current epoch) forces the revalidation path the bug lived on.
        let guard = AtomicU64::new(heap.global_epoch().wrapping_add(1));
        // Operands as the caller picks them for map=[1,0]: x = const 24, y = local 5.
        let out = prim2_inline_exec(heap, PrimOp::Sub, [1, 0], true, minus, &guard, Value::Int(24), Value::Int(5))
            .expect("no arithmetic error");
        match out {
            Some(Value::Int(n)) => assert_eq!(n, 19, "(- 24 5) must inline to 19"),
            Some(other) => panic!("expected Int(19), got tag {:?}", value::tag(other)),
            None => panic!("swapped Prim2SlotInt slow-pathed after an epoch bump (the bug)"),
        }
        // The guard was refreshed to the live epoch, so subsequent calls take the fast path.
        assert_eq!(guard.load(Ordering::Relaxed), heap.global_epoch());
    }

    #[test]
    fn const_handle_round_trips() {
        // A heap-handle const decodes back to the same handle, and `rewrite` moves it.
        let cv = ConstVal::new(Value::Str(StrId::runtime(5)));
        assert!(matches!(cv, ConstVal::Handle { .. }), "a Str must encode as a Handle");
        assert_eq!(str_idx(cv.load()), 5);
        cv.rewrite(&mut |v| bump(v, 100));
        assert_eq!(str_idx(cv.load()), 105, "rewrite must relocate the handle");

        // An atom stays inline and is never touched by a rewrite.
        let atom = ConstVal::new(Value::Int(42));
        assert!(matches!(atom, ConstVal::Atom(_)), "an Int must encode as an Atom");
        atom.rewrite(&mut |_| panic!("an atom const must not be passed to the flush"));
        assert!(matches!(atom.load(), Value::Int(42)));
    }

    #[test]
    fn rewrite_arm_handles_rewrites_every_embedded_handle() {
        // The regression guard: `runtime_collect` calls this on each LIVE arm, so it
        // must reach every movable handle a node tree embeds — a `Const` literal, a
        // `MakeClosure` `fn_rest`, an `&optional` default — through all the structural
        // node variants, while leaving atoms/symbols/indices alone.
        let body = Node::Do(Box::new([
            Node::Const(ConstVal::new(Value::Str(StrId::runtime(1)))),
            Node::If(
                Box::new(Node::Const(ConstVal::new(Value::Int(7)))), // atom — untouched
                Box::new(Node::Const(ConstVal::new(Value::Pair(PairId::runtime(2))))),
                Box::new(Node::MakeClosure {
                    fn_rest: ConstVal::new(Value::Pair(PairId::runtime(3))),
                    captures: Box::new([]),
                    self_name: None,
                }),
            ),
        ]));
        let arm = CompiledArm {
            nrequired: 0,
            noptional: 1,
            optional_defaults: Box::new([Some(Node::Const(ConstVal::new(Value::Str(
                StrId::runtime(4),
            ))))]),
            rest_slot: None,
            nslots: 0,
            body,
            chunk: None,
            has_runtime_handles: true,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        };

        rewrite_arm_handles(&arm, &mut |v| bump(v, 100));

        // Destructure the (known) tree and assert each handle moved, the atom didn't.
        let Node::Do(top) = &arm.body else { panic!("body") };
        assert_eq!(str_idx(load_const(&top[0])), 101);
        let Node::If(cond, then, els) = &top[1] else { panic!("if") };
        assert!(matches!(load_const(cond), Value::Int(7)), "atom const must be untouched");
        assert_eq!(pair_idx(load_const(then)), 102);
        let Node::MakeClosure { fn_rest, .. } = &**els else { panic!("makeclosure") };
        assert_eq!(pair_idx(fn_rest.load()), 103);
        let Some(def) = &arm.optional_defaults[0] else { panic!("optional default") };
        assert_eq!(str_idx(load_const(def)), 104);
    }

    fn load_const(node: &Node) -> Value {
        match node {
            Node::Const(cv) => cv.load(),
            other => panic!("expected a Const, got {:?}", std::mem::discriminant(other)),
        }
    }

    // ===================== state-capture (ADR-100 §8) =====================

    thread_local! {
        /// Drives the suspend-once test native: 0 → suspend, ≥1 → return the value.
        static SUSPEND_GATE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    /// A stand-in for the `%receive` native: the **first** call raises a
    /// `Control::Suspend` (as a clean `receive` on an empty mailbox would); the
    /// **second** returns a value (as it would once a message arrived). Lets the
    /// capture→resume round-trip be tested in isolation
    /// from the mailbox/scheduler plumbing — the machinery under test is the driver's
    /// capture + replay, identical for any native that suspends mid-call.
    fn suspend_once_native(_args: &[Value], _env: EnvId, _heap: &mut Heap) -> LispResult {
        let n = SUSPEND_GATE.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        });
        if n == 0 {
            Err(LispError::suspend(None))
        } else {
            Ok(Value::Int(42))
        }
    }

    #[test]
    fn vm_run_bc_captures_and_resumes_a_suspend() {
        use crate::core::value::{Arity, NativeFn};
        use crate::types::Sig;

        SUSPEND_GATE.with(|c| c.set(0));
        let mut heap = Heap::new();

        // The suspend-once native, held in the arm's one frame slot (slot 0). A 0-arg
        // `Inst::Call` against it is the suspending point — the shape a `(receive …)`
        // lowers to (`%receive` is the callee, here `slot 0`).
        let native = heap.alloc_native(NativeFn {
            name: "%test-suspend-once".to_string(),
            arity: Arity::exact(0),
            func: suspend_once_native,
            params: &[],
            doc: "",
            sig: Sig::any(),
        });

        // Body `(slot0)`: push the native from slot 0, then a non-tail 0-ary call.
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Call { argc: 0, tail: false, pos: None, site: NO_SITE, head: None },
            ],
        };
        let arm = Arc::new(CompiledArm {
            nrequired: 1, // slot 0 = the callee, passed as the sole arg
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 1,
            body: Node::Const(ConstVal::new(Value::Nil)), // unused at runtime (chunk drives)
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        });

        // First run: the native suspends, so the driver captures the continuation
        // WITHOUT unwinding (the operand stack — the pushed callee — survives on the
        // heap for the resume).
        let roots_before = heap.roots_len();
        let outcome = vm_run_bc(&mut heap, arm.clone(), &[native], EnvId::GLOBAL, None, true)
            .expect("first run errored");
        let suspended = match outcome {
            VmOutcome::Suspended(s) => s,
            _ => panic!("expected a captured suspend"),
        };
        assert!(
            heap.roots_len() > roots_before,
            "the captured continuation's frame slots + operands must stay rooted"
        );

        // Resume: replay from the rewound `%receive` call; the native now returns 42.
        let resumed = vm_run_bc(&mut heap, arm, &[native], EnvId::GLOBAL, Some(suspended), true)
            .expect("resume errored");
        match resumed {
            VmOutcome::Done(Value::Int(n)) => assert_eq!(n, 42, "resumed to the wrong value"),
            VmOutcome::Done(other) => panic!("resumed to a non-int: {:?}", value::tag(other)),
            other => panic!(
                "expected Done(42), got {}",
                match other {
                    VmOutcome::Suspended(_) => "Suspended (the gate didn't advance)",
                    VmOutcome::Preempted(_) => "Preempted",
                    VmOutcome::Killed => "Killed",
                    VmOutcome::Done(_) => unreachable!(),
                }
            ),
        }
        // The driver retired its only frame on `Done`, unwinding the operand stack
        // back to where the first run started.
        assert_eq!(
            heap.roots_len(),
            roots_before,
            "a completed resume must tear its frame stack back down to entry"
        );
    }

    /// JIT Stage-1 Step A: lower a straight-line int arm `(+ x 1)` to native code and
    /// run it against a real heap frame — read the arg from `roots[base]`, compute in
    /// registers, box the result back, and match the VM's answer.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_and_runs_a_straight_line_int_arm() {
        let mut heap = Heap::new();
        // Body `(+ x 1)`: [Local(0), Const(1), Prim2 Add].
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::Int(1))),
                Inst::Prim2 {
                    op: PrimOp::Add,
                    map: [0, 1],
                    head: value::intern("+"),
                    guard: AtomicU64::new(0),
                    pos: None,
                },
            ],
        };
        let arm = CompiledArm {
            nrequired: 1,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 1,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        };

        let mut jit = crate::jit::Jit::new();
        let ptr = jit_lower_arm(&mut jit, &arm).expect("straight-line int arm should JIT");
        let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

        // Frame: x = 41 at roots[base].
        let base = heap.roots_len();
        heap.push_root(Value::Int(41));
        let outcome = f(&mut heap as *mut Heap, base as i64);
        assert_eq!(outcome, 0, "Done (no deopt — arg is an Int)");
        match heap.root_at(base) {
            Value::Int(n) => assert_eq!(n, 42, "JIT-compiled (+ x 1) on x=41"),
            other => panic!("expected Int(42), got tag {:?}", value::tag(other)),
        }
    }

    /// JIT Stage-1 Step B: control flow + comparisons. Lower `(if (< x 0) (- 0 x) x)`
    /// (an `abs`) — JumpIfFalse/Jump → CFG blocks, `<` → an `icmp` branch, the two arms
    /// merging at a Done block param — and check both arms against the math.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_and_runs_an_if_with_comparison() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        // (if (< x 0) (- 0 x) x), x = slot 0.
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),                            // 0: x
                Inst::Const(ConstVal::new(Value::Int(0))), // 1: 0
                prim2(PrimOp::Lt, "<"),                    // 2: x < 0
                Inst::JumpIfFalse(8),                      // 3: false → else (ip 8)
                Inst::Const(ConstVal::new(Value::Int(0))), // 4: then: 0
                Inst::Local(0),                            // 5: x
                prim2(PrimOp::Sub, "-"),                   // 6: 0 - x
                Inst::Jump(9),                             // 7: → done (ip 9 = len)
                Inst::Local(0),                            // 8: else: x
            ],
        };
        let arm = CompiledArm {
            nrequired: 1,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 1,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        };

        let mut jit = crate::jit::Jit::new();
        let ptr = jit_lower_arm(&mut jit, &arm).expect("if/cmp arm should JIT");
        let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

        for (x, want) in [(-5i64, 5i64), (3, 3), (0, 0)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::Int(x));
            assert_eq!(f(&mut heap as *mut Heap, base as i64), 0, "Done for x={x}");
            match heap.root_at(base) {
                Value::Int(n) => assert_eq!(n, want, "abs({x})"),
                other => panic!("x={x}: expected Int({want}), got tag {:?}", value::tag(other)),
            }
        }
    }

    /// JIT Stage-1 Step C: the self-recursive **loop**. Lower
    /// `(if (< i 1) acc (sumto (- i 1) (+ acc i)))` — `SelfCall` boxes the new args into
    /// the frame slots and branches the loop header; the frame slots in `roots` carry the
    /// loop state. A native int loop, no per-iteration dispatch. (No `tick` yet — tested
    /// in isolation, not wired into the scheduler.)
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_and_runs_a_self_recursive_int_loop() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        // (defn sumto (i acc) (if (< i 1) acc (sumto (- i 1) (+ acc i)))) — i=slot0, acc=slot1.
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),                            // 0: i
                Inst::Const(ConstVal::new(Value::Int(1))), // 1: 1
                prim2(PrimOp::Lt, "<"),                    // 2: i < 1
                Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
                Inst::Local(1),                            // 4: then: acc
                Inst::Jump(13),                            // 5: → done (len)
                Inst::Local(0),                            // 6: else: i
                Inst::Const(ConstVal::new(Value::Int(1))), // 7: 1
                prim2(PrimOp::Sub, "-"),                   // 8: (- i 1)  = arg0
                Inst::Local(1),                            // 9: acc
                Inst::Local(0),                            // 10: i
                prim2(PrimOp::Add, "+"),                   // 11: (+ acc i) = arg1
                Inst::SelfCall { argc: 2 },                // 12: (sumto arg0 arg1)
            ],
        };
        let arm = CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        };

        let mut jit = crate::jit::Jit::new();
        let ptr = jit_lower_arm(&mut jit, &arm).expect("self-recursive int loop should JIT");
        let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

        // Prime the reduction budget so these short loops run to completion (the
        // back-edge `brood_rt_tick` would otherwise yield at REDUCTIONS == 0).
        crate::process::yield_now();
        // sumto(n,0) = n+(n-1)+…+1; sumto(1,0)→sumto(0,1)→1; sumto(0,0)→0.
        for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::Int(n)); // i = slot 0
            heap.push_root(Value::Int(0)); // acc = slot 1
            assert_eq!(f(&mut heap as *mut Heap, base as i64), 0, "Done for n={n}");
            match heap.root_at(base) {
                Value::Int(r) => assert_eq!(r, want, "sumto({n}, 0)"),
                other => panic!("n={n}: expected Int({want}), got tag {:?}", value::tag(other)),
            }
        }

        // Preemption: a loop longer than the reduction budget yields at a back-edge —
        // the JIT'd arm returns 2 (preempt), with the frame slots left mid-computation
        // in `roots` for the driver to resume on the VM. `brood_rt_tick` only preempts in
        // a capture-mode green process, so simulate one (set/clear `capture_run`).
        crate::process::set_capture_run(true);
        crate::process::yield_now(); // budget = REDUCTION_BUDGET
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::Int(1_000_000)); // far more iterations than the budget
        heap.push_root(Value::Int(0));
        let outcome = f(&mut heap as *mut Heap, base as i64);
        crate::process::set_capture_run(false); // restore (cargo test shares threads)
        assert_eq!(outcome, 2, "a loop exceeding the budget must preempt (return 2) in a green process");
    }

    /// An arm *ending* in a **tail call to a different global** (`Inst::Call { tail: true }`)
    /// must lower (return `Some`), not bail — this is the jit-tier2 §6.2 payoff. The body
    /// is deliberately past the body-weight gate (4 work ops: `=`, `-`, `*`, `*`), since a
    /// thinner tail-call arm is gated out on purpose. We can't run it in isolation (outcome
    /// 4 needs the driver to dispatch the staged callee), so this asserts the *lowering*
    /// succeeds; `tests/jit.rs` proves the end-to-end result.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_an_arm_ending_in_a_tail_call() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op, map: [0, 1], head: value::intern(head), guard: AtomicU64::new(0), pos: None,
        };
        // (defn fa (n acc) (if (= n 0) acc (fb (- n 1) (* (* acc acc) acc)))) — n=slot0, acc=slot1.
        let fb = value::intern("fb");
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),                            // 0: n
                Inst::Const(ConstVal::new(Value::Int(0))), // 1: 0
                prim2(PrimOp::Eq, "="),                    // 2: n == 0    (work 1)
                Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
                Inst::Local(1),                            // 4: then: acc
                Inst::Jump(16),                            // 5: → done (len)
                Inst::Global(fb),                          // 6: else: callee `fb`
                Inst::Local(0),                            // 7: n
                Inst::Const(ConstVal::new(Value::Int(1))), // 8: 1
                prim2(PrimOp::Sub, "-"),                   // 9: (- n 1) = arg0   (work 2)
                Inst::Local(1),                            // 10: acc
                Inst::Local(1),                            // 11: acc
                prim2(PrimOp::Mul, "*"),                   // 12: (* acc acc)     (work 3)
                Inst::Local(1),                            // 13: acc
                prim2(PrimOp::Mul, "*"),                   // 14: (* … acc) = arg1 (work 4)
                Inst::Call { argc: 2, tail: true, pos: None, site: NO_SITE, head: Some(fb) }, // 15
            ],
        };
        let arm = CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        };
        let mut jit = crate::jit::Jit::new();
        assert!(
            jit_lower_arm(&mut jit, &arm).is_some(),
            "an arm ending in a tail call (past the body-weight gate) must lower, not bail"
        );

        // ...and a *thin* tail-call arm (2 work ops: `=`, `-`) is gated out — stays on the
        // VM, where the per-hop round-trip would otherwise cost more than it saves.
        let thin = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::Int(0))),
                prim2(PrimOp::Eq, "="),
                Inst::JumpIfFalse(6),
                Inst::Local(1),
                Inst::Jump(10),
                Inst::Global(fb),
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::Int(1))),
                prim2(PrimOp::Sub, "-"),
                Inst::Call { argc: 1, tail: true, pos: None, site: NO_SITE, head: Some(fb) },
            ],
        };
        let thin_arm = CompiledArm {
            nrequired: 2, noptional: 0, optional_defaults: Box::new([]), rest_slot: None,
            nslots: 2, body: Node::Const(ConstVal::new(Value::Nil)), chunk: Some(thin),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        };
        assert!(
            jit_lower_arm(&mut jit, &thin_arm).is_none(),
            "a thin tail-call arm (2 work ops) must be gated out (stays on the VM)"
        );
    }

    /// JIT Stage-1.5: the **fused** `Prim2Slot*` variants — which `emit_node` actually
    /// produces for real loop bodies (`(- i 1)`, `(+ acc i)`, `(< i 1)`) — lower and run.
    /// Before this, the JIT bailed on every fused prim, so it never fired on real
    /// compiled code. Also pins the two correctness fixes that came with the coverage:
    /// `map` (the `>`/swapped-operand case) and overflow → deopt (so the JIT matches the
    /// VM's BigInt promotion instead of silently wrapping).
    #[cfg(feature = "jit")]
    #[test]
    fn jit_lowers_fused_prims_map_and_overflow() {
        // All uses here are the `(op Local Const)` form, so `swapped: false`.
        let slot_int = |op: PrimOp, map: [u8; 2], slot_a: usize, int_b: i64, head: &str| {
            Inst::Prim2SlotInt {
                op, map, slot_a, int_b, swapped: false,
                head: value::intern(head), guard: AtomicU64::new(0), pos: None,
            }
        };
        let slot_slot = |op: PrimOp, slot_a: usize, slot_b: usize, head: &str| Inst::Prim2SlotSlot {
            op, map: [0, 1], slot_a, slot_b,
            head: value::intern(head), guard: AtomicU64::new(0), pos: None,
        };
        let mk_arm = |chunk: Chunk, nreq: usize, nslots: usize| CompiledArm {
            nrequired: nreq, noptional: 0, optional_defaults: Box::new([]), rest_slot: None,
            nslots, body: Node::Const(ConstVal::new(Value::Nil)), chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: std::sync::atomic::AtomicU32::new(0),
            compile_epoch: std::sync::atomic::AtomicU64::new(0),
        };
        let mut jit = crate::jit::Jit::new();

        // (a) sumto with the REAL fused shape: `(< i 1)`/`(- i 1)` → Prim2SlotInt,
        // `(+ acc i)` → Prim2SlotSlot. i = slot0, acc = slot1.
        let sumto = mk_arm(
            Chunk {
                code: vec![
                    slot_int(PrimOp::Lt, [0, 1], 0, 1, "<"),  // 0: (< i 1)
                    Inst::JumpIfFalse(4),                     // 1: false → else
                    Inst::Local(1),                           // 2: then: acc
                    Inst::Jump(7),                            // 3: → done
                    slot_int(PrimOp::Sub, [0, 1], 0, 1, "-"), // 4: (- i 1) = arg0
                    slot_slot(PrimOp::Add, 1, 0, "+"),        // 5: (+ acc i) = arg1
                    Inst::SelfCall { argc: 2 },               // 6: (sumto arg0 arg1)
                ],
            },
            2, 2,
        );
        let f: extern "C" fn(*mut Heap, i64) -> i64 =
            unsafe { std::mem::transmute(jit_lower_arm(&mut jit, &sumto).expect("fused sumto JITs")) };
        crate::process::yield_now(); // prime the reduction budget so the loop completes
        for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::Int(n));
            heap.push_root(Value::Int(0));
            assert_eq!(f(&mut heap as *mut Heap, base as i64), 0, "Done for sumto({n})");
            match heap.root_at(base) {
                Value::Int(r) => assert_eq!(r, want, "fused sumto({n}, 0)"),
                other => panic!("expected Int, got tag {:?}", value::tag(other)),
            }
        }

        // (b) `map` — `>` lowers to `%lt` with `map = [1, 0]` (operands swapped). The JIT
        // must apply it: `(if (> x 5) 100 200)` is 100 for x=10 and 200 for x=3. Ignoring
        // `map` would compute `x < 5` and flip both answers.
        let gt = mk_arm(
            Chunk {
                code: vec![
                    slot_int(PrimOp::Lt, [1, 0], 0, 5, ">"), // 0: (> x 5)  [swapped]
                    Inst::JumpIfFalse(4),                    // 1
                    Inst::Const(ConstVal::new(Value::Int(100))), // 2: then
                    Inst::Jump(5),                           // 3
                    Inst::Const(ConstVal::new(Value::Int(200))), // 4: else
                ],
            },
            1, 1,
        );
        let g: extern "C" fn(*mut Heap, i64) -> i64 =
            unsafe { std::mem::transmute(jit_lower_arm(&mut jit, &gt).expect("(> x 5) JITs")) };
        for (x, want) in [(10i64, 100i64), (3, 200)] {
            let mut heap = Heap::new();
            let base = heap.roots_len();
            heap.push_root(Value::Int(x));
            assert_eq!(g(&mut heap as *mut Heap, base as i64), 0, "Done for (> {x} 5)");
            match heap.root_at(base) {
                Value::Int(r) => assert_eq!(r, want, "(if (> {x} 5) 100 200) — map must be applied"),
                other => panic!("expected Int, got tag {:?}", value::tag(other)),
            }
        }

        // (c) overflow → deopt. `(* x x)` for a huge x overflows i64; the VM defers such
        // an op to the native, which promotes to a BigInt, so the JIT must deopt (return
        // 1) rather than store a wrapped i64. A non-overflowing x runs to Done (0).
        let sq = mk_arm(Chunk { code: vec![slot_slot(PrimOp::Mul, 0, 0, "*")] }, 1, 1);
        let s: extern "C" fn(*mut Heap, i64) -> i64 =
            unsafe { std::mem::transmute(jit_lower_arm(&mut jit, &sq).expect("(* x x) JITs")) };
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::Int(3));
        assert_eq!(s(&mut heap as *mut Heap, base as i64), 0, "(* 3 3) is in range");
        assert!(matches!(heap.root_at(base), Value::Int(9)), "(* 3 3) = 9");
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::Int(4_000_000_000)); // 4e9 * 4e9 = 1.6e19 > i64::MAX
        assert_eq!(
            s(&mut heap as *mut Heap, base as i64),
            1,
            "an overflowing (* x x) must deopt to the VM (BigInt), not wrap"
        );
    }

    /// JIT Stage-1 1b: tiering. An arm invoked past the hotness threshold is compiled
    /// once and thereafter runs as native code (`jit_tier` returns `Some(0)` with the
    /// result in `roots[base]`); below the threshold it returns `None` (run on the VM).
    /// An arm out of the JIT subset is marked BAILED and always returns `None`.
    #[cfg(feature = "jit")]
    #[test]
    fn jit_tier_compiles_a_hot_arm_then_runs_native() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        // sumto(i acc) = (if (< i 1) acc (sumto (- i 1) (+ acc i))).
        let mk_arm = |chunk: Chunk, nreq: usize, nslots: usize| CompiledArm {
            nrequired: nreq,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: AtomicU32::new(0),
            compile_epoch: AtomicU64::new(0),
        };
        let sumto = Arc::new(mk_arm(
            Chunk {
                code: vec![
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::Int(1))),
                    prim2(PrimOp::Lt, "<"),
                    Inst::JumpIfFalse(6),
                    Inst::Local(1),
                    Inst::Jump(13),
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::Int(1))),
                    prim2(PrimOp::Sub, "-"),
                    Inst::Local(1),
                    Inst::Local(0),
                    prim2(PrimOp::Add, "+"),
                    Inst::SelfCall { argc: 2 },
                ],
            },
            2,
            2,
        ));

        // A prelude-loaded heap, so `jit_tier`'s operator-validation (`+`/`-`/`<` must
        // still resolve to their natives — the hot-reload guard) sees the live globals; a
        // bare `Heap::new()` has no global env. One heap, reused across poll iterations
        // (truncate the frame each time), keeps the epoch stable so the arm stays tiered.
        let mut interp = crate::Interp::new();
        // Compilation is async now (the background compiler thread), so a hot arm returns
        // None until the native pointer is installed. Poll past the threshold, giving the
        // compiler time to land the code, and assert it eventually runs native.
        crate::process::yield_now(); // prime the reduction budget (short loops)
        let mut ran_native = 0;
        for _ in 0..400 {
            crate::process::yield_now(); // keep the budget topped up across calls
            let base = interp.heap.roots_len();
            interp.heap.push_root(Value::Int(5)); // i
            interp.heap.push_root(Value::Int(0)); // acc
            let outcome = jit_tier(&sumto, &mut interp.heap, base, EnvRoot::Stable(EnvId::GLOBAL));
            match outcome {
                None => {
                    interp.heap.truncate_roots(base);
                    std::thread::sleep(std::time::Duration::from_millis(2)); // not hot / compile in flight
                }
                Some(0) => {
                    ran_native += 1;
                    match interp.heap.root_at(base) {
                        Value::Int(r) => assert_eq!(r, 15, "JIT'd sumto(5,0)"),
                        other => panic!("expected Int(15), got tag {:?}", value::tag(other)),
                    }
                    interp.heap.truncate_roots(base);
                    if ran_native >= 3 {
                        break;
                    }
                }
                Some(o) => panic!("unexpected JIT outcome {o}"),
            }
        }
        assert!(ran_native > 0, "the hot arm should tier up to native code");

        // An out-of-subset arm (a non-int `Const` — only `Const(Int)` is lowered) is
        // marked BAILED and never runs native. (A bare `Global` now *is* in subset — it
        // lowers to a `brood_rt_global` resolve — so it's no longer the bail example.)
        let bailing =
            Arc::new(mk_arm(Chunk { code: vec![Inst::Const(ConstVal::new(Value::Nil))] }, 0, 1));
        for _ in 0..400 {
            let base = interp.heap.roots_len();
            interp.heap.push_root(Value::Int(0));
            assert_eq!(
                jit_tier(&bailing, &mut interp.heap, base, EnvRoot::Stable(EnvId::GLOBAL)),
                None,
                "out-of-subset arm bails"
            );
            interp.heap.truncate_roots(base);
            if bailing.jit_code.load(std::sync::atomic::Ordering::Acquire) == crate::jit::BAILED {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            bailing.jit_code.load(std::sync::atomic::Ordering::Acquire),
            crate::jit::BAILED,
            "out-of-subset arm must settle to BAILED"
        );
    }

    /// JIT Stage-1 end-to-end: `vm_run_bc`'s hot-path hook runs a tiered arm as native
    /// code and returns the same result the interpreter would. Warm the arm past the
    /// threshold so it compiles, then invoke it through `vm_run_bc` (fresh start) and
    /// check the `Done` value.
    #[cfg(feature = "jit")]
    #[test]
    fn vm_run_bc_runs_a_tiered_arm_via_the_hook() {
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        let chunk = Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::Int(1))),
                prim2(PrimOp::Lt, "<"),
                Inst::JumpIfFalse(6),
                Inst::Local(1),
                Inst::Jump(13),
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::Int(1))),
                prim2(PrimOp::Sub, "-"),
                Inst::Local(1),
                Inst::Local(0),
                prim2(PrimOp::Add, "+"),
                Inst::SelfCall { argc: 2 },
            ],
        };
        let arm = Arc::new(CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: Some(chunk),
            has_runtime_handles: false,
            jit_code: AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: AtomicU32::new(0),
            compile_epoch: AtomicU64::new(0),
        });

        // Warm it past the threshold so jit_tier hands it to the background compiler;
        // poll until the native pointer is installed (compilation is async now). A
        // prelude-loaded heap, so the operator-validation in `jit_tier` resolves `+`/`-`/`<`.
        use std::sync::atomic::Ordering::Acquire;
        let mut interp = crate::Interp::new();
        crate::process::yield_now();
        let mut tiered = false;
        for _ in 0..400 {
            crate::process::yield_now();
            let base = interp.heap.roots_len();
            interp.heap.push_root(Value::Int(5));
            interp.heap.push_root(Value::Int(0));
            let _ = jit_tier(&arm, &mut interp.heap, base, EnvRoot::Stable(EnvId::GLOBAL));
            interp.heap.truncate_roots(base);
            let code = arm.jit_code.load(Acquire);
            if !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED {
                tiered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(tiered, "the arm should have tiered up to native code");

        // Now run it through vm_run_bc — its fresh-start hook should call the native code.
        crate::process::yield_now();
        let outcome = vm_run_bc(
            &mut interp.heap,
            arm,
            &[Value::Int(5), Value::Int(0)],
            EnvId::GLOBAL,
            None,
            true,
        )
        .expect("vm_run_bc errored");
        match outcome {
            VmOutcome::Done(Value::Int(n)) => assert_eq!(n, 15, "tiered sumto(5,0) via the hook"),
            VmOutcome::Done(other) => panic!("Done non-int: tag {:?}", value::tag(other)),
            _ => panic!("expected Done(15) from the JIT hook"),
        }
    }

    /// JIT Stage-1.5: the actual speedup — JIT'd `sumto(N,0)` vs the interpreter, same
    /// arm, run through `vm_run_bc`. The VM baseline forces BAILED so its hook stays on
    /// the interpreter; the JIT arm is warmed so the hook runs native. Benchmark, not a
    /// pass/fail test — run with `--ignored --nocapture`.
    #[cfg(feature = "jit")]
    #[test]
    #[ignore = "benchmark — cargo test -p brood --features jit --lib jit_speedup -- --ignored --nocapture"]
    fn jit_speedup_vs_vm() {
        use std::time::Instant;
        let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
            op,
            map: [0, 1],
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
        let mk = || CompiledArm {
            nrequired: 2,
            noptional: 0,
            optional_defaults: Box::new([]),
            rest_slot: None,
            nslots: 2,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: Some(Chunk {
                code: vec![
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::Int(1))),
                    prim2(PrimOp::Lt, "<"),
                    Inst::JumpIfFalse(6),
                    Inst::Local(1),
                    Inst::Jump(13),
                    Inst::Local(0),
                    Inst::Const(ConstVal::new(Value::Int(1))),
                    prim2(PrimOp::Sub, "-"),
                    Inst::Local(1),
                    Inst::Local(0),
                    prim2(PrimOp::Add, "+"),
                    Inst::SelfCall { argc: 2 },
                ],
            }),
            has_runtime_handles: false,
            jit_code: AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: AtomicU32::new(0),
            compile_epoch: AtomicU64::new(0),
        };
        let n = 100_000i64; // iterations per sumto call
        let reps = 300;
        // A prelude-loaded heap, reused across reps (vm_run_bc unwinds to entry on Done, so
        // roots stay clean): needed so the JIT tiering hook's operator-validation resolves
        // `+`/`-`/`<`, and so the per-rep cost is the loop, not a prelude load.
        let mut interp = crate::Interp::new();
        let run = |h: &mut Heap, arm: &Arc<CompiledArm>| -> i64 {
            match vm_run_bc(h, arm.clone(), &[Value::Int(n), Value::Int(0)], EnvId::GLOBAL, None, true)
                .expect("run")
            {
                VmOutcome::Done(Value::Int(r)) => r,
                _ => panic!("bad outcome"),
            }
        };

        // VM baseline: BAILED forces the hook to stay on the interpreter.
        let vm_arm = Arc::new(mk());
        vm_arm.jit_code.store(crate::jit::BAILED, std::sync::atomic::Ordering::Release);
        let r0 = run(&mut interp.heap, &vm_arm); // warm caches / verify
        let t = Instant::now();
        for _ in 0..reps {
            assert_eq!(run(&mut interp.heap, &vm_arm), r0);
        }
        let vm = t.elapsed();

        // JIT: warm the arm so the background compiler installs native code, then the
        // hook runs it. Poll until tiered (compilation is async).
        use std::sync::atomic::Ordering::Acquire;
        let jit_arm = Arc::new(mk());
        crate::process::yield_now();
        for _ in 0..1000 {
            let b = interp.heap.roots_len();
            interp.heap.push_root(Value::Int(5));
            interp.heap.push_root(Value::Int(0));
            let _ = jit_tier(&jit_arm, &mut interp.heap, b, EnvRoot::Stable(EnvId::GLOBAL));
            interp.heap.truncate_roots(b);
            let c = jit_arm.jit_code.load(Acquire);
            if !c.is_null() && c != crate::jit::BAILED && c != crate::jit::QUEUED {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let r1 = run(&mut interp.heap, &jit_arm);
        assert_eq!(r1, r0, "JIT must match the VM");
        let t = Instant::now();
        for _ in 0..reps {
            assert_eq!(run(&mut interp.heap, &jit_arm), r1);
        }
        let jit = t.elapsed();

        eprintln!(
            "sumto({n},0) x{reps}: VM {vm:?}  JIT {jit:?}  speedup {:.1}x",
            vm.as_secs_f64() / jit.as_secs_f64().max(1e-9)
        );
    }
}

#[test]
fn test_inst_size() {
    eprintln!("Inst size: {}", std::mem::size_of::<Inst>());
    // This test always passes — it just prints the size.
    assert!(true);
}
