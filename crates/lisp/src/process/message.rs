//! The `Send`, self-contained value form a [`Value`] takes to cross a heap.
//!
//! `Heap` (per-process) is `!Sync` and uses local-only handles, so a `Value`
//! can't directly cross into another process's heap or across the wire. This
//! module is the bridge: [`to_message`] deep-copies a `Value` out into a
//! [`Message`] (an inert, owned, `Send` tree); [`from_message`] rebuilds the
//! `Message` into a destination heap. Symbols travel by their interned id
//! locally and by name across nodes (the `dist` codec re-interns on arrival).
//!
//! Closures travel as data via [`ClosureMsg`] (ADR-033 closure-as-data),
//! capturing only the free *local* bindings the body actually references —
//! free globals re-resolve on the receiver.
//!
//! Both directions cap nesting at [`MAX_MESSAGE_DEPTH`]; the wire codec in
//! `crate::dist::wire` uses the same depth bound so round-trip is symmetric
//! and neither side can be tricked into overflowing its native Rust stack.

use std::sync::Arc;

use crate::core::blob::SharedBlob;
use crate::core::heap::Heap;
use crate::core::value::{self, Closure, ClosureId, EnvId, Symbol, Value};
use crate::error::{LispError, Pos};

/// A `Send`, self-contained copy of a value, for crossing heaps.
#[derive(Clone)]
pub enum Message {
    Nil,
    Bool(bool),
    Int(i64),
    /// An arbitrary-precision integer (a value outside the i64 range), sent as
    /// its decimal string — a portable form that round-trips across nodes (which
    /// have independent heaps) without a custom byte layout. The receiver's
    /// `from_message` parses it and `int_from_bigint`-normalizes it.
    BigInt(String),
    Float(f64),
    /// A small string sent inline by deep copy. Used for strings below
    /// [`crate::core::blob::SHARED_BLOB_THRESHOLD`] (where atomic refcount
    /// traffic would dominate the per-byte copy) and for any string arriving
    /// from a cross-node wire send (the sender's `Arc<SharedBlob>` cannot be
    /// shared across runtimes — the receiver re-allocates).
    Str(String),
    /// A large string sent by handle. The sender bumps the `Arc` refcount
    /// once, both sides keep the same `SharedBlob` identity, and no bytes are
    /// copied. Only used *within one runtime* (inner processes share an
    /// `Arc<BlobHeap>`). The dist wire encoder downgrades this back to
    /// `Str` because separate runtimes have independent blob lifetimes.
    StrShared(Arc<SharedBlob>),
    /// A **bitset** sent by handle (KI-4). Always Arc-backed, so a bitset *always*
    /// crosses by reference (a refcount bump, no byte copy) — its defining advantage
    /// over the bignum board. Within one runtime only; the dist wire encoder **rejects**
    /// it (separate runtimes have independent blob lifetimes, and the bytes aren't UTF-8
    /// so they can't ride the `StrShared`→`M_STR` path). A receiver in the same runtime
    /// reconstructs it with `alloc_bitset`. Never decoded as UTF-8 text.
    Bitset(Arc<SharedBlob>),
    Sym(Symbol),
    Keyword(Symbol),
    /// A cons-list value, plus the **source position** of the original pair
    /// (if known). Carrying the `Pos` here lets a remote-shipped closure's
    /// body forms keep their source coordinates through `(send …)` and across
    /// nodes — the receiver's `from_message` re-stamps it on the rebuilt pair
    /// via `heap.set_form_pos`, so a diagnostic from inside a remote-run
    /// lambda still points at the *sender's* source line. `None` for lists
    /// built at runtime (no recorded position to begin with).
    List(Vec<Message>, Option<Pos>),
    Vector(Vec<Message>),
    Map(Vec<(Message, Message)>),
    Ref(u64),
    /// A process id carrying node identity. In-process this keeps the interned
    /// node `Symbol`; the node-link wire codec (`crate::dist`) re-encodes the
    /// node by *name*, since separate runtimes have independent interners.
    Pid {
        node: Symbol,
        id: u64,
    },
    /// A TCP socket id. Valid only *within one runtime* (the socket registry is
    /// global to the OS process); the dist wire codec rejects it, since the id is
    /// meaningless on another node.
    Socket(u64),
    /// A child-process id. Valid only *within one runtime* (the subprocess registry
    /// is global to the OS process); the dist wire codec rejects it, since the id is
    /// meaningless on another node. The subprocess reader thread emits this in its
    /// `[:proc handle …]` mailbox messages.
    Subprocess(u64),
    /// An in-memory table id (Brood's ETS, ADR-107). Valid only *within one runtime*
    /// (the table registry is global to the OS process) — it may cross in a message or
    /// be captured by a `spawn`ed closure, so many processes share one store. NOT
    /// node-portable: the cross-node wire codec rejects it (the id means nothing in
    /// another runtime). Only the handle rides the message; the store's contents are
    /// deep clones already.
    Table(u64),
    /// A serialised closure (Erlang's "send a fun"). Because a closure's body and
    /// its optionals' defaults are S-expression *forms* (plain data), and its free
    /// globals resolve on the receiver, a function can travel as data. Only its free
    /// *local* variables are copied (see [`ClosureMsg::captured`]). This is what
    /// makes `(spawn …)` shippable to another node — see `docs/decisions.md`.
    Closure(Box<ClosureMsg>),
}

/// The wire form of a [`Closure`]: everything but the global env, which is
/// re-resolved on the receiver rather than copied.
///
/// `pub(crate)` fields rather than accessors: the wire codec in
/// `crate::dist` needs every field (closure-as-data shipping; ADR-033) and
/// they're inert plain data once built — no invariant to defend at the
/// boundary.
#[derive(Clone)]
pub struct ClosureMsg {
    pub(crate) name: Option<Symbol>,
    /// One per arity clause (a single-arity closure has one). See `ClosureArm`.
    pub(crate) arms: Vec<ClosureArmMsg>,
    pub(crate) doc: Option<String>,
    /// The closure's *free variables* that resolve to a **local** binding, flattened
    /// to one frame (name → value). Empty = a global-capturing closure (the common
    /// case, e.g. a `(spawn (* (+ 1 1)))` thunk). We copy only what the body actually
    /// references from its lexical scope — not the whole frame chain — so unrelated
    /// (and possibly unsendable) siblings don't ride along, and a closure capturing a
    /// sibling closure can't form a serialisation cycle through its defining frame.
    pub(crate) captured: Vec<(Symbol, Message)>,
}

/// One arity clause of a [`ClosureMsg`] — the sendable (deep-copied) form of a
/// `ClosureArm`. Params/rest are interned symbols; optionals' defaults and the
/// body are code-as-data.
#[derive(Clone)]
pub struct ClosureArmMsg {
    pub(crate) params: Vec<Symbol>,
    pub(crate) optionals: Vec<(Symbol, Message)>,
    pub(crate) rest: Option<Symbol>,
    pub(crate) body: Vec<Message>,
}

/// Maximum nesting depth `to_message` will descend into. Past this, the
/// serialiser errors out — a deeply nested local data structure (built by a
/// `cons`-in-a-loop or a runaway recursion) should produce a clean error
/// rather than aborting the sender thread with a stack overflow. The wire
/// decoder (`dist::wire::MAX_DECODE_DEPTH`) is defined in terms of this so the
/// two can't diverge — wire round-trip stays symmetric.
pub(crate) const MAX_MESSAGE_DEPTH: u32 = 256;

/// Deep-copy a value out of `heap` into a `Send` message. A closure is sent as
/// data (see [`ClosureMsg`]); builtins and macros can't be.
pub fn to_message(heap: &Heap, v: Value) -> Result<Message, LispError> {
    to_message_rec(heap, v, &mut Vec::new(), 0)
}

/// `visited` carries the closures currently being serialised, so a self- or
/// mutually-recursive *local* closure is rejected cleanly instead of looping.
fn to_message_rec(
    heap: &Heap,
    v: Value,
    visited: &mut Vec<ClosureId>,
    depth: u32,
) -> Result<Message, LispError> {
    if depth >= MAX_MESSAGE_DEPTH {
        return Err(LispError::runtime(format!(
            "value nested deeper than {MAX_MESSAGE_DEPTH} levels (cannot serialise)",
        ))
        .with_code(crate::error::error_codes::MESSAGE_TOO_DEEP)
        .with_hint(
            "messages cross processes by deep copy — flatten or chunk the data \
             (e.g. send a list of items rather than one nested tree)",
        ));
    }
    Ok(match v {
        Value::Nil => Message::Nil,
        Value::Bool(b) => Message::Bool(b),
        Value::Int(n) => Message::Int(n),
        Value::BigInt(id) => Message::BigInt(heap.bigint(id).to_string()),
        // A bitset always ships its Arc<SharedBlob> by reference (no byte copy) — its
        // whole point. Byte-clean: never read as a UTF-8 string.
        Value::Bitset(id) => Message::Bitset(Arc::clone(heap.bitset(id))),
        Value::Float(f) => Message::Float(f),
        Value::Sym(s) => Message::Sym(s),
        Value::Keyword(s) => Message::Keyword(s),
        Value::Str(id) => match heap.local_shared_blob(id) {
            // LOCAL Shared: ship the Arc (atomic incr, no copy). Receiver
            // installs the same handle into its own slab via
            // `alloc_string_from_shared`. PRELUDE/RUNTIME and LOCAL Inline
            // fall through to the deep-copy `Str` path.
            Some(blob) => Message::StrShared(blob),
            None => Message::Str(heap.string(id).to_string()),
        },
        Value::Pair(_) => {
            let pos = heap.form_pos(v);
            let items = heap.list_to_vec(v)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(heap, item, visited, depth + 1)?);
            }
            Message::List(out, pos)
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(heap, item, visited, depth + 1)?);
            }
            Message::Vector(out)
        }
        // A range crosses as the list it stands in for (its elements are plain
        // ints; rare across a message boundary, so realising it is fine).
        Value::Range(id) => {
            let pos = heap.form_pos(v);
            let items = heap.range_to_vec(id);
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(heap, item, visited, depth + 1)?);
            }
            Message::List(out, pos)
        }
        // A lazy seq-view can't be realised here (`to_message` has only `&Heap`,
        // no evaluator to run its transducer). The prelude `send`/`!` realise a
        // view before it crosses, so this is the never-panic fallback for an
        // escaped raw view: a clear error rather than silent corruption.
        Value::SeqView(_) => {
            return Err(LispError::type_err(
                "cannot send a lazy seq-view in a message; realise it first \
                 (e.g. with `seq`, `vec`, or `into`)",
            ))
        }
        Value::Map(id) => {
            let entries = heap.map_entries(id);
            let mut out = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                out.push((
                    to_message_rec(heap, k, visited, depth + 1)?,
                    to_message_rec(heap, v, visited, depth + 1)?,
                ));
            }
            Message::Map(out)
        }
        Value::Ref(n) => Message::Ref(n),
        Value::Pid { node, id } => Message::Pid { node, id },
        Value::Fn(id) => {
            Message::Closure(Box::new(closure_to_message(heap, id, visited, depth + 1)?))
        }
        Value::Macro(_) => return Err(LispError::type_err("cannot send a macro in a message")),
        Value::Native(_) => {
            // A builtin is a Rust function pointer with no portable form — and on
            // another node the receiver has its own copy anyway. Reference it by
            // the symbol it's bound to instead of capturing its value.
            return Err(LispError::type_err(
                "cannot send a builtin in a message; reference it by name (code is shared)",
            ));
        }
        Value::Rope(_) => {
            // A rope is process-local: it lives in exactly one process's heap
            // (the buffer-as-process model, ADR-045). Move its *content* across
            // as a string instead — the receiver rebuilds a rope if it needs one.
            return Err(LispError::type_err(
                "cannot send a rope in a message; send (rope->string r) and \
                 rebuild with (string->rope s) on the other side",
            ));
        }
        // A socket is a global-registry id (not a per-heap handle like a rope),
        // so it is valid across every green process *in this runtime* — it may
        // cross in a message or be captured by a `spawn`ed closure (the
        // per-connection-handler pattern). It is NOT node-portable: the cross-node
        // wire codec rejects it (the id means nothing in another runtime).
        Value::Socket(id) => Message::Socket(id),
        // A subprocess is a global-registry id like a socket (the owning process
        // drives it and receives its output as messages); the reader thread emits
        // `[:proc handle …]`, so the handle must round-trip through a message. Valid
        // across this runtime's processes; not node-portable.
        Value::Subprocess(id) => Message::Subprocess(id),
        // A table is a global-registry id like a socket: the handle rides the message
        // so many processes share one store. Valid across this runtime; not
        // node-portable (the wire codec rejects it).
        Value::Table(id) => Message::Table(id),
        Value::Transient(_) => {
            // A transient is a process-local, identity-mutable build handle (its
            // root maps slab is LOCAL). Deep-copying it across processes would
            // both break identity-mutation and dangle the watermark. Make it
            // persistent first and send the resulting immutable map.
            return Err(LispError::type_err(
                "cannot send a transient in a message; call (persistent! t) and \
                 send the resulting map",
            ));
        }
    })
}

/// Serialise a closure into its wire form. The body and optional-default *forms*
/// are data (S-expressions), so they go straight through. For the environment we
/// copy only the **free variables that resolve to a local binding** — every symbol
/// the body/defaults mention, looked up in the captured frame chain *below* the
/// global scope. Free globals are skipped (they re-resolve on the receiver), which
/// is also why a builtin reached only via a global symbol never gets dragged in.
fn closure_to_message(
    heap: &Heap,
    id: ClosureId,
    visited: &mut Vec<ClosureId>,
    depth: u32,
) -> Result<ClosureMsg, LispError> {
    if visited.contains(&id) {
        // The free-variable walk re-entered this same closure: a local closure that
        // refers to itself (or a cycle of them). Top-level recursion is fine — those
        // capture the global env (no local capture) and resolve by name.
        return Err(LispError::type_err(
            "cannot send a self-referential local closure (define it at top level instead)",
        ));
    }
    visited.push(id);
    // Borrow the closure — `to_message_rec` only needs `&Heap`, so there's no need
    // to clone the whole `Closure` (notably its body `Vec`) on every send.
    let cl = heap.closure(id);

    // Copy only the free variables that resolve to a *local* binding. Skipped
    // entirely for a global-capturing closure (no local env) — the common case
    // (e.g. a `(spawn …)` thunk), so collecting symbols costs nothing there.
    let mut captured = Vec::new();
    if let Some(env) = cl.env {
        let mut mentioned = std::collections::HashSet::new();
        for arm in &cl.arms {
            for &form in &arm.body {
                collect_symbols(heap, form, &mut mentioned);
            }
            for &(_, d) in &arm.optionals {
                collect_symbols(heap, d, &mut mentioned);
            }
        }
        for sym in mentioned {
            if let Some(val) = local_lookup(heap, env, sym) {
                captured.push((sym, to_message_rec(heap, val, visited, depth)?));
            }
        }
    }

    // Deep-copy each arm's `&optional` defaults and body (code-as-data).
    let mut arms = Vec::with_capacity(cl.arms.len());
    for arm in &cl.arms {
        let optionals = arm
            .optionals
            .iter()
            .map(|&(s, d)| Ok((s, to_message_rec(heap, d, visited, depth)?)))
            .collect::<Result<Vec<_>, LispError>>()?;
        let body = arm
            .body
            .iter()
            .map(|&f| to_message_rec(heap, f, visited, depth))
            .collect::<Result<Vec<_>, LispError>>()?;
        arms.push(ClosureArmMsg {
            params: arm.params.clone(),
            optionals,
            rest: arm.rest,
            body,
        });
    }

    visited.pop();
    Ok(ClosureMsg {
        name: cl.name,
        arms,
        doc: cl.doc.clone(),
        captured,
    })
}

/// Collect every symbol that appears anywhere in `form` (operator or operand
/// position, at any depth) into `out`. Deliberately over-approximate: it doesn't
/// track nested binders, because the [`local_lookup`] filter in `closure_to_message`
/// keeps only names that actually resolve to a captured local — a param or a
/// not-yet-bound inner name simply isn't there, so it's harmless to list it.
fn collect_symbols(heap: &Heap, form: Value, out: &mut std::collections::HashSet<Symbol>) {
    match form {
        Value::Sym(s) => {
            out.insert(s);
        }
        Value::Pair(_) => {
            // Walk the spine *iteratively* so a long list can't overflow the stack
            // (recursion depth stays bounded by nesting, not length), with no
            // `list_to_vec` allocation per node. The trailing `collect_symbols` on the
            // final non-pair tail also covers an improper `(a . b)` (and `Nil` no-ops).
            let mut cur = form;
            while let Value::Pair(id) = cur {
                let (car, cdr) = heap.pair(id);
                collect_symbols(heap, car, out);
                cur = cdr;
            }
            collect_symbols(heap, cur, out);
        }
        Value::Vector(id) => {
            for item in heap.vector(id).to_vec() {
                collect_symbols(heap, item, out);
            }
        }
        Value::Map(id) => {
            for (k, v) in heap.map_entries(id) {
                collect_symbols(heap, k, out);
                collect_symbols(heap, v, out);
            }
        }
        _ => {}
    }
}

/// Look `sym` up in the local frame chain rooted at `env`, stopping *before* the
/// global scope — so only a genuinely captured lexical binding is returned, never
/// a global. `None` means it's a global (resolved on the receiver) or unbound.
fn local_lookup(heap: &Heap, env: EnvId, sym: Symbol) -> Option<Value> {
    let mut cur = Some(env);
    while let Some(e) = cur {
        if e == EnvId::GLOBAL {
            break;
        }
        let (parent, vars) = heap.env_frame_ref(e);
        // Scan from the end so a later binding shadows an earlier one (as `env_get`).
        if let Some(&(_, v)) = vars.iter().rev().find(|&&(s, _)| s == sym) {
            return Some(v);
        }
        cur = parent;
    }
    None
}

/// Rebuild a message into `heap`.
pub fn from_message(heap: &mut Heap, m: &Message) -> Value {
    match m {
        Message::Nil => Value::Nil,
        Message::Bool(b) => Value::Bool(*b),
        Message::Int(n) => Value::Int(*n),
        Message::BigInt(s) => match s.parse::<num_bigint::BigInt>() {
            // Normalize through `int_from_bigint` so a value that (against the
            // sender's invariant) fits i64 still demotes to `Int`.
            Ok(n) => heap.int_from_bigint(n),
            // A malformed decimal string can only come from a corrupt/forged
            // wire frame; fall back to 0 rather than panic the receiver.
            Err(_) => Value::Int(0),
        },
        Message::Float(f) => Value::Float(*f),
        Message::Sym(s) => Value::Sym(*s),
        Message::Keyword(s) => Value::Keyword(*s),
        Message::Str(s) => heap.alloc_string(s),
        Message::StrShared(blob) => heap.alloc_string_from_shared(Arc::clone(blob)),
        Message::Bitset(blob) => heap.alloc_bitset(Arc::clone(blob)),
        Message::List(items, pos) => {
            let mut vals = Vec::with_capacity(items.len());
            for item in items {
                vals.push(from_message(heap, item));
            }
            let v = heap.list(vals);
            // Re-stamp the original source position on the rebuilt pair, so
            // a diagnostic from inside a sent / remote-spawned closure still
            // points at the sender's source line. `set_form_pos` no-ops on
            // non-LOCAL handles, but `heap.list` always produces LOCAL.
            if let Some(p) = pos {
                heap.set_form_pos(v, *p);
            }
            v
        }
        Message::Vector(items) => {
            let mut vals = Vec::with_capacity(items.len());
            for item in items {
                vals.push(from_message(heap, item));
            }
            heap.alloc_vector(vals)
        }
        Message::Map(entries) => {
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = from_message(heap, k);
                let v = from_message(heap, v);
                pairs.push((k, v));
            }
            heap.map_from_pairs(pairs)
        }
        Message::Ref(n) => Value::Ref(*n),
        Message::Pid { node, id } => Value::Pid {
            node: *node,
            id: *id,
        },
        Message::Socket(id) => Value::Socket(*id),
        Message::Subprocess(id) => Value::Subprocess(*id),
        Message::Table(id) => Value::Table(*id),
        Message::Closure(c) => closure_from_message(heap, c),
    }
}

/// Rebuild a serialised closure into `heap`. Body/optional-default forms are
/// reconstructed as local data; captured frames are recreated (outermost first)
/// and chained onto this process's global scope, so the closure's free globals
/// resolve here. The result is a fresh, independent copy — a later redefinition
/// of *this* function won't reach it, but globals it *references* still do.
fn closure_from_message(heap: &mut Heap, c: &ClosureMsg) -> Value {
    // Rebuild every arm's optional-default forms and body as local data.
    let arms = c
        .arms
        .iter()
        .map(|arm| {
            let optionals = arm
                .optionals
                .iter()
                .map(|(s, d)| (*s, from_message(heap, d)))
                .collect();
            let body = arm.body.iter().map(|f| from_message(heap, f)).collect();
            value::ClosureArm {
                params: arm.params.clone(),
                optionals,
                rest: arm.rest,
                body,
                passthrough: None, // recomputed by `alloc_closure` on rebuild
            }
        })
        .collect();
    // Rebuild the captured free vars as one frame chained onto this process's
    // global scope, so the closure's free globals resolve here. No captures =>
    // a global-capturing closure (`env: None`).
    let env = if c.captured.is_empty() {
        None
    } else {
        let e = heap.new_env(Some(EnvId::GLOBAL));
        for (s, m) in &c.captured {
            let v = from_message(heap, m);
            heap.env_define(e, *s, v);
        }
        Some(e)
    };
    let id = heap.alloc_closure(Closure {
        name: c.name,
        arms,
        doc: c.doc.clone(),
        env,
    });
    Value::Fn(id)
}
