//! The per-process data heap, plus the two shared regions: the immutable
//! **prelude** and a runtime's mutable, shared **code** region.
//!
//! A `Value`'s heap variants are integer handles whose two high bits (the
//! *region*, see `value.rs`) say where they live:
//!
//! - **LOCAL** — the per-process [`Heap`]: everything a process allocates at
//!   runtime (cons cells, vectors, strings, call-frame env scopes). Plain
//!   `Vec`s, mutated through `&mut Heap`, so the whole `Heap` is `Send`.
//! - **PRELUDE** — a [`SharedCode`] region (behind `Arc`) holding the prelude +
//!   builtins. Built once, frozen, shared read-only by every runtime.
//! - **RUNTIME** — a [`RuntimeCode`] region (behind `Arc`) holding a runtime's
//!   `def`'d code and its global bindings. **Mutable and shared** by all of a
//!   runtime's inner (spawned) processes, so a redefinition is visible to a
//!   running process on its next global lookup (Erlang-style hot reload). The
//!   code slabs are append-only (old code is never moved or freed, so in-flight
//!   calls keep running it); the global bindings are a `RwLock<HashMap>`.
//!
//! No GC yet (the arenas only grow).

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::error::LispError;
use crate::core::value::{
    Closure, ClosureId, EnvId, NativeFn, NativeId, PairId, StrId, Symbol, Value, VecId, LOCAL,
    PRELUDE, RUNTIME,
};

/// Generate a `&self` accessor that resolves a handle to a shared reference by
/// region: the LOCAL/PRELUDE slab is indexed directly; the append-only RUNTIME
/// slab via `boxcar::Vec::get` (stable refs, lock-free). The three uniform
/// all-three-region reference accessors share this; `pair` (returns by value)
/// and the region-restricted `native`/`env_frame` stay hand-written.
macro_rules! region_ref {
    ($name:ident, $id:ty, $field:ident, $ret:ty, $what:literal) => {
        pub fn $name(&self, id: $id) -> $ret {
            match id.region() {
                LOCAL => &self.local.$field[id.index()],
                PRELUDE => &self.prelude.slabs.$field[id.index()],
                RUNTIME => self.runtime.code.$field.get(id.index()).expect($what),
                _ => unreachable!("invalid handle region"),
            }
        }
    };
}

struct EnvFrame {
    vars: HashMap<Symbol, Value>,
    parent: Option<EnvId>,
}

/// Re-tag a value's handle from the local region to the immutable **prelude**
/// region (same slab index, region bits set). Atoms are unchanged.
fn to_prelude(v: Value) -> Value {
    match v {
        Value::Pair(id) => Value::Pair(PairId::prelude(id.index())),
        Value::Vector(id) => Value::Vector(VecId::prelude(id.index())),
        Value::Str(id) => Value::Str(StrId::prelude(id.index())),
        Value::Fn(id) => Value::Fn(ClosureId::prelude(id.index())),
        Value::Macro(id) => Value::Macro(ClosureId::prelude(id.index())),
        Value::Native(id) => Value::Native(NativeId::prelude(id.index())),
        other => other,
    }
}

/// The slabs holding heap objects in the LOCAL data heap and the PRELUDE region.
#[derive(Default)]
struct Slabs {
    pairs: Vec<(Value, Value)>,
    vectors: Vec<Vec<Value>>,
    strings: Vec<String>,
    closures: Vec<Closure>,
    natives: Vec<NativeFn>,
    envs: Vec<EnvFrame>,
}

/// The immutable, read-only prelude region (closures, code values, the
/// builtins). Built once, then shared by `Arc` into every runtime.
#[derive(Default)]
pub struct SharedCode {
    slabs: Slabs,
}

/// A snapshot of the LOCAL heap's sizes, taken at a top-level boundary. Passing
/// it back to [`Heap::reset_local_to`] reclaims everything allocated since (see
/// there for the safety contract). This is the arena-reset reclamation strategy
/// (`docs/memory-model.md`): at a quiescent point the LOCAL heap holds nothing
/// live but the form's result, because globals live in PRELUDE/RUNTIME and never
/// point into LOCAL.
#[derive(Clone, Copy)]
pub struct LocalCheckpoint {
    pairs: usize,
    vectors: usize,
    strings: usize,
    closures: usize,
    envs: usize,
    // No `natives` field: a live runtime never allocates a native into its LOCAL
    // heap (they're registered once during the prelude build, then frozen into
    // PRELUDE). If that ever changes, add a field here and truncate it below.
}

/// Append-only code slabs for the shared RUNTIME region. `boxcar::Vec` gives
/// lock-free reads that return stable references (existing elements never move
/// or free as the vector grows), so process threads read closure bodies without
/// locking while another process `def`s new code.
#[derive(Default)]
struct CodeSlabs {
    pairs: boxcar::Vec<(Value, Value)>,
    vectors: boxcar::Vec<Vec<Value>>,
    strings: boxcar::Vec<String>,
    closures: boxcar::Vec<Closure>,
    /// Captured environments of promoted closures. A closure defined *inside a
    /// function call* (not at top level) closes over a local scope; promoting it
    /// for sharing copies that scope here so it resolves in any process. Frozen
    /// once promoted (read-only), so append-only is sound.
    envs: boxcar::Vec<EnvFrame>,
}

/// A runtime's mutable, shared code region: the code `def`'d at runtime plus the
/// global bindings table. All of a runtime's inner processes share one of these
/// (via `Arc::clone`), which is what makes a `def` propagate to them — and what
/// keeps separate runtimes (nodes) independent (each has its own).
pub struct RuntimeCode {
    code: CodeSlabs,
    /// The global bindings (prelude + user `def`s). Read on every global lookup,
    /// written on `def`/`set!`. The values point into PRELUDE or RUNTIME.
    globals: RwLock<HashMap<Symbol, Value>>,
}

impl Default for RuntimeCode {
    fn default() -> Self {
        RuntimeCode {
            code: CodeSlabs::default(),
            globals: RwLock::new(HashMap::new()),
        }
    }
}

impl RuntimeCode {
    /// A fresh runtime whose global table is seeded with the prelude bindings
    /// (`symbol -> prelude value`). The code slabs start empty — user `def`s
    /// append to them. Inner processes share this whole thing via `Arc`.
    pub fn seeded(bindings: &[(Symbol, Value)]) -> Self {
        let mut globals = HashMap::with_capacity(bindings.len());
        for &(s, v) in bindings {
            globals.insert(s, v);
        }
        RuntimeCode {
            code: CodeSlabs::default(),
            globals: RwLock::new(globals),
        }
    }

    /// Read/write the global table, recovering from a poisoned lock instead of
    /// propagating the panic. The values are `Copy` handles and writers only
    /// `insert`/replace, so a writer that panicked left the map structurally
    /// sound — recovering keeps one bad process from wedging every other one
    /// that later looks up or defines a global.
    fn globals_read(&self) -> RwLockReadGuard<'_, HashMap<Symbol, Value>> {
        self.globals.read().unwrap_or_else(|e| e.into_inner())
    }
    fn globals_write(&self) -> RwLockWriteGuard<'_, HashMap<Symbol, Value>> {
        self.globals.write().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Heap {
    local: Slabs,
    prelude: Arc<SharedCode>,
    runtime: Arc<RuntimeCode>,
    /// This process's global scope. For a real runtime this is [`EnvId::GLOBAL`]
    /// (routing to `runtime.globals`); for the prelude *builder* it's a real
    /// local root frame (so the prelude can be evaluated, then frozen).
    global: EnvId,
    /// Source position of LOCAL list forms, keyed by pair slab index, recorded
    /// by the reader. Queried via `(form-pos …)` (e.g. by the test macros, which
    /// look up a form's line *before* it expands). LOCAL-only and dropped on
    /// reset, since it is read-time metadata for the source being loaded.
    form_pos: HashMap<usize, crate::error::Pos>,
    /// The file currently being `load`ed, exposed via `(current-file)`. Saved and
    /// restored around each load so nested loads don't clobber the outer file.
    current_file: Option<String>,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    /// A bare heap with empty shared regions — used to *build* the prelude
    /// before freezing it. Real runtimes use [`Heap::with_regions`].
    pub fn new() -> Self {
        Heap {
            local: Slabs::default(),
            prelude: Arc::default(),
            runtime: Arc::default(),
            global: EnvId::local(0),
            form_pos: HashMap::new(),
            current_file: None,
        }
    }

    /// A fresh process heap sharing the given prelude + runtime regions (empty
    /// local slabs). Spawned inner processes pass the *same* `runtime` Arc as
    /// their parent, so they see its global bindings and its later `def`s.
    pub fn with_regions(prelude: Arc<SharedCode>, runtime: Arc<RuntimeCode>) -> Self {
        Heap {
            local: Slabs::default(),
            prelude,
            runtime,
            global: EnvId::local(0),
            form_pos: HashMap::new(),
            current_file: None,
        }
    }

    /// Clone the Arc to this heap's prelude region (for spawning a child).
    pub fn prelude_arc(&self) -> Arc<SharedCode> {
        Arc::clone(&self.prelude)
    }

    /// Clone the Arc to this runtime's shared code region (for spawning a child
    /// that shares this runtime's live globals).
    pub fn runtime_arc(&self) -> Arc<RuntimeCode> {
        Arc::clone(&self.runtime)
    }

    /// Consume this (builder) heap: move everything it allocated into a frozen
    /// [`SharedCode`] (PRELUDE) region — re-tagging every handle local→prelude —
    /// and return that region plus the global env's bindings
    /// (`symbol -> prelude value`) used to seed each runtime's global table.
    ///
    /// Env frames are dropped: shared (top-level) closures capture the global
    /// env symbolically (`env == None`), so nothing references a frame.
    pub fn freeze_as_shared_code(self, root: EnvId) -> (SharedCode, Vec<(Symbol, Value)>) {
        let bindings: Vec<(Symbol, Value)> = self.local.envs[root.index()]
            .vars
            .iter()
            .map(|(&s, &v)| (s, to_prelude(v)))
            .collect();

        let mut slabs = self.local;
        for p in &mut slabs.pairs {
            p.0 = to_prelude(p.0);
            p.1 = to_prelude(p.1);
        }
        for vec in &mut slabs.vectors {
            for x in vec.iter_mut() {
                *x = to_prelude(*x);
            }
        }
        for c in &mut slabs.closures {
            for f in c.body.iter_mut() {
                *f = to_prelude(*f);
            }
            for (_, d) in c.optionals.iter_mut() {
                *d = to_prelude(*d);
            }
            debug_assert!(
                c.env.is_none(),
                "shared closures must capture the global env"
            );
        }
        slabs.envs = Vec::new(); // the prelude region has no env frames

        (SharedCode { slabs }, bindings)
    }

    /// Record this process's global scope (call once, after creating it).
    pub fn set_global(&mut self, env: EnvId) {
        self.global = env;
    }

    /// This process's global scope.
    pub fn global(&self) -> EnvId {
        self.global
    }

    /// True if `env` is this process's global scope.
    pub fn is_global(&self, env: EnvId) -> bool {
        env == self.global
    }

    /// Snapshot the LOCAL heap's current sizes (for arena-reset reclamation).
    pub fn checkpoint(&self) -> LocalCheckpoint {
        LocalCheckpoint {
            pairs: self.local.pairs.len(),
            vectors: self.local.vectors.len(),
            strings: self.local.strings.len(),
            closures: self.local.closures.len(),
            envs: self.local.envs.len(),
        }
    }

    /// Reclaim everything allocated into the LOCAL heap since `cp`, by truncating
    /// the slabs back to it.
    ///
    /// **Safety contract (logical, not `unsafe`):** call this only at a top-level
    /// boundary — when the evaluator has fully returned and no value reachable
    /// from here on holds a LOCAL handle at or past `cp`. Globals live in the
    /// PRELUDE/RUNTIME regions and never point into LOCAL (a top-level `def`
    /// *promotes* its value out), so they're always safe; the only thing that can
    /// still be live is the *result* of the form just evaluated — consume or
    /// promote it before resetting. Resetting mid-evaluation would strand the
    /// in-flight computation's values and corrupt later reads.
    pub fn reset_local_to(&mut self, cp: LocalCheckpoint) {
        self.local.pairs.truncate(cp.pairs);
        self.local.vectors.truncate(cp.vectors);
        self.local.strings.truncate(cp.strings);
        self.local.closures.truncate(cp.closures);
        self.local.envs.truncate(cp.envs);
        // Drop position metadata for the pairs just reclaimed (indices reused).
        if !self.form_pos.is_empty() {
            self.form_pos.retain(|&i, _| i < cp.pairs);
        }
    }

    // ----- source-position metadata (editor tooling; see docs/tooling.md) -----

    /// Record the source position of a LOCAL list form (no-op for atoms and
    /// forms in the shared regions). Called by the reader as it builds lists.
    pub fn set_form_pos(&mut self, v: Value, pos: crate::error::Pos) {
        if let Value::Pair(id) = v {
            if id.region() == crate::core::value::LOCAL {
                self.form_pos.insert(id.index(), pos);
            }
        }
    }

    /// The recorded source position of a form, if it is a LOCAL list with one.
    pub fn form_pos(&self, v: Value) -> Option<crate::error::Pos> {
        if let Value::Pair(id) = v {
            if id.region() == crate::core::value::LOCAL {
                return self.form_pos.get(&id.index()).copied();
            }
        }
        None
    }

    /// Set the file currently being loaded, returning the previous value so the
    /// caller can restore it (loads nest).
    pub fn set_current_file(&mut self, file: Option<String>) -> Option<String> {
        std::mem::replace(&mut self.current_file, file)
    }

    /// The file currently being loaded, exposed to Brood via `(current-file)`.
    pub fn current_file(&self) -> Option<&str> {
        self.current_file.as_deref()
    }

    // ----- allocation (always into the local heap) -----

    pub fn alloc_pair(&mut self, head: Value, tail: Value) -> Value {
        let idx = self.local.pairs.len();
        self.local.pairs.push((head, tail));
        Value::Pair(PairId::local(idx))
    }

    pub fn alloc_vector(&mut self, items: Vec<Value>) -> Value {
        let idx = self.local.vectors.len();
        self.local.vectors.push(items);
        Value::Vector(VecId::local(idx))
    }

    pub fn alloc_string(&mut self, s: &str) -> Value {
        let idx = self.local.strings.len();
        self.local.strings.push(s.to_string());
        Value::Str(StrId::local(idx))
    }

    pub fn alloc_closure(&mut self, c: Closure) -> ClosureId {
        let idx = self.local.closures.len();
        self.local.closures.push(c);
        ClosureId::local(idx)
    }

    pub fn alloc_native(&mut self, f: NativeFn) -> Value {
        let idx = self.local.natives.len();
        self.local.natives.push(f);
        Value::Native(NativeId::local(idx))
    }

    /// Build a proper list from a vector of items.
    pub fn list(&mut self, items: Vec<Value>) -> Value {
        self.list_with_tail(items, Value::Nil)
    }

    /// Build a list of `items` ending in `tail`. A `Nil` tail gives a proper
    /// list; any other tail gives an improper (dotted) list, e.g. `(1 2 . 3)`.
    pub fn list_with_tail(&mut self, items: Vec<Value>, tail: Value) -> Value {
        let mut acc = tail;
        for item in items.into_iter().rev() {
            acc = self.alloc_pair(item, acc);
        }
        acc
    }

    // ----- promotion: copy code from LOCAL into the shared RUNTIME region -----

    /// Deep-copy a value's reachable structure from the local heap into the
    /// shared RUNTIME region, returning a handle valid in every inner process.
    /// `def`/`set!` of a global run this so the bound code/data is shareable;
    /// `spawn` runs it on the target function. Atoms and already-shared values
    /// (PRELUDE/RUNTIME) are returned unchanged — no copy.
    ///
    /// Appends only (never mutates existing shared code), so a redefinition adds
    /// a new version while in-flight calls keep running the old one.
    pub fn promote(&self, v: Value) -> Value {
        match v {
            Value::Str(id) if id.region() == LOCAL => {
                let s = self.string(id).to_string();
                Value::Str(StrId::runtime(self.runtime.code.strings.push(s)))
            }
            Value::Pair(id) if id.region() == LOCAL => self.promote_list(id),
            Value::Vector(id) if id.region() == LOCAL => {
                let items: Vec<Value> = self
                    .vector(id)
                    .to_vec()
                    .into_iter()
                    .map(|x| self.promote(x))
                    .collect();
                Value::Vector(VecId::runtime(self.runtime.code.vectors.push(items)))
            }
            Value::Fn(id) if id.region() == LOCAL => Value::Fn(self.promote_closure(id)),
            Value::Macro(id) if id.region() == LOCAL => Value::Macro(self.promote_closure(id)),
            // Atoms, and values already in PRELUDE/RUNTIME, need no copy.
            _ => v,
        }
    }

    /// Promote a local cons-chain. Walks the `cdr` spine *iteratively* so a long
    /// list doesn't recurse its length deep (which overflowed the native stack);
    /// recursion is bounded by element nesting via `promote` on each `car`.
    /// Stops at the first already-shared cell or non-pair tail, preserving both
    /// improper (dotted) lists and existing structure sharing.
    fn promote_list(&self, first: PairId) -> Value {
        let mut heads = Vec::new();
        let mut cur = Value::Pair(first);
        let tail = loop {
            match cur {
                Value::Pair(id) if id.region() == LOCAL => {
                    let (head, next) = self.pair(id);
                    heads.push(self.promote(head));
                    cur = next;
                }
                other => break self.promote(other),
            }
        };
        let mut acc = tail;
        for head in heads.into_iter().rev() {
            acc = Value::Pair(PairId::runtime(self.runtime.code.pairs.push((head, acc))));
        }
        acc
    }

    fn promote_closure(&self, id: ClosureId) -> ClosureId {
        let cl = self.closure(id).clone();
        let body = cl.body.iter().map(|&f| self.promote(f)).collect();
        let optionals = cl
            .optionals
            .iter()
            .map(|&(s, d)| (s, self.promote(d)))
            .collect();
        // A top-level closure captures the global env (`None`) and is fully
        // shareable as-is. A closure that captured a *local* scope has its scope
        // promoted too, so it resolves its free variables in any process.
        let env = cl.env.map(|e| self.promote_env(e));
        let promoted = Closure {
            name: cl.name,
            params: cl.params,
            optionals,
            rest: cl.rest,
            body,
            env,
        };
        ClosureId::runtime(self.runtime.code.closures.push(promoted))
    }

    /// Deep-copy an environment frame chain from LOCAL into the shared RUNTIME
    /// region, promoting each bound value. Stops at the global scope (the shared
    /// sentinel). Already-shared (RUNTIME) frames are returned unchanged.
    fn promote_env(&self, env: EnvId) -> EnvId {
        if env == EnvId::GLOBAL || env.region() == RUNTIME {
            return env;
        }
        // Snapshot the frame, then promote its parent and values (no borrow held).
        let (parent, bindings): (Option<EnvId>, Vec<(Symbol, Value)>) = {
            let frame = self.env_frame(env);
            (
                frame.parent,
                frame.vars.iter().map(|(&s, &v)| (s, v)).collect(),
            )
        };
        let parent = parent.map(|p| self.promote_env(p));
        let mut vars = HashMap::with_capacity(bindings.len());
        for (s, v) in bindings {
            vars.insert(s, self.promote(v));
        }
        EnvId::runtime(self.runtime.code.envs.push(EnvFrame { vars, parent }))
    }

    // ----- access (dispatch on the handle's region) -----

    pub fn pair(&self, id: PairId) -> (Value, Value) {
        match id.region() {
            LOCAL => self.local.pairs[id.index()],
            PRELUDE => self.prelude.slabs.pairs[id.index()],
            RUNTIME => *self
                .runtime
                .code
                .pairs
                .get(id.index())
                .expect("runtime pair handle"),
            _ => unreachable!("invalid handle region"),
        }
    }
    pub fn car(&self, id: PairId) -> Value {
        self.pair(id).0
    }
    pub fn cdr(&self, id: PairId) -> Value {
        self.pair(id).1
    }
    region_ref!(vector, VecId, vectors, &[Value], "runtime vector handle");
    region_ref!(string, StrId, strings, &str, "runtime string handle");
    region_ref!(closure, ClosureId, closures, &Closure, "runtime closure handle");

    pub fn native(&self, id: NativeId) -> &NativeFn {
        match id.region() {
            LOCAL => &self.local.natives[id.index()],
            PRELUDE => &self.prelude.slabs.natives[id.index()],
            _ => unreachable!("natives live only in the local or prelude region"),
        }
    }

    /// Collect a proper list into a `Vec`. Errors on an improper (dotted) list.
    pub fn list_to_vec(&self, v: Value) -> Result<Vec<Value>, LispError> {
        let mut out = Vec::new();
        let mut cur = v;
        loop {
            match cur {
                Value::Nil => return Ok(out),
                Value::Pair(p) => {
                    let (head, tail) = self.pair(p);
                    out.push(head);
                    cur = tail;
                }
                _ => return Err(LispError::type_err("improper list")),
            }
        }
    }

    /// Treat a list or vector as a sequence of items.
    pub fn seq_items(&self, v: Value) -> Result<Vec<Value>, LispError> {
        match v {
            Value::Nil => Ok(Vec::new()),
            Value::Pair(_) => self.list_to_vec(v),
            Value::Vector(id) => Ok(self.vector(id).to_vec()),
            _ => Err(LispError::type_err("expected a list or vector")),
        }
    }

    /// Structural equality (the basis of `=`). Functions/macros/natives compare
    /// by identity (same handle).
    ///
    /// Floats compare by IEEE value, so `-0.0 = 0.0` is true and `nan = nan` is
    /// false — the least-surprising arithmetic semantics (not bitwise equality).
    pub fn equal(&self, a: Value, b: Value) -> bool {
        use Value::*;
        match (a, b) {
            (Nil, Nil) => true,
            (Bool(x), Bool(y)) => x == y,
            (Int(x), Int(y)) => x == y,
            (Float(x), Float(y)) => x == y,
            (Sym(x), Sym(y)) => x == y,
            (Keyword(x), Keyword(y)) => x == y,
            (Str(x), Str(y)) => self.string(x) == self.string(y),
            // Walk the `cdr` spine iteratively so comparing long lists doesn't
            // recurse their length deep; recursion stays bounded by `car` nesting.
            (Pair(x), Pair(y)) => {
                let (mut x, mut y) = (x, y);
                loop {
                    let (a0, a1) = self.pair(x);
                    let (b0, b1) = self.pair(y);
                    if !self.equal(a0, b0) {
                        break false;
                    }
                    match (a1, b1) {
                        (Pair(nx), Pair(ny)) => {
                            x = nx;
                            y = ny;
                        }
                        _ => break self.equal(a1, b1),
                    }
                }
            }
            (Vector(x), Vector(y)) => {
                let xs = self.vector(x);
                let ys = self.vector(y);
                xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(&p, &q)| self.equal(p, q))
            }
            (Fn(x), Fn(y)) => x == y,
            (Macro(x), Macro(y)) => x == y,
            (Native(x), Native(y)) => x == y,
            _ => false,
        }
    }

    // ----- environments -----
    //
    // Real env frames are always LOCAL. The global scope is the sentinel
    // [`EnvId::GLOBAL`], which routes to the shared `runtime.globals` table; a
    // top-level frame's parent chain bottoms out there. (During prelude *build*
    // the global is instead a real local root frame with no parent.)

    fn env_frame(&self, env: EnvId) -> &EnvFrame {
        match env.region() {
            LOCAL => &self.local.envs[env.index()],
            RUNTIME => self
                .runtime
                .code
                .envs
                .get(env.index())
                .expect("runtime env frame"),
            _ => unreachable!("env frames live only in the local or runtime region"),
        }
    }

    pub fn new_env(&mut self, parent: Option<EnvId>) -> EnvId {
        let idx = self.local.envs.len();
        self.local.envs.push(EnvFrame {
            vars: HashMap::new(),
            parent,
        });
        EnvId::local(idx)
    }

    pub fn env_get(&self, env: EnvId, sym: Symbol) -> Option<Value> {
        let mut cur = Some(env);
        while let Some(e) = cur {
            if e == EnvId::GLOBAL {
                return self.runtime.globals_read().get(&sym).copied();
            }
            let frame = self.env_frame(e);
            if let Some(v) = frame.vars.get(&sym) {
                return Some(*v);
            }
            cur = frame.parent;
        }
        None
    }

    pub fn env_define(&mut self, env: EnvId, sym: Symbol, val: Value) {
        if env == EnvId::GLOBAL {
            // Global code/data is shared across inner processes, so promote it
            // into the shared RUNTIME region before binding.
            let shared = self.promote(val);
            self.runtime.globals_write().insert(sym, shared);
        } else {
            self.local.envs[env.index()].vars.insert(sym, val);
        }
    }

    /// Mutate the nearest existing binding; returns false if none exists.
    pub fn env_set(&mut self, env: EnvId, sym: Symbol, val: Value) -> bool {
        let mut cur = Some(env);
        while let Some(e) = cur {
            if e == EnvId::GLOBAL {
                let shared = self.promote(val);
                if let std::collections::hash_map::Entry::Occupied(mut slot) =
                    self.runtime.globals_write().entry(sym)
                {
                    slot.insert(shared);
                    return true;
                }
                return false;
            }
            if self.env_frame(e).vars.contains_key(&sym) {
                if e.region() == LOCAL {
                    self.local.envs[e.index()].vars.insert(sym, val);
                    return true;
                }
                // A promoted (shared, captured) frame is read-only — `set!` can't
                // cross the share boundary. Rare; a documented limitation.
                return false;
            }
            cur = self.env_frame(e).parent;
        }
        false
    }

    /// Snapshot the runtime's global bindings (`symbol -> value`). Cheap: the
    /// values are `Copy` handles. Pair with [`Heap::restore_globals`] to run code
    /// against a *private copy* of the globals — mutations to the live table can
    /// then be rolled back (this is what the `%isolate` primitive does for
    /// `:isolated` tests). Only meaningful when no other process is writing the
    /// table concurrently.
    pub fn snapshot_globals(&self) -> HashMap<Symbol, Value> {
        self.runtime.globals_read().clone()
    }

    /// Restore the runtime's global bindings from a [`Heap::snapshot_globals`]
    /// snapshot, discarding every `def`/`set!` made since it was taken. The
    /// append-only code slabs are *not* reclaimed (there's no GC yet), but the
    /// bindings revert — so a name `def`'d since the snapshot becomes unbound
    /// again, and a rebound name returns to its earlier value.
    pub fn restore_globals(&self, snapshot: HashMap<Symbol, Value>) {
        *self.runtime.globals_write() = snapshot;
    }

    /// Walk to the global scope at the bottom of the frame chain.
    pub fn env_root(&self, env: EnvId) -> EnvId {
        let mut cur = env;
        loop {
            if cur == EnvId::GLOBAL {
                return EnvId::GLOBAL;
            }
            match self.env_frame(cur).parent {
                Some(p) => cur = p,
                None => return cur, // the prelude builder's local root
            }
        }
    }
}
