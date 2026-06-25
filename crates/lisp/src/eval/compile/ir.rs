//! IR types and bytecode definitions for the closure-compiling VM.
//! Extracted from mod.rs for navigability — imports everything from the parent module.

use super::*;

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
    // `max`/`min` (perf): replaces the prelude's variadic fold-over-closure with a
    // single native + JIT-inlined `select` instruction. Eliminates ~2 heap allocs
    // per 2-arg call (one cons cell for `xs` + one closure for fold's lambda).
    Max,
    Min,
    // `bit-and`/`bit-or`/`bit-xor` (perf): plain bitwise builtins; turning them
    // into PrimOps removes the non-tail Call they emit in self-tail loops (e.g.
    // sort's `gen`), which unblocks int register-carry for the loop variables.
    // Int-only fast path; non-Int (BigInt) defers to the native builtin.
    BitAnd,
    BitOr,
    BitXor,
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
    IsNil,
    IsPair,
    IsEmpty,
}

impl PrimOp1 {
    pub(super) fn from_native_name(name: &str) -> Option<PrimOp1> {
        Some(match name {
            "first" => PrimOp1::First,
            "rest" => PrimOp1::Rest,
            "nil?" => PrimOp1::IsNil,
            "pair?" => PrimOp1::IsPair,
            "empty?" => PrimOp1::IsEmpty,
            _ => return None,
        })
    }
}

impl PrimOp {
    pub(super) fn from_native_name(name: &str) -> Option<PrimOp> {
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
            "max" => PrimOp::Max,
            "min" => PrimOp::Min,
            "bit-and" => PrimOp::BitAnd,
            "bit-or" => PrimOp::BitOr,
            "bit-xor" => PrimOp::BitXor,
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
    pub(super) fn new(v: Value) -> ConstVal {
        match v.unpack() {
            ValueRef::Str(id) => ConstVal::Handle {
                kind: HandleKind::Str,
                bits: AtomicU64::new(id.0),
            },
            ValueRef::BigInt(id) => ConstVal::Handle {
                kind: HandleKind::BigInt,
                bits: AtomicU64::new(id.0),
            },
            ValueRef::Pair(id) => ConstVal::Handle {
                kind: HandleKind::Pair,
                bits: AtomicU64::new(id.0),
            },
            ValueRef::Vector(id) => ConstVal::Handle {
                kind: HandleKind::Vector,
                bits: AtomicU64::new(id.0),
            },
            ValueRef::Map(id) => ConstVal::Handle {
                kind: HandleKind::Map,
                bits: AtomicU64::new(id.0),
            },
            ValueRef::Rope(id) => ConstVal::Handle {
                kind: HandleKind::Rope,
                bits: AtomicU64::new(id.0),
            },
            ValueRef::Fn(id) => ConstVal::Handle {
                kind: HandleKind::Fn,
                bits: AtomicU64::new(id.0),
            },
            ValueRef::Macro(id) => ConstVal::Handle {
                kind: HandleKind::Macro,
                bits: AtomicU64::new(id.0),
            },
            _ => ConstVal::Atom(v),
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
                    HandleKind::Str => Value::str_(StrId(b)),
                    HandleKind::BigInt => Value::bigint(BigIntId(b)),
                    HandleKind::Pair => Value::pair(PairId(b)),
                    HandleKind::Vector => Value::vector(VecId(b)),
                    HandleKind::Map => Value::map(MapId(b)),
                    HandleKind::Rope => Value::rope(RopeId(b)),
                    HandleKind::Fn => Value::func(ClosureId(b)),
                    HandleKind::Macro => Value::macro_(ClosureId(b)),
                }
            }
        }
    }

    /// Rewrite a `Handle` in place through `f` (a `runtime_collect` flush). The kind
    /// is invariant under evacuation (a `Str` stays a `Str`), so only the bits change;
    /// an `Atom` is left untouched. Single-threaded (the owning process), so `Relaxed`
    /// suffices.
    pub(super) fn rewrite(&self, f: &mut dyn FnMut(Value) -> Value) {
        if let ConstVal::Handle { bits, .. } = self {
            let new = f(self.load());
            let nb = match new.unpack() {
                ValueRef::Str(id) => id.0,
                ValueRef::BigInt(id) => id.0,
                ValueRef::Pair(id) => id.0,
                ValueRef::Vector(id) => id.0,
                ValueRef::Map(id) => id.0,
                ValueRef::Rope(id) => id.0,
                ValueRef::Fn(id) | ValueRef::Macro(id) => id.0,
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
        file: Option<std::sync::Arc<str>>,
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
    SelfCall { args: Box<[Node]>, pos: Option<Pos> },
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
    /// `(%try body_fn handler_fn)` desugared inline: body is the thunk's
    /// unwrapped body, bind_slot is the frame slot for the caught exception,
    /// handler is the handler body. Both run via exec_value in the same frame.
    TryCatch {
        body: Box<Node>,
        bind_slot: usize,
        handler: Box<Node>,
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
    /// Shared-JIT key (the spawn lever, ADR-101): `Some((runtime_id, argc))` for a
    /// simple fixed-arity **RUNTIME/PRELUDE** closure arm — the stable identity under
    /// which this arm's compiled native code can be shared across all processes of a
    /// runtime. Every process recompiles the same bytecode from the same shared
    /// closure, and the JIT'd native code embeds no per-process state (the subset's
    /// only consts are immediates; globals resolve via callbacks; any embedded global
    /// is epoch-guarded), so the code pointer is interchangeable between processes.
    /// `None` for LOCAL closures (recycled handles) and optional/rest arms (no
    /// unambiguous `(id, argc)` key). When set, [`jit_tier`] installs an epoch-current
    /// entry from [`RuntimeCode`]'s shared cache instead of re-tiering + recompiling
    /// the arm in every process — without it, N spawned workers each recompile + swamp
    /// the background compiler, so most run interpreted (the spawn-14× cause).
    pub share_key: Option<(u64, u16)>,
    /// True once this process has published its native code to the shared cache (or
    /// installed the code *from* it) — so the publish costs one lock acquire per
    /// arm-instance, not one per call. Reset when the arm is epoch-invalidated so the
    /// recompiled code re-publishes.
    pub shared_published: std::sync::atomic::AtomicBool,
    /// Captured enclosing-lexical names, in capture-slot order (#3 lexical addressing).
    /// Empty for a top-level / non-capturing arm. When non-empty, each name occupies a
    /// **capture slot** at `[capture_base + k]` where `capture_base = nrequired +
    /// noptional + (rest_slot.is_some())`; the body resolves the name to that
    /// `Node::Local(slot)` instead of an `env_get` symbol-scan, and [`push_frame`] fills
    /// the slot from the closure's captured env at call setup (an index fast-path for a
    /// flat capture frame — the VM-built common case — with an `env_get`-by-name fallback
    /// for a chained/tree-walker env, so it's correct in both engines).
    pub capture_names: Box<[Symbol]>,
    /// Recursive self-inlining (Phase B, the two-stage tiering upgrade, devlog
    /// 2026-06-17). `Some(name)` when this arm qualifies as a top-level no-capture
    /// recursive `defn` whose body the JIT can splice depth-1 of into its own frame
    /// (removing the per-call protocol for the inlined level — the fib lever). The
    /// VM keeps the ORIGINAL small `body`/`chunk`/`nslots`; the inlined body is
    /// re-derived fresh in `jit_lower_arm` (`shift_slots` clone → `inline_self_calls`),
    /// so nothing here grows the interpreted frame. `None` = the arm doesn't qualify
    /// (the common case).
    #[cfg(feature = "jit")]
    pub inline_name: Option<Symbol>,
    /// The per-block slot stride for inlining (`m` = the original arm's slot
    /// high-water mark `scope.max`); each inlined call site occupies a disjoint
    /// shifted range `[m*i .. m*(i+1))`. Only meaningful when `inline_name.is_some()`.
    #[cfg(feature = "jit")]
    pub inline_stride: usize,
    /// The inlined arm's frame high-water mark (the spliced layout's `scope.max`
    /// plus its own chunk spill reserve) — computed once at arm construction by
    /// running the inliner on a CLONE of `body` (then discarded). The frame the
    /// **inlined** native version runs against is `[base .. base+inline_nslots)`;
    /// the small native + the VM use the original (smaller) `nslots`. Per-engine
    /// frame sizing keys on which version is installed (`inline_installed`).
    #[cfg(feature = "jit")]
    pub inline_nslots: usize,
    /// Two-stage tiering: the **deferred** inlined native code pointer (null =
    /// not compiled, `QUEUED`, `BAILED`, else callable). Compiled as a separate,
    /// lower-priority background upgrade *after* the small original arm has tiered
    /// — so short-lived processes (spawn's `fib 15`) finish on the small native and
    /// never wait on the bigger inlined compile, while a long-lived workload (fib 35)
    /// picks up the inlined upgrade once it lands. Installed into `jit_code` by
    /// `jit_tier` (epoch-bumped swap), at which point `inline_installed` flips true.
    #[cfg(feature = "jit")]
    pub inline_code: std::sync::atomic::AtomicPtr<u8>,
    /// True once the deferred inlined compile has been enqueued (so we enqueue it at
    /// most once per arm-instance per epoch). Reset on epoch invalidation.
    #[cfg(feature = "jit")]
    pub inline_queued: std::sync::atomic::AtomicBool,
    /// True once the inlined native code has been installed into `jit_code` (the
    /// small→inlined swap fired). **This is the per-engine frame-sizing key**: while
    /// false the active native is the small original arm (frame `nslots`); once true
    /// the active native is the inlined arm (frame `inline_nslots`). One-way false→true
    /// within an epoch; reset on epoch invalidation. See `active_nslots`.
    #[cfg(feature = "jit")]
    pub inline_installed: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "jit")]
impl CompiledArm {
    /// The frame size the **currently installed** native version runs against —
    /// the per-engine frame-sizing key for two-stage tiering. The VM always uses the
    /// original `nslots` (it runs the original `chunk`); only a native entry consults
    /// this. Small native → `nslots`; inlined native (post-swap) → `inline_nslots`.
    #[inline]
    pub fn active_nslots(&self) -> usize {
        if self.inline_installed.load(std::sync::atomic::Ordering::Acquire) {
            self.inline_nslots
        } else {
            self.nslots
        }
    }
}

/// One arm of a closure: its arity shape plus the compiled body **if** it was
/// VM-eligible. Every arm is recorded — even ones that defer — so [`arm_for`]
/// reproduces [`Closure::select_arm`](crate::core::value::Closure::select_arm)
/// *exactly* (picks the same arm) before checking whether that arm can run on the
/// VM. Without the full table a variadic arm (which accepts a *range* of arities)
/// could be picked where the tree-walker would pick an overlapping fixed arm — a
/// silent wrong-arm miscompile.
pub(super) struct ArmSpec {
    pub(super) nrequired: usize,
    pub(super) noptional: usize,
    pub(super) has_rest: bool,
    pub(super) compiled: Option<Arc<CompiledArm>>,
}

impl ArmSpec {
    pub(super) fn accepts(&self, argc: usize) -> bool {
        argc >= self.nrequired && (self.has_rest || argc <= self.nrequired + self.noptional)
    }
}

/// A compiled closure: every arm's arity shape + (if VM-eligible) its compiled body.
pub struct CompiledClosure {
    pub(super) arms: Vec<ArmSpec>,
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
pub(super) enum Step {
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
pub(super) enum ChunkExit {
    Done(Value),
    Tail {
        arm: Arc<CompiledArm>,
        args: SmallVec<[Value; 4]>,
        genv: EnvId,
    },
    Call {
        arm: Arc<CompiledArm>,
        args: SmallVec<[Value; 4]>,
        genv: EnvId,
    },
    /// A clean `receive` on an empty mailbox raised `Control::Suspend` through the
    /// `%receive` native (state-capture path, ADR-100 §8). `exec_chunk` rewound `ip`
    /// so re-entry re-runs the suspending `Inst::Call`, leaving the callee + args on
    /// the operand stack untouched; the driver ([`vm_run_bc`]) captures the whole
    /// frame stack as a [`Suspended`] and returns it to the scheduler to park. Produced
    /// only by a clean top-level `receive` (a native-nested one blocks the worker, §7.4).
    Suspend {
        deadline: Option<std::time::Instant>,
    },
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
    /// Only ever constructed under `--features jit`; dead (but kept for the match) in a
    /// non-jit build such as `brood-lsp`.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    SelfTail,
}


/// Walk a compiled `Node` tree, rewriting every embedded movable handle
/// (a `Const` literal or a `MakeClosure` `fn_rest`) in place through `f`. The crux of
/// the live-arm fixup: a RUNTIME compaction evacuates the code region, but the `Arc`'d
/// node trees of the **executing** arms are off the GC root graph (and held by
/// `&Node` on the Rust stack, so the `Arc` can't be swapped). `runtime_collect` walks
/// the live arms (registered in `Heap::live_vm_arms`) and calls this with `f` =
/// `flush_rt_value` so their handles point into the compacted region. Atoms and child
/// structure are untouched; only `ConstVal::Handle` bits move.
pub(super) fn rewrite_node(node: &Node, f: &mut dyn FnMut(Value) -> Value) {
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
        Node::MakeClosure {
            fn_rest,
            captures,
            self_name: _,
        } => {
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
        Node::TryCatch { body, handler, .. } => {
            rewrite_node(body, f);
            rewrite_node(handler, f);
        }
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
pub(super) fn rewrite_chunk(chunk: &Chunk, f: &mut dyn FnMut(Value) -> Value) {
    for inst in chunk.code.iter() {
        match inst {
            Inst::Const(cv) => cv.rewrite(f),
            Inst::MakeClosure { fn_rest, .. } => fn_rest.rewrite(f),
            _ => {}
        }
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
/// Non-owning raw pointer into a `Node` tree owned by a `CompiledArm`.
/// Used by `Inst::TryCatch` to avoid double-rewriting: `rewrite_node(arm.body)`
/// rewrites the pointed-to nodes in place; `rewrite_chunk` skips `Inst::TryCatch`.
pub struct NodePtr(pub(crate) NonNull<Node>);
// SAFETY: Node is Send+Sync (all interior mutability is through AtomicU64).
unsafe impl Send for NodePtr {}
unsafe impl Sync for NodePtr {}

/// the operand region of `Heap::roots` that sits just above the activation frame's
/// slots (`base..base+nslots`). Frame slots are read/written by absolute index
/// (`Local`/`SetLocal`); everything else is push/pop. The semantics of each arm
/// mirror the matching [`Node`] case in `exec_value`/`exec_node` exactly.
pub enum Inst {
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
    Prim1 {
        op: PrimOp1,
        head: Symbol,
        guard: AtomicU64,
        pos: Option<Pos>,
    },
    /// Inlined 2-ary primitive (`+`/`<`/`=`/`cons`/…): replace the top two operands
    /// with the result, or fall back to a general call on `head`. Mirrors
    /// `Node::Prim2`. (No `broot`: both operands are already rooted on the operand
    /// stack, so the stack machine never needs the explicit-root dance.)
    Prim2 {
        op: PrimOp,
        map: [u8; 2],
        head: Symbol,
        guard: AtomicU64,
        pos: Option<Pos>,
    },
    /// Fused Prim2 where both operands are frame locals — reads `slot_a` and `slot_b`
    /// directly, bypassing 2 intermediate root-stack pushes. The inline fast path
    /// pushes only the result; the fallback pushes both slots before dispatching.
    Prim2SlotSlot {
        op: PrimOp,
        map: [u8; 2],
        slot_a: usize,
        slot_b: usize,
        head: Symbol,
        guard: AtomicU64,
        pos: Option<Pos>,
    },
    /// Fused Prim2 where operand A is a frame local and B is a literal integer.
    /// Covers the very common `(+ slot 1)` / `(- slot 1)` / `(< slot k)` shape.
    /// Uses i64 directly (not ConstVal) so this variant stays under MakeClosure's
    /// size, avoiding Inst enum bloat that would widen every instruction.
    /// `swapped` records that the operands came from `(op Const Local)` — the fusion
    /// stored the *local* as `slot_a` and the *const* as `int_b` (with an inverted `map`
    /// so the inline path still routes correctly). The slow-path dispatch fallback calls
    /// the user `head`, which needs the **original** call order `(head Const Local)`, so
    /// it must un-swap. `false` for the `(op Local Const)` case (already in order).
    Prim2SlotInt {
        op: PrimOp,
        map: [u8; 2],
        slot_a: usize,
        int_b: i64,
        swapped: bool,
        head: Symbol,
        guard: AtomicU64,
        pos: Option<Pos>,
    },
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
    Call {
        argc: usize,
        tail: bool,
        pos: Option<Pos>,
        site: u32,
        head: Option<Symbol>,
    },
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
    MakeClosure {
        fn_rest: ConstVal,
        names: Box<[Symbol]>,
        self_name: Option<Symbol>,
    },
    /// Inline try/catch: run body via exec_value; on non-control error write
    /// the caught value to bind_slot and run handler; push result. NodePtrs
    /// point into arm.body — rewrite_chunk skips them; rewrite_node handles them.
    TryCatch {
        body: NodePtr,
        bind_slot: usize,
        handler: NodePtr,
    },
}

impl Inst {
    /// Compact name for `BROOD_VM_TRACE` output — variant + key operands,
    /// no AtomicU64 (not Debug-able).
    #[cfg(debug_assertions)]
    pub(crate) fn trace_name(&self) -> String {
        match self {
            Inst::Const(cv) => format!("Const({})", match cv.load().unpack() {
                crate::core::value::ValueRef::Int(n) => format!("int:{n}"),
                crate::core::value::ValueRef::Bool(b) => format!("bool:{b}"),
                crate::core::value::ValueRef::Nil => "nil".into(),
                crate::core::value::ValueRef::Float(f) => format!("float:{f}"),
                _ => "handle".into(),
            }),
            Inst::Local(i) => format!("Local({i})"),
            Inst::Global(s) => format!("Global({})", crate::core::value::symbol_name_ref(*s)),
            Inst::GlobalIc { sym, site } => format!("GlobalIc({}, site={site})", crate::core::value::symbol_name_ref(*sym)),
            Inst::Pop => "Pop".into(),
            Inst::SetLocal(i) => format!("SetLocal({i})"),
            Inst::Jump(t) => format!("Jump({t})"),
            Inst::JumpIfFalse(t) => format!("JumpIfFalse({t})"),
            Inst::MakeVector(n) => format!("MakeVector({n})"),
            Inst::MakeMap(n) => format!("MakeMap({n})"),
            Inst::Prim1 { op, head, .. } => format!("Prim1({op:?}, {})", crate::core::value::symbol_name_ref(*head)),
            Inst::Prim2 { op, head, .. } => format!("Prim2({op:?}, {})", crate::core::value::symbol_name_ref(*head)),
            Inst::Prim2SlotSlot { op, slot_a, slot_b, head, .. } => format!("Prim2SlotSlot({op:?}, s{slot_a},s{slot_b}, {})", crate::core::value::symbol_name_ref(*head)),
            Inst::Prim2SlotInt { op, slot_a, int_b, head, .. } => format!("Prim2SlotInt({op:?}, s{slot_a},{int_b}, {})", crate::core::value::symbol_name_ref(*head)),
            Inst::Call { argc, tail, head, .. } => format!("Call(argc={argc}, tail={tail}, head={})",
                head.map(|s| crate::core::value::symbol_name_ref(s)).unwrap_or("computed")),
            Inst::SelfCall { argc } => format!("SelfCall({argc})"),
            Inst::MakeClosure { names, .. } => format!("MakeClosure(captures={})", names.len()),
            Inst::TryCatch { .. } => "TryCatch".into(),
        }
    }
}

/// A compiled-to-bytecode arm body: a flat instruction stream evaluated by
/// [`exec_chunk`], leaving the body's single value on top of the operand stack.
/// `Send + Sync` (its `Inst`s hold only atoms, symbols, indices, and atomics), so it
/// rides in the `Arc<CompiledArm>` cached in a `Send` `Heap`.
pub struct Chunk {
    pub(crate) code: Vec<Inst>,
}
