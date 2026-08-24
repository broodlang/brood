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
    /// An arbitrary-precision base-10 decimal, sent as its canonical decimal
    /// string (mirrors [`Message::BigInt`]) — a portable form that round-trips
    /// across nodes without a custom byte layout. The receiver's `from_message`
    /// parses it back into a `Value::Decimal`.
    Decimal(String),
    /// An exact rational, sent as its `num/den` string (mirrors [`Message::Decimal`]).
    /// The receiver's `from_message` parses it back into a `Value::Ratio`.
    Ratio(String),
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
    /// **Raw bytes** sent by handle. Always
    /// Arc-backed, so within one runtime it crosses by reference (a refcount bump, no byte
    /// copy). A same-runtime receiver reconstructs it with `alloc_bytes`. Across the wire (a
    /// cross-node send, or the startup image) it is copied inline as a length-prefixed raw
    /// blob — immutable data, so the receiver allocates its own `SharedBlob` with no shared
    /// identity. Never decoded as UTF-8 text.
    Bytes(Arc<SharedBlob>),
    Sym(Symbol),
    Keyword(Symbol),
    /// A closure sent **by shared handle** rather than by deep copy — the serialised
    /// counterpart of ADR-194's L1 fast path, and the fix for the throughput/RSS decay in
    /// `docs/handoff.md` thread 6.
    ///
    /// Only ever produced when the closure already lives in the shared RUNTIME region and
    /// the target process is on the **same runtime** (`Mailbox::runtime_tag`), so the handle
    /// means the same thing on both sides. Deep-copying it instead is what made the receiver
    /// hold a LOCAL closure, which has no VM-eligible arm (`cache_key` requires a non-LOCAL
    /// body), so it tree-walked every call and `spawn_impl` re-promoted a fresh copy into the
    /// append-only region per call — ~0.87 RUNTIME closures per operation, unbounded.
    ///
    /// `pin` keeps the handle's generation alive for exactly as long as this message exists.
    /// A queued message is in no heap and no process's roots, so the drain's reachability
    /// probe cannot see it — and the drain's cached clean ack explicitly assumes it cannot
    /// exist. See [`crate::core::heap::GenPin`]. The dist wire encoder must reject this
    /// variant: a handle is meaningless to another runtime (separate regions).
    FnShared {
        bits: u64,
        pin: crate::core::heap::GenPin,
    },
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
    /// A set value — its elements (the backing values are all `true`, so only the
    /// elements ship). Rebuilt as a `Value::Set` on the receiver, preserving the
    /// distinct set type across a `send`/node boundary.
    Set(Vec<Message>),
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
    /// A builtin (`Value::Native`) carried by the **name** it is bound to, not by its Rust
    /// function pointer (which has no portable form). Produced ONLY by the startup-image
    /// writer ([`to_message_image`]): the image restores global bindings in the same runtime,
    /// where a builtin is a stable, registered primitive, so it travels by name and
    /// [`from_message`] re-resolves it. A cross-process / cross-node *message* still refuses a
    /// builtin — the image is binary-keyed (build-id in its fingerprint), so the name is
    /// guaranteed to resolve to the same primitive on read; a plain message has no such
    /// guarantee. The wire codec encodes it like any other by-name symbol.
    Native(Symbol),
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
    crate::perf_time!(ns_msg_out, { to_message_timed(heap, v) })
}

fn to_message_timed(heap: &Heap, v: Value) -> Result<Message, LispError> {
    to_message_rec(heap, v, &mut Vec::new(), 0, None)
}

/// [`to_message`], but told which runtime the message is destined for. When that is *this*
/// runtime, an already-shared RUNTIME closure crosses as a handle ([`Message::FnShared`])
/// instead of being deep-copied. `None` — every other caller, including the whole dist wire
/// path — behaves exactly as before.
pub fn to_message_to_runtime(
    heap: &Heap,
    v: Value,
    dest_runtime: Option<u64>,
) -> Result<Message, LispError> {
    to_message_rec(heap, v, &mut Vec::new(), 0, dest_runtime)
}

thread_local! {
    /// Set only while the startup-image writer serialises a global: it flips a builtin from
    /// "refused" to [`Message::Native`]. A plain `send` never sets it, so message and wire
    /// semantics are unchanged. See [`to_message_image`].
    static IMAGE_NATIVE_BY_NAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Reverts [`IMAGE_NATIVE_BY_NAME`] on drop, so an early `?`-return or a panic can't leave it
/// stuck on for the next `to_message` on this thread.
struct ImageNativeMode;
impl ImageNativeMode {
    fn enter() -> Self {
        IMAGE_NATIVE_BY_NAME.with(|f| f.set(true));
        ImageNativeMode
    }
}
impl Drop for ImageNativeMode {
    fn drop(&mut self) {
        IMAGE_NATIVE_BY_NAME.with(|f| f.set(false));
    }
}

/// [`to_message`] for the **startup image** only: a builtin is serialised by name
/// ([`Message::Native`]) instead of being refused, because the image restores bindings in the
/// same runtime where the primitive is registered under that name (and is binary-keyed, so it
/// re-resolves). Everything else behaves exactly as `to_message`.
pub fn to_message_image(heap: &Heap, v: Value) -> Result<Message, LispError> {
    let _mode = ImageNativeMode::enter();
    to_message_rec(heap, v, &mut Vec::new(), 0, None)
}

/// `BROOD_NO_SHARE_FN_MSG=1` reverts a serialised same-runtime send to deep-copying the
/// closure — the A/B lever and the stopgap if a shared handle is implicated in a fault. The
/// sibling of `BROOD_NO_SHARE_FN`, which covers the L1 (parked-receiver) path.
fn share_fn_msg_enabled() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("BROOD_NO_SHARE_FN_MSG").is_none())
}

/// The death reason for a process killed by an uncaught error: `[:error {…}]`,
/// where the map mirrors [`LispError::to_value_map`] — `{:kind :message [:code
/// :file :line :col :hint :trace]}`, `:trace` a list of `{:fn [:file :line
/// :col]}` frames — so a monitor's `[:down …]` / a trapping link's `[:EXIT …]`
/// carries the **structured** error (BEAM's `{Reason, Stacktrace}` parity), not
/// a flattened string. Built as a heap-independent [`Message`] directly (the
/// dying process's heap is about to drop), so it deep-copies into any
/// receiver's heap and crosses the dist wire intact. A supervisor can log
/// `(get m :message)` / walk `(get m :trace)` from the reason alone.
pub fn error_reason(e: &crate::error::LispError) -> Message {
    let kw = |s: &str| Message::Keyword(crate::core::value::intern(s));
    let mut m: Vec<(Message, Message)> = Vec::with_capacity(8);
    m.push((kw("kind"), kw(e.kind.tag_name())));
    m.push((kw("message"), Message::Str(e.message.clone())));
    if let Some(code) = e.code {
        m.push((kw("code"), Message::Str(code.to_string())));
    }
    if let Some(file) = &e.file {
        m.push((kw("file"), Message::Str(file.clone())));
    }
    if let Some(pos) = e.pos {
        m.push((kw("line"), Message::Int(pos.line as i64)));
        m.push((kw("col"), Message::Int(pos.col as i64)));
    }
    if let Some(hint) = &e.hint {
        m.push((kw("hint"), Message::Str(hint.clone())));
    }
    if !e.trace.is_empty() {
        let frames: Vec<Message> = e
            .trace
            .iter()
            .map(|f| {
                let mut fm: Vec<(Message, Message)> = Vec::with_capacity(4);
                if let Some(name) = f.name {
                    fm.push((kw("fn"), Message::Str(name.to_string())));
                }
                if let Some(file) = &f.file {
                    fm.push((kw("file"), Message::Str(file.clone())));
                }
                if let Some(pos) = f.pos {
                    fm.push((kw("line"), Message::Int(pos.line as i64)));
                    fm.push((kw("col"), Message::Int(pos.col as i64)));
                }
                Message::Map(fm)
            })
            .collect();
        m.push((kw("trace"), Message::List(frames, None)));
    }
    Message::Vector(vec![kw(crate::process::keywords::ERROR), Message::Map(m)])
}

/// `visited` carries the closures currently being serialised, so a self- or
/// mutually-recursive *local* closure is rejected cleanly instead of looping.
fn to_message_rec(
    heap: &Heap,
    v: Value,
    visited: &mut Vec<ClosureId>,
    depth: u32,
    dest_runtime: Option<u64>,
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
        // A decimal ships as its canonical decimal string (mirrors BigInt).
        Value::Decimal(id) => Message::Decimal(heap.decimal(id).to_string()),
        // A ratio ships as its `num/den` string (mirrors BigInt/Decimal).
        Value::Ratio(id) => Message::Ratio(heap.ratio(id).to_string()),
        // Raw bytes ship their Arc<SharedBlob> by reference (no byte copy). Byte-clean.
        Value::Bytes(id) => Message::Bytes(Arc::clone(&heap.bytes(id))),
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
            let pos = heap.form_pos_only(v);
            let items = heap.list_to_vec(v)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(
                    heap,
                    item,
                    visited,
                    depth + 1,
                    dest_runtime,
                )?);
            }
            Message::List(out, pos)
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(
                    heap,
                    item,
                    visited,
                    depth + 1,
                    dest_runtime,
                )?);
            }
            Message::Vector(out)
        }
        // A range crosses as the list it stands in for (its elements are plain
        // ints; rare across a message boundary, so realising it is fine).
        Value::Range(id) => {
            let pos = heap.form_pos_only(v);
            let items = heap.range_to_vec(id)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(
                    heap,
                    item,
                    visited,
                    depth + 1,
                    dest_runtime,
                )?);
            }
            Message::List(out, pos)
        }
        // A lazy seq-view can't be realised here (`to_message` has only `&Heap`,
        // no evaluator to run its transducer). `send` is a Rust builtin with no
        // pre-realize step, so a raw view reaches here directly: return a clear
        // error (the caller must realise it first) rather than risk silent
        // corruption — never a panic.
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
                    to_message_rec(heap, k, visited, depth + 1, dest_runtime)?,
                    to_message_rec(heap, v, visited, depth + 1, dest_runtime)?,
                ));
            }
            Message::Map(out)
        }
        Value::Set(id) => {
            let elems = heap.set_elems(id);
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(to_message_rec(heap, e, visited, depth + 1, dest_runtime)?);
            }
            Message::Set(out)
        }
        Value::Ref(n) => Message::Ref(n),
        Value::Pid { node, id } => Message::Pid { node, id },
        Value::Fn(id) => {
            // Share by handle when the destination is THIS runtime and the closure already
            // lives in the shared region — the serialised twin of ADR-194's L1 path, and the
            // guards mirror it exactly. RUNTIME only: never promote to make something
            // shareable, which grows the append-only region per send (measured 541 MB vs
            // 150 MB over 800k transient sends). Same runtime only: the process registry is
            // global, so a second `Interp`'s handles must not cross. Plus an off-switch.
            // `pin_gen_of` holds the handle's generation for the message's whole life.
            if let Some(tag) = dest_runtime {
                if share_fn_msg_enabled()
                    && id.region() == crate::core::value::RUNTIME
                    && tag == heap.runtime_tag()
                {
                    if let Some(pin) = heap.pin_gen_of(id) {
                        return Ok(Message::FnShared { bits: id.0, pin });
                    }
                }
            }
            Message::Closure(Box::new(closure_to_message(
                heap,
                id,
                visited,
                depth + 1,
                dest_runtime,
            )?))
        }
        Value::Macro(_) => return Err(LispError::type_err("cannot send a macro in a message")),
        Value::Native(id) => {
            // A builtin is a Rust function pointer with no portable form — and on another node
            // the receiver has its own copy anyway. In a MESSAGE it is refused (the sender must
            // reference it by name). But the STARTUP IMAGE carries it by the name it is bound
            // to: the image restores bindings in the same runtime and is binary-keyed, so the
            // name re-resolves to the same primitive on read. See `Message::Native`.
            if IMAGE_NATIVE_BY_NAME.with(|f| f.get()) {
                Message::Native(crate::core::value::intern(&heap.native(id).name))
            } else {
                return Err(LispError::type_err(
                    "cannot send a builtin in a message; reference it by name (code is shared)",
                ));
            }
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
    dest_runtime: Option<u64>,
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
        for arm in cl.arms.iter() {
            for &form in &arm.body {
                collect_symbols(heap, form, &mut mentioned);
            }
            for &(_, d) in &arm.optionals {
                collect_symbols(heap, d, &mut mentioned);
            }
        }
        for sym in mentioned {
            if let Some(val) = local_lookup(heap, env, sym) {
                captured.push((
                    sym,
                    to_message_rec(heap, val, visited, depth, dest_runtime)?,
                ));
            }
        }
    }

    // Deep-copy each arm's `&optional` defaults and body (code-as-data).
    let mut arms = Vec::with_capacity(cl.arms.len());
    for arm in cl.arms.iter() {
        let optionals = arm
            .optionals
            .iter()
            .map(|&(s, d)| Ok((s, to_message_rec(heap, d, visited, depth, dest_runtime)?)))
            .collect::<Result<Vec<_>, LispError>>()?;
        let body = arm
            .body
            .iter()
            .map(|&f| to_message_rec(heap, f, visited, depth, dest_runtime))
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
    crate::perf_time!(ns_msg_in, { from_message_timed(heap, m) })
}

fn from_message_timed(heap: &mut Heap, m: &Message) -> Value {
    match m {
        // A closure handed over by handle (same runtime, already-shared region). No rebuild,
        // no allocation — the point of the whole exercise.
        //
        // `rearm_drain_ack` is the other half of the lifetime argument. Until now this heap
        // may have acked a RUNTIME drain "clean", and `report_gen_liveness` caches that for
        // the whole epoch because "an old-gen handle can never arrive by message (messages
        // deep-copy)". It just did. Forgetting the ack makes this process re-walk on its next
        // safepoint, where Phase 2 finds the handle in its local heap and pins the generation
        // the ordinary way. The message's `GenPin` covers the window before that walk.
        Message::FnShared { bits, .. } => {
            heap.rearm_drain_ack();
            Value::Fn(crate::core::value::ClosureId(*bits))
        }
        Message::Nil => Value::nil(),
        Message::Bool(b) => Value::boolean(*b),
        Message::Int(n) => Value::int(*n),
        Message::BigInt(s) => match s.parse::<num_bigint::BigInt>() {
            // Normalize through `int_from_bigint` so a value that (against the
            // sender's invariant) fits i64 still demotes to `Int`.
            Ok(n) => heap.int_from_bigint(n),
            // A malformed decimal string can only come from a corrupt/forged
            // wire frame; fall back to 0 rather than panic the receiver.
            Err(_) => Value::int(0),
        },
        Message::Decimal(s) => match s.parse::<bigdecimal::BigDecimal>() {
            Ok(n) => heap.alloc_decimal(n),
            // A malformed decimal string can only come from a corrupt/forged
            // wire frame; fall back to 0 rather than panic the receiver.
            Err(_) => Value::int(0),
        },
        Message::Ratio(s) => match s.parse::<num_rational::BigRational>() {
            Ok(n) => heap.alloc_ratio(n),
            Err(_) => Value::int(0),
        },
        Message::Float(f) => Value::float(*f),
        Message::Sym(s) => Value::symbol(*s),
        Message::Keyword(s) => Value::keyword(*s),
        Message::Native(s) => {
            // Re-resolve the builtin by the name it was imaged under. The image is binary-keyed
            // (build-id in its fingerprint), so the same primitive is registered under this name
            // on read. Defensive fallback to nil if a stale image ever slipped through — a
            // missing binding, never a wrong one. Only the image produces this variant.
            match heap.env_get(heap.global(), *s) {
                Some(v @ Value::Native(_)) => v,
                _ => Value::nil(),
            }
        }
        Message::Str(s) => heap.alloc_string(s),
        Message::StrShared(blob) => heap.alloc_string_from_shared(Arc::clone(blob)),
        Message::Bytes(blob) => heap.alloc_bytes(Arc::clone(blob)),
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
        Message::Set(items) => {
            let mut elems = Vec::with_capacity(items.len());
            for item in items {
                elems.push(from_message(heap, item));
            }
            heap.set_from_elems(elems)
        }
        Message::Ref(n) => Value::ref_(*n),
        Message::Pid { node, id } => Value::pid(*node, *id),
        Message::Socket(id) => Value::socket(*id),
        Message::Subprocess(id) => Value::subprocess(*id),
        Message::Table(id) => Value::table(*id),
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
    Value::func(id)
}

// ---- inbound IO chunk decoding (proc/net readers) ----

/// Decode one inbound IO chunk into the payload to deliver, carrying an incomplete
/// trailing UTF-8 sequence across reads so a multi-byte character split at a
/// read-buffer (64 KiB) boundary is not mangled.
///
/// - **Binary mode** (`binary == true`): byte-faithful. Any bytes held in `carry`
///   from a prior text-mode read are flushed *ahead of* `chunk` into one `bytes`
///   payload, so flipping to binary mid-stream never drops or reorders bytes.
/// - **Text mode** (`binary == false`): `carry ++ chunk` is split at the longest
///   valid-UTF-8 prefix. The valid prefix is delivered as a string; a genuinely
///   invalid byte in the *middle* is passed through `from_utf8_lossy` (→ U+FFFD),
///   exactly as the old per-chunk decode did; only an *incomplete trailing*
///   sequence — a valid multi-byte start whose continuation bytes haven't arrived
///   yet — is held back in `carry` for the next read. That carry is bounded to ≤3
///   bytes: a lone continuation or over-long lead byte is a hard error
///   (`error_len().is_some()`), not "incomplete", so it is emitted immediately
///   rather than accumulated (no unbounded-growth DoS from a stream of `0x80`s).
///
/// Returns `None` when a text-mode chunk contributed only continuation bytes to an
/// as-yet-incomplete character — nothing to deliver yet.
pub(crate) fn chunk_payload(carry: &mut Vec<u8>, chunk: &[u8], binary: bool) -> Option<Message> {
    if binary {
        if carry.is_empty() {
            return Some(Message::Bytes(SharedBlob::new(chunk)));
        }
        let mut v = std::mem::take(carry);
        v.extend_from_slice(chunk);
        return Some(Message::Bytes(SharedBlob::new(&v)));
    }
    let mut work = std::mem::take(carry);
    work.extend_from_slice(chunk);
    let valid = match std::str::from_utf8(&work) {
        Ok(_) => work.len(),
        // Incomplete trailing sequence → carry the tail, deliver the valid prefix.
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        // A genuine invalid sequence in the middle → deliver the whole chunk lossily
        // (as the previous per-chunk `from_utf8_lossy` did) and carry nothing.
        Err(_) => work.len(),
    };
    *carry = work.split_off(valid);
    (!work.is_empty()).then(|| Message::Str(String::from_utf8_lossy(&work).into_owned()))
}

/// Flush any bytes still held in `carry` at end-of-stream. In text mode a leftover
/// carry is a genuinely truncated final character (the stream ended mid-sequence),
/// so it is delivered lossily (→ U+FFFD). Binary mode never carries, so this is a
/// no-op there. Returns `None` when nothing is buffered.
pub(crate) fn chunk_flush(carry: &mut Vec<u8>) -> Option<Message> {
    (!carry.is_empty()).then(|| {
        let v = std::mem::take(carry);
        Message::Str(String::from_utf8_lossy(&v).into_owned())
    })
}

// ===== Direct cross-heap copy (L1) ===========================================
//
// The local-send fast path. `send` normally serialises the value into a
// heap-independent `Message` and the receiver rebuilds it — two full copies of the
// graph, with both intermediates becoming garbage. That shape is right for the *dist*
// wire (a peer node has its own heap and its own interner), but a local send to a
// **parked** receiver can copy straight from one heap into the other, which is what the
// BEAM does. Measured 2026-07-29: the round trip is ~580 ns of a ~1160 ns `[:tag v]`
// message, so removing one of the two copies is worth ~25% of send+receive.
//
// Two invariants make this sound, both verified in the code rather than assumed:
//
//  * **Exclusive access.** Only ever called with the receiver *parked*: `wake_parked`
//    hands the sender its `Box<Process>` under the mailbox state lock, so nothing else
//    can touch that heap for the duration. A *running* receiver falls back to `Message`.
//  * **No incremental rooting needed.** Allocation never collects in this runtime
//    (collection runs at eval safepoints), so children accumulated in an ordinary Rust
//    `Vec` while building a parent cannot go stale mid-copy. `from_message` relies on
//    exactly this.
//
// The result must then be parked in `Heap::msg_roots` — a *traced* slot table — and not
// on `roots`, which is the operand stack and is truncated on every frame pop.

/// Deep-copy `v` out of `src` and into `dst`, returning the equivalent value in `dst`.
/// `None` for anything the message path refuses (a rope, a macro, a builtin, an
/// unrealised lazy seq) or that is nested past [`MAX_MESSAGE_DEPTH`] — the caller then
/// falls back to the `Message` path, which produces the proper user-facing error.
pub(crate) fn copy_cross_heap(src: &Heap, dst: &mut Heap, v: Value) -> Option<Value> {
    copy_cross_heap_rec(src, dst, v, 0)
}

/// Whether a closure crossing a **same-runtime** local send is handed over as a shared
/// RUNTIME handle (the default) instead of being deep-copied into the receiver.
/// `BROOD_NO_SHARE_FN=1` reverts to the copy — the A/B and bisect lever, and the
/// stopgap if a shared handle is ever implicated in a fault. Read once and cached.
fn share_fn_enabled() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("BROOD_NO_SHARE_FN").is_none())
}

fn copy_cross_heap_rec(src: &Heap, dst: &mut Heap, v: Value, depth: u32) -> Option<Value> {
    if depth >= MAX_MESSAGE_DEPTH {
        return None;
    }
    Some(match v {
        // Atoms carry no heap identity — they are the same value in any heap.
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Sym(_)
        | Value::Keyword(_)
        | Value::Ref(_)
        | Value::Pid { .. } => v,
        // Registry ids (runtime-global, not per-heap handles) cross unchanged, exactly
        // as they do through `Message`.
        Value::Socket(_) | Value::Subprocess(_) | Value::Table(_) => v,
        Value::BigInt(id) => dst.int_from_bigint(src.bigint(id).clone()),
        Value::Decimal(id) => dst.alloc_decimal(src.decimal(id).clone()),
        Value::Ratio(id) => dst.alloc_ratio(src.ratio(id).clone()),
        // Byte blobs are `Arc`-shared by the message path too: an atomic bump, no copy.
        Value::Bytes(id) => dst.alloc_bytes(Arc::clone(&src.bytes(id))),
        Value::Str(id) => match src.local_shared_blob(id) {
            Some(blob) => dst.alloc_string_from_shared(blob),
            None => dst.alloc_string(&src.string(id)),
        },
        Value::Pair(_) => {
            let items = src.list_to_vec(v).ok()?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(copy_cross_heap_rec(src, dst, item, depth + 1)?);
            }
            dst.list(out)
        }
        Value::Vector(id) => {
            let items = src.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(copy_cross_heap_rec(src, dst, item, depth + 1)?);
            }
            dst.alloc_vector(out)
        }
        // A range stands in for the list of its elements, like `to_message`.
        Value::Range(id) => {
            // `None` here means "cannot be copied", which is exactly what an
            // un-realisable range is (see `range_to_vec`'s element cap).
            let items = src.range_to_vec(id).ok()?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(copy_cross_heap_rec(src, dst, item, depth + 1)?);
            }
            dst.list(out)
        }
        Value::Map(id) => {
            let entries = src.map_entries(id);
            let mut out = Vec::with_capacity(entries.len());
            for (k, val) in entries {
                out.push((
                    copy_cross_heap_rec(src, dst, k, depth + 1)?,
                    copy_cross_heap_rec(src, dst, val, depth + 1)?,
                ));
            }
            dst.map_from_pairs(out)
        }
        Value::Set(id) => {
            let elems = src.set_elems(id);
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(copy_cross_heap_rec(src, dst, e, depth + 1)?);
            }
            dst.set_from_elems(out)
        }
        // A closure that **already lives in the shared code region** crosses to a process
        // of the **same runtime** by handle, not by copy. Both processes read that region
        // through the same `Arc`, so the handle means the same thing in `dst` — this is
        // what `spawn` relies on for its thunk. Copying instead gives the receiver a
        // private duplicate of code its own runtime already has: measured at **436 bytes
        // against 48** for the same trivial thunk, ~670 objects each, which is what made a
        // supervisor retaining one `:start` closure per child spend two thirds of
        // `start-child` in GC (docs/runtime-frontier.md A3).
        //
        // **It deliberately does NOT promote a local closure to make it shareable**, even
        // though that would widen the win to inline `(fn () …)` specs. Promotion appends
        // to the append-only RUNTIME region, and a *transient* closure — sent, used,
        // dropped — then costs a whole aging/drain/free cycle to reclaim instead of dying
        // at the next minor GC. Measured over N sent-and-discarded closures, peak RSS went
        // 129 / 190 / 340 / 541 MB at N = 100k / 200k / 400k / 800k against a flat
        // 112–180 MB for the copy: growth proportional to closures sent, which is a leak
        // in any long-running receiver. Restricted to already-shared closures there is no
        // new promotion and no growth at all: at N=800k, 150 MB against the copy's 181 MB for
        // transient closures, and 121 vs 129 MB when the sent closure is already shared
        // (best-of-3 peak RSS — single runs of this vary by tens of MB).
        // Widening this needs the RUNTIME collector to reclaim promptly (ADR-091 stage 4),
        // not a looser rule here.
        //
        // What this covers in practice: a closure that captures **no local variables**
        // (it refers only to globals) is already a RUNTIME-region value, so the idiomatic
        // supervisor spec `(fn () (spawn-link (worker)))` is handed over by handle —
        // measured 6 µs against 54 µs per send for the same shape capturing a local.
        // A closure that *does* capture a local is a LOCAL value and takes the copy, which
        // is correct and unchanged; it simply does not get the win.
        //
        // Not covered yet: a **PRELUDE**-region closure (sending `map` itself, say) still
        // copies. It is arguably safer than the RUNTIME case — the prelude is immutable and
        // never collected — but it needs its own guard (the prelude is a second `Arc`, and
        // `shares_runtime_with` only compares the runtime one) and its own validation run,
        // so it is left as a follow-up rather than folded in untested.
        //
        // Guards, all load-bearing:
        //  - `region() == RUNTIME` — the no-new-promotion rule above.
        //  - `shares_runtime_with` — the process REGISTRY is global, so a second `Interp`
        //    in the same OS process has a *different* region; its handles must not cross.
        //    Cross-**node** sends never reach here at all (they serialise via `Message`).
        //  - `share_fn_enabled` — `BROOD_NO_SHARE_FN=1` reverts to the copy, so this is
        //    A/B-able and instantly revertible without a rebuild.
        //  - anything else declines (`None`) to the existing `Message` path unchanged.
        //
        // Lifetime: a shared handle retained in `dst`'s LOCAL data pins its RUNTIME
        // generation, and that is *sound* — the drain's Phase 2 walks the whole local
        // heap, so `runtime_gen_referenced` sees it and refuses to free underneath us;
        // aging never moves handles, and compaction requires unique ownership. It costs
        // reclamation latency for a superseded version whose only remaining reference is
        // a receiver's retained handle — bounded by that value's lifetime.
        Value::Fn(id)
            if share_fn_enabled()
                && id.region() == crate::core::value::RUNTIME
                && src.shares_runtime_with(dst) =>
        {
            v
        }
        // Everything else — a closure we declined to share, ropes, macros, builtins,
        // unrealised seq-views — takes the `Message` path, so its existing semantics and
        // error messages are unchanged.
        _ => return None,
    })
}

#[cfg(test)]
mod chunk_tests {
    use super::{chunk_flush, chunk_payload, Message};

    /// Pull the delivered string out of a text-mode payload.
    fn text(m: Option<Message>) -> Option<String> {
        m.map(|m| match m {
            Message::Str(s) => s,
            _ => panic!("expected a Str payload"),
        })
    }

    /// Pull the delivered bytes out of a binary-mode payload.
    fn raw(m: Option<Message>) -> Vec<u8> {
        match m {
            Some(Message::Bytes(b)) => b.as_bytes().to_vec(),
            _ => panic!("expected a Bytes payload"),
        }
    }

    #[test]
    fn ascii_passes_straight_through() {
        let mut carry = Vec::new();
        assert_eq!(
            text(chunk_payload(&mut carry, b"hello", false)).as_deref(),
            Some("hello")
        );
        assert!(carry.is_empty());
    }

    #[test]
    fn a_multibyte_char_split_across_two_reads_is_reassembled() {
        // "é" is 0xC3 0xA9; split it across the chunk boundary.
        let mut carry = Vec::new();
        // First read ends mid-character: deliver the valid prefix, carry the tail.
        assert_eq!(
            text(chunk_payload(&mut carry, b"ab\xc3", false)).as_deref(),
            Some("ab")
        );
        assert_eq!(carry, vec![0xc3]);
        // Second read completes it: no U+FFFD anywhere.
        assert_eq!(
            text(chunk_payload(&mut carry, b"\xa9cd", false)).as_deref(),
            Some("écd")
        );
        assert!(carry.is_empty());
    }

    #[test]
    fn a_4byte_emoji_dribbled_one_byte_at_a_time_survives() {
        // "🦀" (U+1F980) = F0 9F A6 80.
        let mut carry = Vec::new();
        assert_eq!(text(chunk_payload(&mut carry, b"\xf0", false)), None);
        assert_eq!(text(chunk_payload(&mut carry, b"\x9f", false)), None);
        assert_eq!(text(chunk_payload(&mut carry, b"\xa6", false)), None);
        assert_eq!(carry.len(), 3); // carry never exceeds 3 bytes
        assert_eq!(
            text(chunk_payload(&mut carry, b"\x80", false)).as_deref(),
            Some("🦀")
        );
        assert!(carry.is_empty());
    }

    #[test]
    fn a_genuinely_invalid_byte_is_lossy_now_not_carried_forever() {
        // 0xFF is never a valid UTF-8 byte: replace it immediately, don't accumulate.
        let mut carry = Vec::new();
        assert_eq!(
            text(chunk_payload(&mut carry, b"a\xffb", false)).as_deref(),
            Some("a\u{fffd}b")
        );
        assert!(carry.is_empty());
    }

    #[test]
    fn binary_mode_is_byte_faithful_and_flushes_a_text_carry() {
        let mut carry = vec![0xc3]; // a partial char left over from text mode
                                    // Flipping to binary flushes the carry ahead of the new bytes, verbatim.
        assert_eq!(
            raw(chunk_payload(&mut carry, &[0x28, 0xff], true)),
            vec![0xc3, 0x28, 0xff]
        );
        assert!(carry.is_empty());
    }

    #[test]
    fn flush_delivers_a_truncated_final_char_lossily() {
        let mut carry = vec![0xf0, 0x9f]; // stream ended mid-emoji
        assert_eq!(text(chunk_flush(&mut carry)).as_deref(), Some("\u{fffd}"));
        assert!(carry.is_empty());
        assert!(chunk_flush(&mut carry).is_none()); // nothing left
    }
}
