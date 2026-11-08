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
    // (through the prelude `nth`) any indexed vector walk. The JIT lowers it now
    // (`jit_plan::chunk_in_jit_subset` admits it, and an inline vector read is one of
    // the two unboxing signals `plan_general_lowering` looks for — it is what rules
    // `bintree`/`matmul` into the general lowering).
    VectorRef,
    // `max`/`min` (perf): replaces the prelude's variadic fold-over-closure with a
    // single native + JIT-inlined `select` instruction. Eliminates ~2 heap allocs
    // per 2-arg call (one cons cell for `xs` + one closure for fold's lambda).
    Max,
    Min,
    // `bit/and`/`bit/or`/`bit/xor` (perf): plain bitwise builtins; turning them
    // into PrimOps removes the non-tail Call they emit in self-tail loops (e.g.
    // sort's `gen`), which unblocks int register-carry for the loop variables.
    // Int-only fast path; non-Int (BigInt) defers to the native builtin.
    BitAnd,
    BitOr,
    BitXor,
    // `table-has?` / 2-arg `table-get` (perf): the Table workhorses (`sieve`'s marks,
    // the regex DFA memo, counters). Like `Cons`/`VectorRef` they need the heap, so
    // they're handled in the exec arm; a non-Table first operand defers to the native
    // so type errors stay bit-identical. Removing the per-op native Call is what lets
    // a table-driven self-tail loop keep its register carry (same rationale as the
    // bitops above), and the JIT lowers them as a single runtime-callback call.
    TableHas,
    TableGet,
    /// `(get m k)` on a **CHAMP map** — the one collection read with no primitive.
    /// Vectors have [`PrimOp::VectorRef`] and the mutable table has [`PrimOp::TableGet`];
    /// a map read compiled to an ordinary call, which is why no body that reads a field
    /// could ever be leaf-inlined (`leaf_body_qualifies` requires a call-free body) — not
    /// a `defrecord` accessor, not an ability impl, not the map-shaped helpers that are
    /// most of Brood. `get`'s own source calls its map branch "the hottest path in the
    /// language (4796 call sites)".
    ///
    /// Inlines **only a present, non-nil value**; a non-map receiver, an absent key and a
    /// stored `nil` all defer to the real `get`, which owns the set / string / integer-index
    /// branches and the `%lookup-miss` path (a record whose contents are not its fields
    /// resolves its miss through the `Lookup` ability — policy that stays in Brood).
    ///
    /// `BROOD_NO_MAPGET=1` disables the lowering (`debug_flags::map_get_lowering`).
    MapGet,
    /// `(%contract-await ret call)` — answers `call`'s value. A contract shim wraps the
    /// call to its original in it, and its `Inst::Prim2` is the MARK a suspended frame is
    /// recognised by: a frame whose resume point is this instruction is a shim waiting on
    /// its original, about to check the result against `ret`, the operand directly below
    /// the callee's frame. `%contract-tail?` reads exactly that (KI-216). Never lowered by
    /// the JIT: a chunk holding it stays on the VM, where the frame stack is the driver's.
    ContractAwait,
}

/// A 3-ary inlinable primitive — the `PrimOp` family's arity-3 sibling. Three members:
/// `table-put` (the write half of the Table workhorses — `sieve`'s 2.5M marks), and the
/// two map ops process-shaped code lives on (ADR-368): `(get m k default)` and
/// `(assoc coll k v)`, head-keyed like `PrimOp::MapGet` and accepted only while the head
/// is the PRELUDE closure. Same discipline as `PrimOp`: the op needs the heap so it runs in
/// the exec arm; a receiver the inline rule does not cover (or a redefined head, via the
/// epoch guard) defers to the dispatched wrapper so every other branch and every error
/// stays in Brood; the JIT lowers each as one runtime callback. Kept a separate enum (not
/// `PrimOp`) so every existing 2-ary match stays exhaustive without dead arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimOp3 {
    TablePut,
    /// `(get m k default)` on a CHAMP map — `Heap::map_get3_inline` is the rule: a present
    /// non-nil value is answered, and so is a plain map's miss (its stored nil, else the
    /// default); a RECORD's miss declines to `get`, whose `%lookup-miss` asks the `Lookup`
    /// ability whatever the default (KI-298). Pure: never allocates.
    MapGet3,
    /// `(assoc m k v)` on a CHAMP map — `Heap::map_assoc`, a fresh path-copied trie. A
    /// vector receiver declines to the wrapper's `%vector-assoc` branch. ALLOCATES (never
    /// collects), so the planner lists it under `inst_may_allocate`/`inst_allocates_hot`.
    MapAssoc,
    /// `(nth v i default)` on a vector with a fixnum index — `Heap::nth3_inline` is the
    /// rule: the element in range, else the default. Bytes, lists, records and a bad index
    /// decline to the prelude `nth`, which stays the definition. Pure: never allocates.
    /// The 2-arity `(nth v i)` is [`PrimOp::VectorRef`]; this is its default-taking twin,
    /// which was a full Brood call (~1 200 instructions from native code, 2026-10-25).
    VectorRefOr,
}

impl PrimOp3 {
    pub(super) fn from_native_name(name: &str) -> Option<PrimOp3> {
        (name == kw::TABLE_PUT).then_some(PrimOp3::TablePut)
    }
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
    // Prelude `sqrt` (special-cased in `resolve_prim1`, like `nth` → `VectorRef`):
    // the wrapper guards negatives (error) and zero (0.0), so the inline covers ONLY
    // x > 0 — one IEEE `fsqrt` in the JIT, `f64::sqrt` in the VM (identical, both
    // correctly rounded) — and every other shape falls back to dispatching the real
    // wrapper. This is nbody's 2M-calls-per-run `(sqrt dsq)` without the per-call
    // wrapper activation (whose error branch keeps its own arm off the JIT).
    Sqrt,
    // `type-of`: total over every operand (tag → cached keyword id, no error, no
    // allocation), so the inline covers ALL shapes — a tag read in the VM, a
    // tag-indexed table load in the JIT. This is what every type predicate
    // (`vector?`/`int?`/`string?`/…) bottoms out in (they are Brood wrappers over
    // `type-of` — hit per candidate by `match`/`receive` pattern dispatch and per
    // element by the seq predicates), and making it call-free also makes those
    // one-line wrappers leaf-inlinable (their bodies lose their last `Call`).
    TypeOf,
    /// A prelude **type predicate** — `vector?`, `map?`, `string?`, `int?`, `keyword?`, … —
    /// carrying the `type-of` keyword it tests for. Every one of them is the one-line
    /// wrapper `(%eq (type-of x) :kw)`, which `resolve_prim1` recognises STRUCTURALLY (the
    /// `sqrt` discipline: the shape, not the name, earns the inline, and a rebind that
    /// changes the shape drops it at the next epoch check). Otherwise each is a full
    /// Brood call where a tag read suffices — and they are what every `match`
    /// / `receive` / destructuring `let` clause tests first, what `get` and `assoc` dispatch
    /// on, and what the seq predicates apply per element. Total over every operand: a tag
    /// byte compared to a constant, the same `type_of_kw_table` load `TypeOf` uses in the
    /// JIT, so the collapsing rules (`BigInt` → `int`, `Range`/`SeqView` → `pair`) hold by
    /// construction.
    TypeIs(Symbol),
    /// `(%vector-length v)`: the length of a vector, inline. The other half of the
    /// pattern compiler's vector test (`(and (vector? m) (%eq (%vector-length m) 3))`),
    /// which `%vector-ref` then reads through [`PrimOp::VectorRef`]. A non-vector
    /// defers to the native for its exact type error.
    VectorLen,
    /// The prelude `count` (special-cased in `resolve_prim1`, like `nth` → `VectorRef`):
    /// the sizes the heap already holds — a vector, string, range, bytes, set, nil, a
    /// non-record map, a proper list — answered inline by [`Heap::count_inline`]. A
    /// record, seq-view, rope, table, improper list or `Seqable` impl dispatches the real
    /// `count`, which stays the definition. Every `count` was otherwise a full Brood call
    /// (~3 000 instructions from native code, 2026-10-25) — and its body builds a closure,
    /// so it never tiers itself.
    ///
    /// [`Heap::count_inline`]: crate::core::heap::Heap::count_inline
    Count,
    /// `(%contract-tail? ret)` — is the frame directly beneath this one a contract shim
    /// suspended at its [`PrimOp::ContractAwait`] with a declared result EQUAL to `ret`?
    /// Then this activation's result is already going to be checked against `ret`, and the
    /// shim may call its original in tail position instead of stacking an identical check
    /// per call (KI-216, ADR-396). Only the driver can see the frame stack, so the VM's exec
    /// arm exits with [`ChunkExit::ContractProbe`]; every other engine answers `false`,
    /// which is always sound — the shim then checks as it always did.
    ContractTail,
}

impl PrimOp1 {
    pub(super) fn from_native_name(name: &str) -> Option<PrimOp1> {
        Some(match name {
            "first" => PrimOp1::First,
            "rest" => PrimOp1::Rest,
            "nil?" => PrimOp1::IsNil,
            "pair?" => PrimOp1::IsPair,
            "empty?" => PrimOp1::IsEmpty,
            "type-of" => PrimOp1::TypeOf,
            "%vector-length" => PrimOp1::VectorLen,
            "%contract-tail?" => PrimOp1::ContractTail,
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
            "%rem" => PrimOp::Rem,
            "%div" => PrimOp::Div,
            "%quot" => PrimOp::Quot,
            "cons" => PrimOp::Cons,
            // Must match the native's CURRENT spelling: a stale name here silently kills the
            // direct inline for every pattern-match element read.
            "%vector-ref" => PrimOp::VectorRef,
            "%max" => PrimOp::Max,
            "%min" => PrimOp::Min,
            "bit/and" => PrimOp::BitAnd,
            "bit/or" => PrimOp::BitOr,
            "bit/xor" => PrimOp::BitXor,
            _ if name == kw::TABLE_HAS => PrimOp::TableHas,
            _ if name == kw::TABLE_GET => PrimOp::TableGet,
            _ if name == kw::EQ_PRIM => PrimOp::Eq,
            "%contract-await" => PrimOp::ContractAwait,
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
    Set,
    Rope,
    Fn,
    Macro,
}

/// A constant baked into a compiled [`Node`] (a `Const` literal or a
/// `MakeClosure` `fn_rest`). Either a truly-immovable **atom** kept inline, or a
/// movable **heap handle** stored as `(kind, AtomicU64)` so the RUNTIME compactor
/// ([`Heap::runtime_collect`](crate::core::heap::Heap::runtime_collect)) can rewrite
/// it **in place** — the `Node` tree lives behind an `Arc` that is read by
/// `&Node`, so the `Arc` can't be swapped for a relocated copy; the handle bits must
/// move under the live reference. The atomic also keeps `ConstVal`/`Node`
/// `Send + Sync` (required because `Arc<CompiledArm>` is cached in a `Send` `Heap`).
/// A plain `Value` is not enough: the compactor moves promoted handles, and a stale
/// one reads as a slab-OOB / corruption.
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
            ValueRef::Set(id) => ConstVal::Handle {
                kind: HandleKind::Set,
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
                    HandleKind::Set => Value::set(MapId(b)),
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
                ValueRef::Map(id) | ValueRef::Set(id) => id.0,
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
/// `Defer` node: a VM-run body is *fully* compiled, which is what lets its bytecode
/// never need an `EnvId` for locals.
pub enum Node {
    /// A self-evaluating literal (number, bool, nil, string, keyword), as a
    /// [`ConstVal`]: an immovable atom inline, or a movable RUNTIME/PRELUDE heap
    /// handle as `(kind, AtomicU64)`. Construct only via [`const_node`], which
    /// `promote`s out of LOCAL first. The cached `Node` tree is an `Arc` off the GC
    /// root graph, so the collector never traces it — a LOCAL handle here would
    /// dangle (use-after-GC), and a *RUNTIME* handle would
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
        /// The callee is staged ahead of the args — see [`Inst::Call::staged`] (KI-19).
        staged: bool,
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
        /// The module the enclosing arm was compiled under (`Scope::module`), which
        /// the built closure inherits as its contract boundary (ADR-383). A symbol.
        module: Option<Symbol>,
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
    /// An inlined 3-ary primitive (`table-put`): args in source order; no arg-map
    /// (its one member is a direct native — no wrapper reordering to normalise).
    /// Same guard discipline as [`Node::Prim2`].
    Prim3 {
        op: PrimOp3,
        a: Box<Node>,
        b: Box<Node>,
        c: Box<Node>,
        head: Symbol,
        guard: AtomicU64,
        pos: Option<Pos>,
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
    /// handler is the handler body. Both run in the same frame.
    TryCatch {
        body: Box<Node>,
        bind_slot: usize,
        handler: Box<Node>,
    },
    /// `(%binding '(n…) vals (fn () body…))` desugared inline (ADR-402): `vals` builds the
    /// vector of new values, the names are pushed on the process's dynamic stack, and
    /// `body` runs in this frame for the extent of the binding.
    Binding {
        names: Box<[Symbol]>,
        vals: Box<Node>,
        body: Box<Node>,
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
/// Mint a fresh [`CompiledArm::uid`]. Process-wide (not per-runtime): uniqueness is
/// all that matters, and a single relaxed counter is cheaper than plumbing a
/// per-runtime one through every construction site.
pub(crate) fn next_arm_uid() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct CompiledArm {
    /// Required params — `argv[0..nrequired]` fill slots `0..nrequired`. Selection
    /// guarantees `argc >= nrequired`, so they're always present.
    pub nrequired: usize,
    /// Count of `&optional` params. A provided arg fills the slot; a missing one
    /// takes its default (see `optional_flags`).
    pub noptional: usize,
    /// Per-optional, indexed `0..noptional`: the hidden frame slot `push_frame` sets to
    /// `true` when that optional's argument is missing, or `u32::MAX` for a nil default
    /// (the slot is just left nil). A real default is compiled into the arm's own body, as
    /// a prologue that tests the flag and sets the slot — left to right, so a later default
    /// sees earlier ones (matching the tree-walker); see `compile_arm`.
    pub optional_flags: Box<[u32]>,
    /// `&` rest param's slot, if any: collects `argv[nrequired+noptional..]` into a
    /// fresh list.
    pub rest_slot: Option<usize>,
    /// Total frame slots (params + optionals + rest + `let`/`letrec` binders).
    pub nslots: usize,
    /// Call-site inline-cache slots this arm's body uses, numbered **arm-relative**
    /// from 0 (ADR-175 Phase A). A process activating this arm resolves a contiguous
    /// block of this many slots in its own IC tables ([`Heap::vm_arm_block`]) and runs
    /// the arm with the block's base in `Heap::cur_ic_base` — so the *same* compiled
    /// arm (and its JIT'd native code, whose site ids are baked in) is valid in every
    /// process, each against its own IC block. Absolute sites would make a shared arm's
    /// ICs silently dead in every process but the compiling one.
    pub nsites: u32,
    /// Global-read IC slots, arm-relative from 0 — the `Node::GlobalIc` counterpart of
    /// [`Self::nsites`], indexing `Heap::vm_global_ics` via `Heap::cur_gic_base`. Built by
    /// [`gsite_reservation`]: the body's own global reads, then one slot per call site for
    /// native code's tail-call head ([`Self::tail_head_gsite`]).
    pub ngsites: u32,
    /// Process-independent identity for this arm instance, from a global counter —
    /// the key under which each process tracks its IC block for this arm
    /// ([`Heap::vm_arm_block`]). An `Arc` pointer would ABA after a drop; this can't.
    pub uid: u64,
    /// Source position per arm-relative call site (indexed by site id), recorded at
    /// compile time. `Heap::vm_arm_block` copies these into the process's absolute
    /// `dbg_site_pos` table (debug builds) so the JIT stale-operand diagnostics can
    /// name a site's `.blsp` file:line. Empty for synthetic/test arms.
    pub site_pos: Box<[Option<(crate::error::Pos, Option<std::sync::Arc<str>>)>]>,
    /// Where each global read is written, keyed by its global-read site: a `GlobalIc`
    /// carries no position, so an unbound name reports from here — a STAGED head (KI-19) its
    /// call's position, any other read the innermost positioned form around it (an unbound
    /// name in argument position used to report at whatever call reached the arm). Cold —
    /// read only when that resolution fails.
    pub staged_head_pos: Box<[(u32, crate::error::Pos)]>,
    /// The head of each call whose head is a LOCAL, keyed by the call's position: the
    /// bytecode calls a slot, which carries no name, so `cannot call non-function` names the
    /// local from here (`eval::name_local_callee`). Cold — read only on that error.
    pub local_call_heads: Box<[(crate::error::Pos, crate::core::value::Symbol)]>,
    /// The arm receives at its head (`jit_plan::receive_at_head`, ADR-410): its one `%receive`
    /// follows only loads, so re-running it from ip 0 re-runs just that `%receive`. A park
    /// then rewinds the frame to ip 0 (the wake re-tiers into native code), and only the
    /// driver may enter its native code — a `receive` under any other native entry must block.
    pub receive_at_head: bool,
    pub body: Node,
    /// The body compiled to flat **bytecode** (`Chunk`). [`vm_run_bc`] runs this — the
    /// sole VM executor (ADR-100 Stage 5). `compile_arm` always fills it (every
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
    /// Why `jit_code` is `BAILED`, set by [`CompiledArm::latch_bailed`]: the reason its
    /// `[jit-bail]` trace names (`hosts-receive`, `call-mediated-boxed`, …), readable after
    /// the fact through `%jit-arm-state`'s `:reason`.
    pub bail_reason: std::sync::OnceLock<Box<str>>,
    /// The runtime's [`code_relocations`](crate::core::heap::Heap::code_relocations) when
    /// `compile_epoch` was stamped: equal readings mean no RUNTIME handle the native code
    /// baked in has moved, the one thing a moved epoch can mean that re-reading the
    /// bindings cannot check (`jit_runtime::native_still_valid`).
    pub compile_relocations: std::sync::atomic::AtomicU64,
    /// How many times [`reset_native_state`](crate::eval::compile::jit_runtime) has thrown
    /// this arm's native code away. A lowering in flight installs only while this is what it
    /// read when it began: `compile_epoch` cannot say, since an entry that finds the code
    /// still valid moves it forward without a reset (KI-303).
    pub native_resets: std::sync::atomic::AtomicU64,
    /// Deopt feedback (the dynamic replacement for the static "call-mediated boxed
    /// work" lowering gate). `deopt_watch` is true for a non-loop arm with ≥1
    /// non-tail call — the shape whose native profitability only runtime behaviour
    /// can decide (a `SelfCall` loop arm is never watched: its deopt follows
    /// productive native iterations, e.g. an overflow at the end of a long int
    /// loop, so counting activations would mis-bail it). `jit_deopts` counts
    /// **consecutive** type-deopts: a successful native completion resets it, a
    /// deopt bumps it, and 16 in a row store `BAILED` — a persistent thrasher
    /// (paying native entry + deopt + a full VM re-run per call) self-heals onto the VM,
    /// while an arm with only occasional deopts keeps its native code. Unwatched
    /// arms pay a single bool test; watched healthy arms one relaxed load.
    pub deopt_watch: bool,
    pub jit_deopts: std::sync::atomic::AtomicU32,
    /// Every type-deopt of this arm, CUMULATIVE and watched or not — the probe
    /// `%jit-arm-state` reports as `:deopts-total`. `jit_deopts` above is the feedback's
    /// consecutive count and only moves for a watched arm, so a `SelfCall` loop that
    /// deopts at entry on every activation reads `:deopts 0` there. One relaxed
    /// increment on the deopt path, which is cold by definition.
    pub jit_deopts_total: std::sync::atomic::AtomicU32,
    /// Param slots the tier-time profile typed `Int`/`Float` that activations have since
    /// contradicted AT ENTRY (deopt reason 106, the register-carry tag check): bit `k` set
    /// means slot `k` is polymorphic and the next lowering must leave it boxed on the frame
    /// (`profile_slot_tags` masks it). A profile is one activation's snapshot — `json/emit
    /// (v acc)` sampled with `v = 1` would carry `v` as an i64 and deopt on every map it is
    /// handed, and a `SelfCall` arm has no deopt feedback to latch
    /// it. After `ENTRY_DEOPT_RELOWER` such deopts the arm is reset to untried and re-tiers
    /// with the slot boxed: one recompile per polymorphic slot, never a bail.
    pub jit_poly_slots: std::sync::atomic::AtomicU32,
    /// Entry-tag deopts (reason 106) this arm has taken, cumulative, toward
    /// `ENTRY_DEOPT_RELOWER`. Never reset on the hot path — the count is cold on purpose.
    pub jit_entry_deopts: std::sync::atomic::AtomicU32,
    /// Set once `FLOAT_DEOPT_RELOWER` integer-path arithmetic guards (reasons 20/21) have
    /// deopted on a `Float` operand: the arm computes on floats that arrive through
    /// type-erased reads — `(nth a 0)` on a vector of floats — with no float param or
    /// literal to say so, which is every function of a 2D vector library (`(dot a b)`,
    /// `(add a b)`). The tier-time profile types only params, so such an arm would lower
    /// its `*` on the integer path, deopt on every call and latch BAILED. The next lowering takes the float
    /// context as if a param had profiled `Float` (`has_float_slot`), which is a guess the
    /// `as_f64` tag guards keep sound.
    pub jit_float_context: std::sync::atomic::AtomicBool,
    /// Integer-path guard deopts on a `Float` (toward `FLOAT_DEOPT_RELOWER`), cumulative.
    pub jit_float_deopts: std::sync::atomic::AtomicU32,
    /// Free globals this arm reads that held a `Value::Float` when the arm was
    /// enqueued for tiering — the global-read counterpart of the tier-time
    /// `slot_tags` param profile. The profile snapshots the live *frame*, so it
    /// only ever types **parameters**; an arm whose floats arrive from a `def`'d
    /// constant instead (nbody's `advance-body`, which takes `(b i)` — a vector and
    /// an int — and multiplies by the global `dt`) reads as non-float-context, and
    /// its `(* dt nvx)` lowers to the *integer* path, whose `as_int` tag-check
    /// deopts on every activation. Sixteen consecutive deopts then mark the arm
    /// `BAILED` and nbody's hottest function runs interpreted for the whole run.
    /// Recording the symbols here lets the lowering unbox such a read to an
    /// `Op::Float` behind `as_f64`'s existing tag guard, so a stale or
    /// per-runtime-divergent observation is a deopt, never a miscompile — the same
    /// soundness argument the `has_float_slot` optimism already rests on. Set once,
    /// by whichever process wins the tiering CAS.
    pub float_globals: std::sync::OnceLock<Box<[Symbol]>>,
    /// Does the global named by `dbg_name` still resolve to **this very arm**? Observed at
    /// each tiering election (where a `Heap` is in hand) and read by the unboxed register
    /// worker, which lowers a *non-tail* `(f …)` whose head is `dbg_name` to a direct call
    /// to itself.
    ///
    /// That link is what makes `fib` fast, but `dbg_name` is only the symbol the closure
    /// was first `def`'d under — not evidence the global still binds this closure. Alias
    /// the closure (`(def f h)`), rebind `h`, and call through the alias: the old body's
    /// recursive `(h …)` must see the NEW `h` (late binding, ADR-013), yet the hard link
    /// kept calling the old arm — `(f 12)` gave 12 where every other engine gave 1001.
    /// Re-establishing the link on recompile does not help: the epoch bump invalidates the
    /// arm, and the recompile then bakes the same wrong callee in again.
    ///
    /// An `AtomicBool` rather than a `OnceLock` precisely because it must be *re-observed*:
    /// a `def` bumps the global epoch, `jit_tier` invalidates, and the next election
    /// re-checks before the arm is lowered again. `false` is the safe answer (fall back to
    /// the epoch-guarded IC call), so an unobserved arm simply forgoes the link.
    pub self_global_ok: std::sync::atomic::AtomicBool,
    /// The last [`self_binding_stamp`](super::self_binding_stamp) at which the VM proved
    /// `dbg_name` resolves to **this very arm** — the memo behind `Inst::SelfCall`'s
    /// first-back-edge check. A frame starts at `SELF_UNCHECKED_EPOCH`, so its first
    /// `SelfCall` asks whether the name still names this arm: the frame may have been
    /// entered through an ALIAS after the name was rebound (`(def old f) (def f …)
    /// (old …)`), and then the loop must hand its tail call to the new `f`, as the
    /// tree-walker does. The stamp (runtime tag + global epoch) makes that check one
    /// atomic load and a compare while no `def` has happened since the last proof, so a
    /// short loop called a million times pays the env lookup once per epoch, not once
    /// per activation. `u64::MAX` = never proven.
    pub self_bound_stamp: std::sync::atomic::AtomicU64,
    /// Deopt-resume checkpoint layout: `u32::MAX` = no
    /// checkpointing (no non-tail calls in the chunk — a from-ip-0 re-run is
    /// then effect-free). Otherwise `ckpt_slot` is the frame slot holding the
    /// packed `(resume_ip << 16) | operand_depth` journal (a plain `Value::Int`,
    /// GC-inert), and slots `[ckpt_slot+1, ckpt_slot+1+depth)` hold the journaled
    /// operand stack — written by the JIT'd arm after every completed non-tail
    /// call, reset at entry and on every self-tail back-edge. On an outcome-1
    /// deopt the VM resumes AT the checkpoint (operands re-pushed from the
    /// journal slots) instead of re-running from ip 0, so side effects that
    /// already executed are never repeated.
    pub ckpt_slot: u32,
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
    /// This body was compiled against bindings a LATER module load changed (ADR-366): a
    /// global miss inside it — or the tier-up pre-load of what it names — actually loaded a
    /// module. Every compile-time decision that read the global table (the `resolve_prim*`
    /// inlines, the thin-wrapper elision, the leaf derivation) was made without that
    /// module's bindings, so the chunk is CORRECT and SLOW — `(math/rem i 3)` compiled to a
    /// generic call where the eager compile inlines `PrimOp::Rem`, 2.8x the instructions on
    /// `pipeline`. Set once; honoured by the cache lookups (`compiled_arm_for`,
    /// `probe_arm_for`), which evict the entry so the next activation recompiles with the
    /// bindings present, and by the tiering election, which never lowers a stale chunk.
    /// One recompile per (arm, first load) — the load is what earns it.
    pub stale_bindings: std::sync::atomic::AtomicBool,
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
    /// the background compiler, so most run interpreted.
    pub share_key: Option<(u64, u16)>,
    /// True once this process has published its native code to the shared cache (or
    /// installed the code *from* it) — so the publish costs one lock acquire per
    /// arm-instance, not one per call. Reset when the arm is epoch-invalidated so the
    /// recompiled code re-publishes.
    pub shared_published: std::sync::atomic::AtomicBool,
    /// The code pointer this arm has SETTLED on: the tier-up work [`jit_tier_in_frame`]
    /// does around an installed pointer — the inlined or re-lowered upgrade, the shared
    /// publish — is finished for it, so an entry that finds `jit_code` equal to this runs
    /// only the checks every entry needs and enters. Null until the slow path settles it.
    /// Keyed on the pointer rather than a flag, so every writer of `jit_code` voids it
    /// without knowing it exists; [`deopt::reset_native_state`], which can see the same
    /// pointer re-adopted from the shared cache, clears it explicitly.
    pub settled_code: std::sync::atomic::AtomicPtr<u8>,
    /// Captured enclosing-lexical names, in capture-slot order (#3 lexical addressing).
    /// Empty for a top-level / non-capturing arm. When non-empty, each name occupies a
    /// **capture slot** at `[capture_base + k]` where `capture_base = nrequired +
    /// noptional + (rest_slot.is_some())`; the body resolves the name to that
    /// `Node::Local(slot)` instead of an `env_get` symbol-scan, and [`push_frame`] fills
    /// the slot from the closure's captured env at call setup (an index fast-path for a
    /// flat capture frame — the VM-built common case — with an `env_get`-by-name fallback
    /// for a chained/tree-walker env, so it's correct in both engines).
    pub capture_names: Box<[Symbol]>,
    /// The closure's name (`Closure::name`), for error stack traces — `None` for
    /// an anonymous `fn` or a synthetic wrapper arm. Unlike the jit-only
    /// `dbg_name` (top-level `defn`s only — its symbol keys semantic tables like
    /// `i64_mark_too_deep`), this is purely diagnostic and set for *any* named
    /// closure.
    pub fn_name: Option<Symbol>,
    /// The source file this arm's body was read from (via `Heap::form_pos` at
    /// compile time), for error stack traces. `None` for REPL/eval'd code.
    pub src_file: Option<std::sync::Arc<str>>,
    /// The module whose code this arm compiles — the closure's authoring module (ADR-383),
    /// carried here so the error-position rule can tell generated code from a user's
    /// without touching the heap (KI-184: a contract shim's positions are never reported).
    pub module: Option<Symbol>,
    /// Recursive self-inlining (Phase B, the two-stage tiering upgrade).
    /// `Some(name)` when this arm qualifies as a top-level no-capture
    /// recursive `defn` whose body the JIT can splice depth-1 of into its own frame
    /// (removing the per-call protocol for the inlined level — the fib lever). The
    /// VM keeps the ORIGINAL small `body`/`chunk`/`nslots`; the inlined body is
    /// re-derived fresh in `jit_lower_arm` (`shift_slots` clone → `inline_self_calls`),
    /// so nothing here grows the interpreted frame. `None` = the arm doesn't qualify
    /// (the common case).
    #[cfg(feature = "jit")]
    pub inline_name: Option<Symbol>,
    /// Diagnostic only: this arm's defining-`defn` name (independent of `inline_name`,
    /// which is set only for inlinable recursive defns). Lets `jit_tier` label the arm in
    /// the staged-stale report so a use-after-GC names the arm holding the stale handle.
    /// NOT jit-gated, though everything around it is: the bytecode VM's `Inst::SelfCall`
    /// hot-reload guard (ADR-013, `exec_chunk`) re-resolves this name on a global-epoch
    /// bump to notice a `def` of the very function a loop is running. That guard is a
    /// correctness rule, not an optimisation, so it must hold in a `--no-default-features`
    /// runtime too — which is exactly the build that stopped compiling when the VM began
    /// reading a field only the jit had.
    pub dbg_name: Option<Symbol>,
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
    /// within an epoch; reset on epoch invalidation. See `frame_size_for_new_entry`.
    #[cfg(feature = "jit")]
    pub inline_installed: std::sync::atomic::AtomicBool,
    /// Lazily computed by `jit_tier`: does this arm's chunk contain a non-tail **named**
    /// call — i.e. would the §7.5 inline fast-frame blob arm at least one site if this
    /// same body were re-lowered hot? Gates the xcall re-lowering enqueue for arms with
    /// no inline derivation. Requires `dbg_name` too (the swap re-points this process's
    /// fast links by callee name). Unset until first asked.
    #[cfg(feature = "jit")]
    pub xcall_wanted: std::sync::OnceLock<bool>,
    /// The STATIC half of the i64/f64 register-worker eligibility (`arm_scalar_kind`):
    /// the arm's shape and body walk, which never change for an arm. `Ok(1)` Int, `Ok(2)`
    /// Float, `Err(reason)` refused. Cached because the eligibility is asked on every
    /// VM→native entry of an arm awaiting its inline upgrade (`declines_inline_upgrade`).
    #[cfg(feature = "jit")]
    pub scalar_kind: std::sync::OnceLock<Result<u8, &'static str>>,
    /// Does this arm's own chunk call `%receive` directly? Asked per tree-walker→VM route
    /// (`tw_vm_route`), so computed once (`arm_calls_receive`).
    pub calls_receive: std::sync::OnceLock<bool>,
    /// Leaf-callee inlining: the body with
    /// each qualifying small non-recursive callee's body spliced in, derived ONCE at
    /// arm-compile time (the only moment a `&Heap` can resolve the callee symbols).
    /// Rides the same two-stage deferred-upgrade channel as self-inlining
    /// (`inline_name`/`inline_nslots`/`inline_code`), mutually exclusive with it.
    /// The stored derivation is valid only at [`LeafInline::epoch`] — `jit_lower_inlined_arm`
    /// refuses to lower it at any other epoch (a `def`/compaction may have rebound a
    /// spliced callee), so hot-reload correctness falls out of the existing epoch guard.
    #[cfg(feature = "jit")]
    pub leaf: Option<Box<LeafInline>>,
}

/// An arm with every field at the value a freshly compiled arm starts from: no body, no
/// frame, no native code yet, every counter at zero and every sentinel at its sentinel
/// (`self_bound_stamp`, `ckpt_slot`). A construction site names only what it compiled
/// (`CompiledArm { nrequired, body, … ..CompiledArm::default() }`), so a new runtime field
/// is one edit here — not one per construction site, where a wrong start value would
/// compile fine. It mints a fresh `uid`, like every compiled arm.
impl Default for CompiledArm {
    fn default() -> Self {
        use std::sync::OnceLock;
        use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64};
        CompiledArm {
            nrequired: 0,
            noptional: 0,
            optional_flags: Box::new([]),
            rest_slot: None,
            nslots: 0,
            nsites: 0,
            ngsites: 0,
            uid: next_arm_uid(),
            site_pos: Box::new([]),
            staged_head_pos: Box::new([]),
            local_call_heads: Box::new([]),
            receive_at_head: false,
            body: Node::Const(ConstVal::new(Value::Nil)),
            chunk: None,
            has_runtime_handles: false,
            jit_code: AtomicPtr::new(std::ptr::null_mut()),
            jit_calls: AtomicU32::new(0),
            bail_reason: OnceLock::new(),
            compile_relocations: AtomicU64::new(0),
            native_resets: AtomicU64::new(0),
            deopt_watch: false,
            jit_deopts: AtomicU32::new(0),
            jit_deopts_total: AtomicU32::new(0),
            jit_poly_slots: AtomicU32::new(0),
            jit_entry_deopts: AtomicU32::new(0),
            jit_float_context: AtomicBool::new(false),
            jit_float_deopts: AtomicU32::new(0),
            float_globals: OnceLock::new(),
            self_global_ok: AtomicBool::new(false),
            // Sentinel: no binding validated yet.
            self_bound_stamp: AtomicU64::new(u64::MAX),
            // Sentinel: no deopt-checkpoint journal slot.
            ckpt_slot: u32::MAX,
            compile_epoch: AtomicU64::new(0),
            stale_bindings: AtomicBool::new(false),
            share_key: None,
            shared_published: AtomicBool::new(false),
            settled_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            capture_names: Box::new([]),
            fn_name: None,
            src_file: None,
            module: None,
            #[cfg(feature = "jit")]
            inline_name: None,
            dbg_name: None,
            #[cfg(feature = "jit")]
            inline_stride: 0,
            #[cfg(feature = "jit")]
            inline_nslots: 0,
            #[cfg(feature = "jit")]
            inline_code: AtomicPtr::new(std::ptr::null_mut()),
            #[cfg(feature = "jit")]
            inline_queued: AtomicBool::new(false),
            #[cfg(feature = "jit")]
            inline_installed: AtomicBool::new(false),
            #[cfg(feature = "jit")]
            xcall_wanted: OnceLock::new(),
            #[cfg(feature = "jit")]
            scalar_kind: OnceLock::new(),
            calls_receive: OnceLock::new(),
            #[cfg(feature = "jit")]
            leaf: None,
        }
    }
}

/// The stored leaf-inline derivation: see [`CompiledArm::leaf`].
#[cfg(feature = "jit")]
pub struct LeafInline {
    /// The **resume arm** — a full [`CompiledArm`] over the spliced body (the caller's
    /// body with each qualifying leaf callee's body inlined: args → shifted callee param
    /// slots via `LetBind`, callee body `shift_slots`-relocated above the caller's
    /// frame). It carries the spliced `body`, its compiled `chunk`, `nslots =
    /// inline_nslots`, and **its own `ckpt_slot`** in the spliced layout.
    ///
    /// It exists because the inlined native's bytecode ip space is its own: a deopt
    /// journals a resume ip into the *spliced* chunk, which the caller's small chunk
    /// cannot interpret. Resuming against this arm makes ip space and frame layout
    /// agree, which is what lets a partial splice keep a residual non-tail call (see
    /// [`super::leaf_inline_probe`]). It shares the caller's `uid`, so it indexes the
    /// same inline-cache block — sound because a spliced callee body contributes no
    /// call/global sites of its own (`leaf_body_qualifies` rejects both).
    ///
    /// Holding the chunk here also retains it: the inlined native bakes the raw
    /// addresses of this chunk's `ConstVal`s into itself, and `JIT_ARM_KEEPALIVE`
    /// retains the caller arm (hence this `Arc`) for the process lifetime.
    pub resume: Arc<CompiledArm>,
    /// The epoch the derivation is known valid at: `heap.global_epoch()` when derived, and
    /// moved forward by the tier-up when [`Self::still_valid`] finds nothing it depends on
    /// changed. Lowering requires `compile_epoch ==` this.
    pub epoch: std::sync::atomic::AtomicU64,
    /// The callees spliced in, as resolved at derivation.
    pub callees: Vec<LeafCallee>,
    /// `heap.code_relocations()` at derivation: the recorded closure handles mean what they
    /// meant then only while this is unchanged.
    pub relocations: u64,
}

/// One callee a leaf derivation spliced in: the call head and the closure it named.
#[cfg(feature = "jit")]
pub struct LeafCallee {
    pub sym: Symbol,
    /// The closure's handle bits (`ClosureId.0`), compared, never dereferenced.
    pub closure: u64,
}

#[cfg(feature = "jit")]
impl LeafInline {
    /// Is the derivation still what deriving it now would give? No code handle has been
    /// rewritten since, each head still names the closure that was spliced (a spliced
    /// callee captures nothing, so the same closure is the same body), and the spliced
    /// chunk's primitives are still native. A rebind of anything else, or a module load,
    /// leaves all three true — and the epoch moving for that reason no longer costs the
    /// arm its inlined body for good.
    pub(crate) fn still_valid(&self, heap: &crate::core::heap::Heap) -> bool {
        heap.code_relocations() == self.relocations
            && self.callees.iter().all(|c| {
                // A LOCAL handle is recycled by the minor collector, which counts no
                // relocation, so only a shared one can be trusted by identity.
                heap.env_get(heap.global(), c.sym).is_some_and(|v| {
                    matches!(v.unpack(), ValueRef::Fn(id)
                        if id.0 == c.closure && id.region() != crate::core::value::LOCAL)
                })
            })
            && self
                .resume
                .chunk
                .as_ref()
                .is_some_and(|chunk| super::chunk_ops_native(heap, chunk))
    }
}

/// The global-read slots an arm reserves for a body with `gsites` global reads and `sites`
/// call sites: the reads first, then one per call site. Native code resolves the head of a
/// tail call by name through the global-read cache ([`CompiledArm::tail_head_gsite`]); with
/// the call site's own number as the slot, that index was another read's slot or past the
/// arm's block, where the cache drops every write — each such call walked the environment
/// (`json`'s `string-acc` 100 000 times a run).
pub(crate) fn gsite_reservation(gsites: u32, sites: u32) -> u32 {
    gsites + sites
}

impl CompiledArm {
    /// The local the call at `pos` calls, when its head is a local (`local_call_heads`).
    pub(crate) fn local_call_head(&self, pos: crate::error::Pos) -> Option<Symbol> {
        self.local_call_heads
            .iter()
            .find(|(at, _)| *at == pos)
            .map(|&(_, head)| head)
    }

    /// The global-read slot native code resolves call site `call_site`'s head through when
    /// it makes that call in tail position — past every slot the body's own global reads
    /// use (see [`gsite_reservation`]).
    pub(crate) fn tail_head_gsite(&self, call_site: u32) -> u32 {
        debug_assert!(
            call_site < self.nsites,
            "call site {call_site} outside the arm's {} sites",
            self.nsites
        );
        self.ngsites - self.nsites + call_site
    }

    /// The arm's name for diagnostics: its `dbg_name`, or `<closure>` for an anonymous arm.
    pub(crate) fn dbg_label(&self) -> &'static str {
        self.dbg_name
            .map(crate::core::value::symbol_name_ref)
            .unwrap_or("<closure>")
    }
}

#[cfg(feature = "jit")]
impl CompiledArm {
    /// Latch the arm off the native tier for good (`BAILED`), keeping `reason` — the name
    /// its `[jit-bail]` trace line gives — so `%jit-arm-state` can say why an arm is not
    /// native without `BROOD_JIT_BAIL_TRACE` having been armed beforehand. The first reason
    /// is kept: a latched arm is not tried again.
    pub(crate) fn latch_bailed(&self, reason: &str) {
        let _ = self.bail_reason.set(reason.into());
        self.jit_code
            .store(crate::jit::BAILED, std::sync::atomic::Ordering::Release);
    }

    /// Put `code` in `jit_code` only if it still holds `expected` — what the writer read
    /// when it decided. Every writer but [`latch_bailed`](Self::latch_bailed) goes through
    /// here or [`clear_code_unless_latched`](Self::clear_code_unless_latched), because a
    /// latch is final and arms are shared: another process can latch this one between a
    /// writer's read and its store, and a plain store then put the arm back on the native
    /// code it was latched off (KI-309: a background compile finishing after the latch).
    /// `false`: the slot moved on, and what it holds now stands.
    pub(crate) fn replace_code(&self, expected: *mut u8, code: *mut u8) -> bool {
        use std::sync::atomic::Ordering::{Acquire, Release};
        self.jit_code
            .compare_exchange(expected, code, Release, Acquire)
            .is_ok()
    }

    /// Back to untried (null), unless the arm is latched: a reset is for native code being
    /// replaced, and a latched arm has none (see [`replace_code`](Self::replace_code)).
    /// `false` when it was latched.
    pub(crate) fn clear_code_unless_latched(&self) -> bool {
        use std::sync::atomic::Ordering::{Acquire, Release};
        let mut current = self.jit_code.load(Acquire);
        loop {
            if current == crate::jit::BAILED {
                return false;
            }
            match self.jit_code.compare_exchange_weak(
                current,
                std::ptr::null_mut(),
                Release,
                Acquire,
            ) {
                Ok(_) => return true,
                Err(now) => current = now,
            }
        }
    }

    /// The arm's bytecode as a space-separated opcode list, for the `[jit-bail]` traces.
    pub(crate) fn dbg_opcodes(&self) -> String {
        let code = self
            .chunk
            .as_ref()
            .map(|c| c.code.as_slice())
            .unwrap_or(&[]);
        code.iter()
            .map(super::jit_plan::codegen::inst_opcode_name)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The frame size the **currently installed** native version runs against —
    /// the per-engine frame-sizing key for two-stage tiering. Small native → `nslots`;
    /// inlined native (post-swap) → `inline_nslots`.
    ///
    /// # This read is RACY. Call it only to BUILD a frame, and capture the result.
    ///
    /// It loads `inline_installed`, which the background inline upgrade flips at any
    /// moment. Calling it a second time — to interpret a frame that already exists — can
    /// therefore return a different size than the frame was built to, and the staged
    /// `[callee, args…]` region is then written at one offset and read at another.
    ///
    /// That is **KI-48** (also KI-26 and the ADR-210 bugs — the same anti-pattern).
    ///
    /// The rule: whoever builds the frame captures this ONCE and **tells** every consumer
    /// (`jit_dispatch_tail` takes `frame_nslots`; the deopt-resume helpers are passed it).
    /// A consumer that re-derives it is a bug even when the sizes happen to agree today.
    /// `crates/lisp/tests/frame_size_callsites.rs` pins the permitted callers so a new one
    /// has to be justified rather than merely compile. The name keeps the hazard visible at
    /// the call site.
    #[inline]
    pub fn frame_size_for_new_entry(&self) -> usize {
        if self
            .inline_installed
            .load(std::sync::atomic::Ordering::Acquire)
        {
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
    fn shape(&self) -> crate::core::value::ArmShape {
        crate::core::value::ArmShape {
            required: self.nrequired,
            optional: self.noptional,
            rest: self.has_rest,
        }
    }
}

/// A compiled closure: every arm's arity shape + (if VM-eligible) its compiled body.
pub struct CompiledClosure {
    pub(super) arms: Vec<ArmSpec>,
}

impl CompiledClosure {
    /// **Test-only.** Every compiled arm of this closure, for invariant tests that need
    /// a corpus of real chunks rather than the one arm a given argc selects.
    #[cfg(test)]
    pub(crate) fn dbg_arms(&self) -> impl Iterator<Item = &Arc<CompiledArm>> {
        self.arms.iter().filter_map(|a| a.compiled.as_ref())
    }

    /// Was any arm of this closure compiled against bindings a later module load changed
    /// (ADR-366)? A cache entry answering true is evicted rather than served.
    pub(crate) fn stale_bindings(&self) -> bool {
        self.arms.iter().any(|a| {
            a.compiled.as_ref().is_some_and(|arm| {
                arm.stale_bindings
                    .load(std::sync::atomic::Ordering::Acquire)
            })
        })
    }

    /// The compiled arm to run for `argc`, or `None` to defer to the tree-walker. The arm
    /// is chosen by the tree-walker's own rule ([`crate::core::value::select_arm_by`]);
    /// its compiled body is returned iff it was VM-eligible — otherwise `None`, so the
    /// tree-walker runs the *same* arm.
    pub(crate) fn arm_for(&self, argc: usize) -> Option<&Arc<CompiledArm>> {
        crate::core::value::select_arm_by(&self.arms, argc, ArmSpec::shape)?
            .compiled
            .as_ref()
    }
}

/// A **process-local handle** to a (possibly runtime-shared) [`CompiledArm`] — the
/// thing the VM's call path clones, so that the clone is never contended (KI-40).
///
/// ADR-175 Phase B publishes a compiled arm to `Runtime::shared_closures`, so every
/// green process running that closure holds an `Arc` into **one** allocation. The VM's
/// non-tail call path then clones that `Arc` up to three times per call — the IC probe
/// ([`Heap::vm_call_ic_probe`]), the `BcFrame` the driver pushes, and (for a
/// handle-bearing arm) `live_arm_push` — so N worker threads RMW one refcount cache
/// line per call — a contention cost that can triple a parallel workload's runtime.
///
/// The fix is to interpose an `Arc` that only **one** process ever clones. `ArmHandle`
/// is created once per (process, call site) when an inline cache is filled and owns the
/// shared `Arc`; the per-call traffic then lands on the handle's own refcount, which is
/// core-local, while the arm itself — the ~18 KB the sharing exists to deduplicate —
/// stays shared. Liveness is strictly stronger than cloning the inner `Arc` directly
/// (the handle keeps it alive for at least as long), so this needs no immortality
/// argument and changes no GC invariant: `runtime_collect` still reaches the same arms
/// through [`Self::arc`].
pub struct ArmHandle {
    inner: Arc<CompiledArm>,
}

impl ArmHandle {
    #[inline]
    pub fn new(inner: Arc<CompiledArm>) -> Arc<Self> {
        Arc::new(Self { inner })
    }

    /// The shared `Arc`, for the paths that genuinely need to own or publish it (the
    /// JIT's work queue and keepalive, `runtime_collect`'s handle rewrite).
    #[inline]
    pub fn arc(&self) -> &Arc<CompiledArm> {
        &self.inner
    }
}

impl std::ops::Deref for ArmHandle {
    type Target = CompiledArm;
    #[inline]
    fn deref(&self) -> &CompiledArm {
        &self.inner
    }
}

/// The result of `dispatch`: a finished value, or a *tail call* the caller continues.
/// `Tail` carries a resolved VM arm un-run, so a tail call reuses a frame (in
/// [`vm_run_bc`]) or is forced (in value position, via [`force`]). The bytecode loop's own
/// exits are [`ChunkExit`].
pub(crate) enum Step {
    Done(Value),
    Tail {
        compiled: Arc<ArmHandle>,
        args: SmallVec<[Value; 4]>,
        /// The tail callee's own captured env — switched to so the next arm resolves
        /// its free vars in *its* scope (Stage 2c: a tail call can cross into a
        /// closure with a different captured env).
        genv: EnvId,
        /// The callee arm's IC block in this process ([`Heap::vm_arm_block`],
        /// ADR-175 Phase A) — the driver installs it as the current bases when it
        /// enters the arm.
        bases: (u32, u32),
    },
}

/// What running a bytecode [`Chunk`] frame yields back to the explicit-frame driver
/// ([`vm_run_bc`], ADR-100 Stage 4). Unlike [`Step`] (which the `Node` trampoline
/// uses), this adds `Call` — a **non-tail** call to a chunked VM arm, which the
/// driver turns into a **frame push** rather than native recursion. `Tail`/`SelfTail`
/// reuse the current frame (TCO); `Done` pops it. A non-tail call to a native or a
/// tree-walked arm is already executed inside `exec_chunk` (via `dispatch`) and
/// surfaces as an ordinary pushed value, never as `Call`.
pub(crate) enum ChunkExit {
    Done(Value),
    Tail {
        arm: Arc<ArmHandle>,
        args: SmallVec<[Value; 4]>,
        genv: EnvId,
        /// Callee IC block (see [`Step::Tail::bases`]).
        bases: (u32, u32),
    },
    Call {
        arm: Arc<ArmHandle>,
        args: SmallVec<[Value; 4]>,
        genv: EnvId,
        /// Callee IC block (see [`Step::Tail::bases`]).
        bases: (u32, u32),
    },
    /// A non-tail call to a VM arm whose frame is its arguments (exactly `argc` required
    /// params: no optionals, no rest) — the `argc` args ALREADY sit at the top of `roots`,
    /// so the driver adopts them as the callee's first slots instead of `Call`'s round trip
    /// (copy them into `args`, carry them out by value, `push_frame` them back to where they
    /// were). The frame base is `roots_len() - argc`; the result lands there on `Done`,
    /// exactly where `Call`'s would (`drop_base`).
    CallInPlace {
        arm: Arc<ArmHandle>,
        argc: usize,
        genv: EnvId,
        /// Callee IC block (see [`Step::Tail::bases`]).
        bases: (u32, u32),
    },
    /// A native callee ran **in place** from `exec_chunk`'s non-tail `Inst::Call` (the
    /// VM→native direct call, `docs/compute-frontier.md` §7.12) through
    /// `jit_tier_in_frame`, and ended with an outcome only the driver can honour: a deopt
    /// (`Some(1)`) or preempt (`Some(2)`) — continue THIS activation on the VM, at its
    /// checkpoint or ip 0 — or `None` (declined, or an i64 depth bail, both already booked
    /// by `jit_tier_in_frame`) — run the activation on the VM from ip 0 without tiering it
    /// again. The callee's frame is still laid out at `roots[base..base+nslots]` (params
    /// plus whatever the native left), the caller's operands below `base`. The driver
    /// adopts it as the current frame — pushing the caller exactly as `Call` does, minus
    /// `push_frame` — and settles it with the same routine that settles a frame the
    /// frame path's `jit_tier_in_frame` hands back with the same outcome. A value, an
    /// error and a staged tail call never reach here: `exec_chunk` finishes those itself.
    #[cfg(feature = "jit")]
    CallResume {
        arm: Arc<ArmHandle>,
        base: usize,
        /// The size the frame was BUILT to (`frame_size_for_new_entry()` at entry) — the
        /// deopt-resume helpers must be told it, never re-derive it (KI-48 family).
        nslots: usize,
        genv: EnvId,
        bases: (u32, u32),
        outcome: Option<i64>,
    },
    /// A native preempt unwound out to the driver (ADR-401): the activations it passed
    /// through are recorded in the thread's unwind record (`take_unwound`), their frames still in `roots`, for the
    /// driver to rebuild as interpreter frames and park. `keep_current`: the current frame
    /// is an interpreter frame that called the outermost of them directly (the VM→native
    /// call) and waits for its value at the ip after that call; otherwise the current frame
    /// IS the outermost of them (the frame path) and is rebuilt with the rest.
    #[cfg(feature = "jit")]
    Unwound {
        keep_current: bool,
    },
    /// A clean `receive` on an empty mailbox raised `Control::Suspend` through the
    /// `%receive` native (state-capture path, ADR-100 §8). `exec_chunk` rewound `ip`
    /// so re-entry re-runs the suspending `Inst::Call`, leaving the callee + args on
    /// the operand stack untouched; the driver ([`vm_run_bc`]) captures the whole
    /// frame stack as a [`Suspended`] and returns it to the scheduler to park. Produced
    /// only by a clean top-level `receive` (a native-nested one blocks the worker, §7.4).
    Suspend {
        deadline: Option<crate::process::SchedDeadline>,
    },
    /// Hard `:kill` was pending at the inline `SelfCall` safepoint. The frame is already
    /// reset (ip=0, new args in slots); the driver retires the process.
    Killed,
    /// A `(%contract-tail? ret)` ([`PrimOp1::ContractTail`]) asked about the frame stack,
    /// which only the driver holds. `ret` is the operand on top of `roots`, and `ip` is
    /// already past the instruction: the driver replaces `ret` with the answer and
    /// re-enters this same frame.
    ContractProbe,
    /// `(proc/interruptible ceiling thunk)` with a VM thunk (ADR-491): the driver opens an
    /// interrupt REGION ([`crate::core::heap::HandlerKind::Region`]) in this frame and
    /// pushes the thunk's frame — `Call`'s frame push, no arguments — so the thunk runs on
    /// the driver's own frame stack and a `receive` in it parks instead of blocking the
    /// worker. The call's operands are already dropped (at `drop_base`, where the answer
    /// lands); `tail`: the call was in tail position, so the answer returns from this frame.
    Interruptible {
        ceiling: Option<u64>,
        arm: Arc<ArmHandle>,
        genv: EnvId,
        bases: (u32, u32),
        tail: bool,
    },
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
            module: _,
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
        Node::Prim3 { a, b, c, .. } => {
            rewrite_node(a, f);
            rewrite_node(b, f);
            rewrite_node(c, f);
        }
        Node::Prim1 { a, .. } => rewrite_node(a, f),
        Node::TryCatch { body, handler, .. } => {
            rewrite_node(body, f);
            rewrite_node(handler, f);
        }
        Node::Binding { vals, body, .. } => {
            rewrite_node(vals, f);
            rewrite_node(body, f);
        }
    }
}

/// Rewrite every movable handle embedded in a live compiled arm — its body (which holds
/// any real `&optional` default too), and the bytecode `chunk` if present (its `Const`s
/// and `MakeClosure` `fn_rest` are separate handle copies that must move too). Called
/// by `runtime_collect` per registered live arm.
pub fn rewrite_arm_handles(arm: &CompiledArm, f: &mut dyn FnMut(Value) -> Value) {
    rewrite_node(&arm.body, f);
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
// A compiled arm's `Node` body is also lowered to a flat **bytecode** `Chunk` — a linear
// instruction stream over the **same** operand stack (`Heap::roots`) the `Node`
// interpreter uses, run by a single non-recursive loop (`exec_chunk`). Every compiled
// body lowers; the bytecode is the VM's only executor.
//
// Why this shape: a process's continuation must be relocatable heap data rather than a
// native Rust stack. The operand state lives on `Heap::roots`; the chunk reifies the
// *control* state (the instruction pointer), and `Call`/`Return` are explicit frame
// push/pop, so the cross-arm call stack is data too (ADR-100 §8.4). The driver stays
// bit-identical to the `Node` interpreter, guarded by the differential test.

/// One bytecode instruction. A **stack machine**: each instruction pushes/pops on
/// the operand region of `Heap::roots` that sits just above the activation frame's
/// slots (`base..base+nslots`). Frame slots are read/written by absolute index
/// (`Local`/`SetLocal`); everything else is push/pop. The semantics of each arm
/// mirror the matching [`Node`] case, and the tree-walker's form, exactly.
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
    /// Record that this source line executed, for `nest test --cover-lines`
    /// (ADR-148 tier 2). Emitted ONLY when coverage is armed at compile time
    /// (`coverage::enabled()`), so an ordinary chunk is byte-for-byte what it always
    /// was and the interpreter never sees this opcode. Carries the line alone; the
    /// FILE comes from the executing arm's `src_file`, which `exec_chunk` already has.
    RecordLine(u32),
    /// Record that one edge of a branch was taken, for `nest test --cover-branches`
    /// (ADR-148 tier 2). Emitted only when coverage is armed, at each `if`'s then/else
    /// edge — `(line, col, taken)`, keyed by the *test's* source position so nested `if`s
    /// from one `cond`/`match` on shared lines stay distinct. A branch is fully covered
    /// only when BOTH edges record. FILE comes from the executing arm's `src_file`.
    RecordBranch(u32, u32, bool),
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
    /// Inlined 3-ary primitive (`table-put`): replace the top three operands with
    /// the result, or fall back to a general call on `head`. Mirrors `Node::Prim3`.
    Prim3 {
        op: PrimOp3,
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
    /// passthrough probe + `compiled_arm_for`.
    ///
    /// **Eval order** (KI-19, ADR-191): eliding the head moves its resolution *after* the
    /// arguments, which diverges from the tree-walker when an argument rebinds it. The
    /// compiler therefore sets [`staged`](Self::Call::staged) for a rebindable head whose
    /// args can run user code, and the callee is taken off the stack instead. `head`/`site`
    /// stay populated either way, so the IC keeps caching the arm; a `def` bumping the epoch
    /// invalidates the entry as before.
    Call {
        argc: usize,
        tail: bool,
        pos: Option<Pos>,
        site: u32,
        head: Option<Symbol>,
        /// Is the free-global callee **staged on the operand stack** (below the args)
        /// rather than resolved at the call?
        ///
        /// KI-19: the tree-walker evaluates the operator first, but eliding the head moves
        /// its resolution *after* the arguments, so an argument that rebinds it made the
        /// engines disagree — `(f (bump))` gave `:new` on the VM and `:old` everywhere
        /// else. The compiler now stages the head for exactly those calls whose arguments
        /// can run user code (the only way a `def` can happen mid-call).
        ///
        /// `head`/`site` stay populated when this is set, which is the whole point: the
        /// call-site IC still caches the resolved **arm** — the thing the elided head was
        /// really buying — and the staged value is validated against the IC's cached
        /// callee. Don't drop to the plain computed-callee path instead: it loses the IC
        /// (~6x on `json`).
        staged: bool,
    },
    /// Direct `letrec` self-tail-call (always tail position): args have been pushed;
    /// returns a `Step::SelfTail` so the trampoline re-enters this arm in place.
    /// Mirrors `Node::SelfCall`.
    SelfCall { argc: usize },
    /// Build a closure (`(fn …)` evaluated inside a compiled body). The `names`'
    /// capture values have been pushed (in order) by preceding leaf instructions;
    /// this binds them into a fresh captured env, builds the closure from `fn_rest`,
    /// and (for a direct `letrec` self-ref) late-binds `self_name` to it. As
    /// `Node::MakeClosure` describes. `fn_rest` is a movable
    /// RUNTIME handle — rewritten in place by [`rewrite_chunk`].
    MakeClosure {
        fn_rest: ConstVal,
        names: Box<[Symbol]>,
        self_name: Option<Symbol>,
        /// The built closure's contract boundary — see `Node::MakeClosure::module`.
        module: Option<Symbol>,
    },
    /// Open an inline `try` (ADR-400): register a handler for THIS frame on the process's
    /// handler stack (`Heap::vm_handlers`) — the operand-stack height here, the frame's
    /// slot `bind_slot` the caught value lands in, and `handler`, the ip of the handler
    /// code. The body follows inline as ordinary bytecode, so it runs in the bytecode
    /// driver's own loop: it can be preempted, park at a `receive`, and call natively —
    /// none of which a body run as a nested evaluation could do. A non-control error
    /// raised anywhere above this frame unwinds the driver to it, stores the caught value
    /// and resumes at `handler` (`vm_run_bc`'s catch).
    TryEnter { handler: usize, bind_slot: usize },
    /// Close the innermost `try`: the body finished normally, its value is on the
    /// operand stack, and the handler registered by the matching [`Inst::TryEnter`]
    /// retires. Always followed by the `Jump` over the handler code.
    TryExit,
    /// Open an inline `binding` (ADR-402): pop the vector of new values and push each name
    /// with its value on the process's dynamic stack (every name must be dynamic and the
    /// counts must match, checked before anything is pushed). The body follows inline, so
    /// it runs in the driver's loop like a `try` body. An error leaving the body is
    /// restored by truncation — to the length a catching `try` recorded, or to the driver's
    /// entry length when the error leaves the driver.
    BindEnter { names: Box<[Symbol]> },
    /// Close the innermost inline `binding`: pop its names, the body's value on the stack.
    BindExit(usize),
}

impl Inst {
    /// The source position this instruction was compiled from, when recorded —
    /// used by the error-trace builder: a caller frame's `code[ip - 1]` is the
    /// `Inst::Call` that pushed the callee, so its `pos` is the call site.
    pub(crate) fn call_pos(&self) -> Option<Pos> {
        match self {
            Inst::Prim1 { pos, .. }
            | Inst::Prim2 { pos, .. }
            | Inst::Prim3 { pos, .. }
            | Inst::Prim2SlotSlot { pos, .. }
            | Inst::Prim2SlotInt { pos, .. }
            | Inst::Call { pos, .. } => *pos,
            _ => None,
        }
    }

    /// Compact name for `BROOD_VM_TRACE` output — variant + key operands,
    /// no AtomicU64 (not Debug-able). Also the spelling `%vm-arm-ops` (dev-tools) reports,
    /// so a release dev binary carries it too.
    #[cfg(any(debug_assertions, feature = "dev-tools"))]
    pub(crate) fn trace_name(&self) -> String {
        match self {
            Inst::RecordLine(line) => format!("RecordLine({line})"),
            Inst::RecordBranch(line, col, taken) => format!("RecordBranch({line}:{col} {taken})"),
            Inst::Const(cv) => format!(
                "Const({})",
                match cv.load().unpack() {
                    crate::core::value::ValueRef::Int(n) => format!("int:{n}"),
                    crate::core::value::ValueRef::Bool(b) => format!("bool:{b}"),
                    crate::core::value::ValueRef::Nil => "nil".into(),
                    crate::core::value::ValueRef::Float(f) => format!("float:{f}"),
                    _ => "handle".into(),
                }
            ),
            Inst::Local(i) => format!("Local({i})"),
            Inst::Global(s) => format!("Global({})", crate::core::value::symbol_name_ref(*s)),
            Inst::GlobalIc { sym, site } => format!(
                "GlobalIc({}, site={site})",
                crate::core::value::symbol_name_ref(*sym)
            ),
            Inst::Pop => "Pop".into(),
            Inst::SetLocal(i) => format!("SetLocal({i})"),
            Inst::Jump(t) => format!("Jump({t})"),
            Inst::JumpIfFalse(t) => format!("JumpIfFalse({t})"),
            Inst::MakeVector(n) => format!("MakeVector({n})"),
            Inst::MakeMap(n) => format!("MakeMap({n})"),
            Inst::Prim1 { op, head, .. } => format!(
                "Prim1({op:?}, {})",
                crate::core::value::symbol_name_ref(*head)
            ),
            Inst::Prim2 { op, head, .. } => format!(
                "Prim2({op:?}, {})",
                crate::core::value::symbol_name_ref(*head)
            ),
            Inst::Prim3 { op, head, .. } => format!(
                "Prim3({op:?}, {})",
                crate::core::value::symbol_name_ref(*head)
            ),
            Inst::Prim2SlotSlot {
                op,
                slot_a,
                slot_b,
                head,
                ..
            } => format!(
                "Prim2SlotSlot({op:?}, s{slot_a},s{slot_b}, {})",
                crate::core::value::symbol_name_ref(*head)
            ),
            Inst::Prim2SlotInt {
                op,
                slot_a,
                int_b,
                head,
                ..
            } => format!(
                "Prim2SlotInt({op:?}, s{slot_a},{int_b}, {})",
                crate::core::value::symbol_name_ref(*head)
            ),
            Inst::Call {
                argc, tail, head, ..
            } => format!(
                "Call(argc={argc}, tail={tail}, head={})",
                head.map(crate::core::value::symbol_name_ref)
                    .unwrap_or("computed")
            ),
            Inst::SelfCall { argc } => format!("SelfCall({argc})"),
            Inst::MakeClosure { names, .. } => format!("MakeClosure(captures={})", names.len()),
            Inst::TryEnter { handler, bind_slot } => {
                format!("TryEnter(handler={handler}, bind={bind_slot})")
            }
            Inst::TryExit => "TryExit".into(),
            Inst::BindEnter { names } => format!("BindEnter(names={})", names.len()),
            Inst::BindExit(n) => format!("BindExit({n})"),
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
