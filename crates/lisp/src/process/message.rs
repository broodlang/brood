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
    /// A **failure** value — its fields, mirroring `Map` (they share one CHAMP store).
    Failure(Vec<(Message, Message)>),
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
    /// builtin — the image is binary-keyed (system/build-id in its fingerprint), so the name is
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
    /// Modules the receiving runtime must load before this closure's body can run
    /// (KI-55). Auto-require (ADR-227/229) fires on the node that **compiles** a form;
    /// a shipped closure arrives already expanded and resolved, so nothing on the
    /// receiver triggers the load and a body calling `reflect/form-pos` dies with a bare
    /// `unbound symbol` far from the cause. The sender — which *does* have the module
    /// loaded, or the reference would not resolve for it either — names them here, and
    /// the receiver weaves a load for each one it lacks into the rebuilt body (see
    /// [`guard_form`]). Almost always empty (a bare-name body) or one element. See
    /// [`ModuleNeed`].
    pub(crate) modules: Vec<ModuleNeed>,
}

/// One module a shipped closure's body needs on the receiver (KI-55).
///
/// Two symbols, because the receiver has two different jobs: `probe` answers "is this
/// already satisfied here?" with a single allocation-free global lookup (it is one
/// qualified name the body actually references, so it is bound exactly when the module
/// is loaded), and `module` is what gets `require-one`d when it is not. Deriving the
/// module on the *sender* also keeps the receiver from having to re-resolve an alias or
/// an intra-package root against its own tables.
#[derive(Clone)]
pub struct ModuleNeed {
    pub(crate) module: Symbol,
    pub(crate) probe: Symbol,
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
        // A failure crosses as its fields — the map path verbatim, re-wrapped so the
        // receiver gets a failure rather than a plain map.
        Value::Failure(id) => {
            let entries = heap.map_entries(id);
            let mut out = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                out.push((
                    to_message_rec(heap, k, visited, depth + 1, dest_runtime)?,
                    to_message_rec(heap, v, visited, depth + 1, dest_runtime)?,
                ));
            }
            Message::Failure(out)
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

    // Which modules the receiver must load first (KI-55). Skipped entirely for a
    // destination inside THIS runtime — its processes share our globals, so every module
    // this body references is loaded there by construction — and for the startup image,
    // which restores into the runtime it was written from. What is left is exactly the
    // case auto-require cannot reach: a closure crossing to another runtime (a node, or a
    // second `Interp` in this process), where the receiver never compiles the body and so
    // never infers its imports.
    let modules =
        if dest_runtime == Some(heap.runtime_tag()) || IMAGE_NATIVE_BY_NAME.with(|f| f.get()) {
            Vec::new()
        } else {
            let mut candidates = Vec::new();
            for arm in cl.arms.iter() {
                for &form in &arm.body {
                    collect_qualified_syms(heap, form, &mut candidates);
                }
                for &(_, d) in &arm.optionals {
                    collect_qualified_syms(heap, d, &mut candidates);
                }
            }
            if candidates.is_empty() {
                Vec::new()
            } else {
                resolve_module_needs(heap, &candidates)
            }
        };

    visited.pop();
    Ok(ClosureMsg {
        name: cl.name,
        arms,
        doc: cl.doc.clone(),
        captured,
        modules,
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
        Value::Map(id) | Value::Failure(id) => {
            for (k, v) in heap.map_entries(id) {
                collect_symbols(heap, k, out);
                collect_symbols(heap, v, out);
            }
        }
        _ => {}
    }
}

/// Is `name` a module name we are willing to hand to the loader? (KI-55.)
///
/// The list rides in from another node, and `require-one` resolves a module name to a
/// **file path**. Cross-node closure shipping is authenticated (a cookie holder already
/// has remote eval by design), so triggering a load is inside the threat model — but a
/// name that can climb out of the module roots is not, so this admits only what a real
/// module name looks like: slash-separated segments of `[A-Za-z0-9_.+*<>=!?-]`, no empty
/// segment, no `.`/`..` segment, no leading/trailing slash, and a length cap.
fn safe_module_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 200 {
        return false;
    }
    name.split('/').all(|seg| {
        !seg.is_empty()
            && seg != "."
            && seg != ".."
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.+*<>=!?-".contains(&b))
    })
}

/// Collect every **qualified symbol** that appears in the closure-body form `form` as
/// code, into `out` (KI-55) — the candidates [`resolve_module_needs`] then turns into the
/// module list the receiver must load.
///
/// Two things this walk deliberately does *not* do, and one it does:
///   - a `(quote …)` / `(quasiquote …)` subtree is data, not code, so it is skipped —
///     `'json/x` must not drag `json` onto the receiver;
///   - it resolves nothing: the only work per symbol is a name lookup, a `/` scan and a
///     dedup against a handful of candidates, so the pass stays proportional to the
///     deep-copy this rides alongside;
///   - it dedups by symbol, so a body naming `math/sqrt` five times resolves it once.
///
/// The spine walk is iterative so a long body list can't overflow the stack.
fn collect_qualified_syms(heap: &Heap, form: Value, out: &mut Vec<Symbol>) {
    match form {
        Value::Sym(s) => {
            if value::symbol_name_ref(s).contains('/') && !out.contains(&s) {
                out.push(s);
            }
        }
        Value::Pair(_) => {
            // `(quote x)` / `(quasiquote x)` is data — skip the whole subtree.
            if let Value::Pair(id) = form {
                if let Value::Sym(h) = heap.pair(id).0 {
                    if value::symbol_is(h, crate::core::keywords::QUOTE)
                        || value::symbol_is(h, crate::core::keywords::QUASIQUOTE)
                    {
                        return;
                    }
                }
            }
            let mut cur = form;
            while let Value::Pair(id) = cur {
                let (car, cdr) = heap.pair(id);
                collect_qualified_syms(heap, car, out);
                cur = cdr;
            }
            collect_qualified_syms(heap, cur, out);
        }
        Value::Vector(id) => {
            for item in heap.vector(id).to_vec() {
                collect_qualified_syms(heap, item, out);
            }
        }
        Value::Map(id) | Value::Failure(id) => {
            for (k, v) in heap.map_entries(id) {
                collect_qualified_syms(heap, k, out);
                collect_qualified_syms(heap, v, out);
            }
        }
        Value::Set(id) => {
            for e in heap.set_elems(id) {
                collect_qualified_syms(heap, e, out);
            }
        }
        _ => {}
    }
}

/// Turn the qualified symbols a closure body mentions into the modules the receiver has to
/// load (KI-55), deduped by module — normally zero or one entry.
///
/// A candidate is kept only when it is **bound as a global here**, which is what tells a
/// genuine reference (auto-require loaded its module when this body was compiled) from a
/// qualified-looking symbol that merely sits in the body. That filter is also what makes an
/// unresolvable module on the receiver a real error rather than a false alarm: we only ever
/// name modules we ourselves have. This is the resolving half, run once per *distinct*
/// symbol — `module_to_require` interns and consults the import table, so it must not sit
/// inside the walk.
fn resolve_module_needs(heap: &Heap, candidates: &[Symbol]) -> Vec<ModuleNeed> {
    let mut out: Vec<ModuleNeed> = Vec::new();
    for &s in candidates {
        if heap.env_get(EnvId::GLOBAL, s).is_none() {
            continue; // not a resolved reference here — nothing to ask the receiver for
        }
        let Some(module) = crate::eval::derive::module_to_require(heap, s) else {
            continue; // alias prefix, root-escape, or the bare `/` operator
        };
        if !safe_module_name(value::symbol_name_ref(module)) {
            continue;
        }
        if out.iter().any(|n| n.module == module) {
            continue;
        }
        out.push(ModuleNeed { module, probe: s });
    }
    out
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

/// Whether rebuilding `m` into a heap is small enough to do **with the mailbox mutex
/// held** — the receive-side counterpart of [`l1_copy_budget`]'s question on the send side
/// (KI-56, ADR-245).
///
/// A selective-receive scan that has skipped the head cannot pop its candidate (it may not
/// match, and has to stay queued), so it rebuilds *in place*, under the lock, once per
/// candidate. That is the same unbounded hold the send side had. The scan asks this first
/// and, for anything too big, pops the candidate and rebuilds with the mutex released —
/// which is exactly what the optimistic branch beside it already does.
///
/// The probe is a bounded, **allocation-free** walk of the wire tree that stops the moment
/// the count clears `budget`, so it is O(min(n, budget)) and cannot itself become the
/// stall it exists to prevent.
pub(crate) fn message_fits(m: &Message, budget: i64) -> bool {
    fn walk(m: &Message, left: &mut i64) {
        if *left < 0 {
            return;
        }
        *left -= 1;
        match m {
            Message::List(items, _) | Message::Vector(items) | Message::Set(items) => {
                for it in items {
                    walk(it, left);
                    if *left < 0 {
                        return;
                    }
                }
            }
            Message::Map(entries) | Message::Failure(entries) => {
                for (k, v) in entries {
                    walk(k, left);
                    walk(v, left);
                    if *left < 0 {
                        return;
                    }
                }
            }
            // Unlike the send side, a `Str` here IS worth charging for. `to_message` routes
            // anything at or above `SHARED_BLOB_THRESHOLD` to `StrShared` (an `Arc` bump on
            // rebuild, free) — but a `Message` does not only come from `to_message`: one
            // decoded from a remote node's wire frame carries whatever that encoder chose,
            // so an inline `Str` of any length can arrive here and `alloc_string` memcpys it.
            Message::Str(s) => *left -= (s.len() / 1024) as i64,
            // Rebuilding a shipped closure reconstructs *code* — arms, captured env, and
            // (ADR-245's sibling, KI-55) the woven module guards. Never under the lock.
            Message::Closure(_) => *left = -1,
            // Everything else is one node: an atom, or an `Arc` handed over without a copy.
            _ => {}
        }
    }
    let mut left = budget;
    walk(m, &mut left);
    left >= 0
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
            // (system/build-id in its fingerprint), so the same primitive is registered under this name
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
        Message::Failure(entries) => {
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                pairs.push((from_message(heap, k), from_message(heap, v)));
            }
            match heap.map_from_pairs(pairs) {
                Value::Map(id) => Value::failure(id),
                other => other,
            }
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

/// Wrap `form` so the modules in `missing` are loaded **before it runs** (KI-55), or hand
/// `form` back untouched when there are none — the overwhelmingly common case, and one
/// that allocates nothing.
///
/// The load is woven into the closure's own body rather than run here, at rebuild time,
/// for two reasons and they are both hard:
///
///   1. **`from_message` is not a place where Brood code may run.** It rebuilds a value
///      graph whose half-built lists/vectors/maps live in unrooted Rust locals, and the
///      selective-`receive` scan calls it **while holding the mailbox lock** (the
///      peek-in-place branch of `mailbox::scan`). A module load evaluates arbitrary
///      top-level code — it can collect, and `require-one` can `sleep` waiting out another
///      process's in-flight load of the same feature, which under that lock is a deadlock.
///   2. **A call site is where the error belongs.** Woven in, a module this node cannot
///      load raises an ordinary Brood error naming the module, the reference, and the fact
///      that the closure was shipped — from the call that needed it, instead of a bare
///      `unbound symbol` at whatever line happens to touch the name first.
///
/// The emitted guard is built from **primitives and core special forms only** (`if`, `do`,
/// `quote`, `fn`, `%try`) — a rebuilt body is never macroexpanded on the receiver, so
/// `unless`/`try` would survive to the compiler as unexpanded calls:
///
/// ```text
/// (do (if (bound? 'math/sqrt)
///       nil
///       (%try (fn () (require-one 'math))
///             (fn (e) (throw (str "…needs module `math`…: " (if (map? e) (get e :message) e))))))
///     <form>)
/// ```
///
/// The `bound?` test makes it idempotent and self-retiring: once the module is loaded the
/// guard is one primitive call per invocation, and a closure whose modules were already
/// present here never got a guard in the first place.
fn guard_form(heap: &mut Heap, missing: &[&ModuleNeed], form: Value) -> Value {
    if missing.is_empty() {
        return form;
    }
    let sym = |s: &str| Value::symbol(value::intern(s));
    let mut out: Vec<Value> = Vec::with_capacity(missing.len() + 2);
    out.push(sym("do"));
    for need in missing {
        let module = value::symbol_name_ref(need.module);
        let probe = value::symbol_name_ref(need.probe);
        if !safe_module_name(module) {
            // The list arrived over a socket and a module name drives a filesystem path.
            // Refuse to emit a load for it at all; the reference then fails as an ordinary
            // unbound symbol, with this line to say why.
            eprintln!(
                "[shipped-closure] refusing to load module `{module}` (referenced as \
                 `{probe}` by a closure shipped from another runtime): not a valid module name"
            );
            continue;
        }
        // (fn () (require-one 'module))
        let q = heap.list(vec![sym("quote"), Value::symbol(need.module)]);
        let load = heap.list(vec![sym("require-one"), q]);
        let load_thunk = heap.list(vec![sym("fn"), Value::nil(), load]);
        // (fn (e) (throw (str "…" (get e :message))))
        let msg = heap.alloc_string(&format!(
            "this closure was shipped from another runtime and needs module `{module}` \
             (its body references `{probe}`), which this node cannot load: "
        ));
        // `(if (map? e) (get e :message) e)` — a structured error is a map, but a plain
        // `(throw "…")` (which is what a missing module raises) binds the string itself.
        let get_msg = heap.list(vec![
            sym("get"),
            sym("e"),
            Value::keyword(value::intern("message")),
        ]);
        let is_map = heap.list(vec![sym("map?"), sym("e")]);
        let reason = heap.list(vec![sym("if"), is_map, get_msg, sym("e")]);
        let text = heap.list(vec![sym("str"), msg, reason]);
        let throw = heap.list(vec![sym("throw"), text]);
        let params = heap.list(vec![sym("e")]);
        let handler = heap.list(vec![sym("fn"), params, throw]);
        let try_load = heap.list(vec![sym("%try"), load_thunk, handler]);
        // (if (bound? 'probe) nil <try_load>)
        let q = heap.list(vec![sym("quote"), Value::symbol(need.probe)]);
        let bound = heap.list(vec![sym("bound?"), q]);
        let guard = heap.list(vec![sym("if"), bound, Value::nil(), try_load]);
        out.push(guard);
    }
    if out.len() == 1 {
        return form; // every need was refused — nothing to guard with
    }
    out.push(form);
    heap.list(out)
}

/// Rebuild a serialised closure into `heap`. Body/optional-default forms are
/// reconstructed as local data; captured frames are recreated (outermost first)
/// and chained onto this process's global scope, so the closure's free globals
/// resolve here. The result is a fresh, independent copy — a later redefinition
/// of *this* function won't reach it, but globals it *references* still do.
fn closure_from_message(heap: &mut Heap, c: &ClosureMsg) -> Value {
    // The modules this body needs that this runtime does NOT already have (KI-55). The
    // probe check is one global lookup and no allocation, so a closure arriving into a
    // runtime that already has its modules — every same-runtime send, every table read,
    // and any node that has loaded them once — costs exactly that and nothing else.
    let missing: Vec<&ModuleNeed> = c
        .modules
        .iter()
        .filter(|need| heap.env_get(EnvId::GLOBAL, need.probe).is_none())
        .collect();
    // Rebuild every arm's optional-default forms and body as local data.
    let arms = c
        .arms
        .iter()
        .map(|arm| {
            let optionals = arm
                .optionals
                .iter()
                .map(|(s, d)| {
                    let d = from_message(heap, d);
                    // A default is evaluated at frame setup, *before* the body — so a
                    // module a default needs has to be loaded ahead of it too.
                    (*s, guard_form(heap, &missing, d))
                })
                .collect();
            let mut body: Vec<Value> = arm.body.iter().map(|f| from_message(heap, f)).collect();
            if let Some(first) = body.first().copied() {
                body[0] = guard_form(heap, &missing, first);
            }
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
/// Bounded: `budget` (start it at [`l1_copy_budget`]) is decremented as the copy walks and
/// the whole thing declines (`None`) the moment it goes negative. The caller reads `budget`
/// afterwards to tell *why* it declined — a negative value means the size cap, anything
/// else means a value kind the copier does not handle.
///
/// **Why a cap at all** (KI-56). This copy runs with the receiver's mailbox mutex held —
/// that is not incidental, it is what makes the fast path sound: the lock is what gives us
/// the parked receiver's `Box<Process>` and therefore exclusive `&mut` on its heap. So the
/// lock hold is proportional to the message, and a *large* message stalls every unrelated
/// operation on that mailbox. Measured with an unrelated `%mailbox-size` probe (a pure
/// lock-acquire, zero message work, so a stall can only be lock wait) against a parked
/// receiver in synchronous request/reply:
///
/// | payload | L1 p50 | L1 p99 | wire p50 | wire p99 |
/// |---|---|---|---|---|
/// | ~8 KB | 4.8 µs | 11 µs | 4.4 µs | 7.5 µs |
/// | ~80 KB | 4.9 µs | 1 106 µs | 4.9 µs | 8.7 µs |
/// | ~1.6 MB | 5 011 µs | 13 994 µs | 7.3 µs | 15.0 µs |
///
/// The wire arm is flat across a 500× payload range because its heavy work happens
/// *outside* the lock. So the fix is not to move this copy out of the lock — that was
/// tried and is unsound, since `shutdown_runtime_parked` reaps parked waiters and would
/// skip a process during the window — but to **decline the large ones** and let them take
/// the wire path that already handles them well. The `st.waiter` invariant is untouched.
pub(crate) fn copy_cross_heap(
    src: &Heap,
    dst: &mut Heap,
    v: Value,
    budget: &mut i64,
) -> Option<Value> {
    copy_cross_heap_rec(src, dst, v, 0, budget)
}

/// The copy-work budget for one L1 delivery, in units: **one heap node is one unit**, and
/// a copied string charges one more per [`STR_BYTES_PER_UNIT`] of payload (a string is a
/// single node but a `memcpy`, and a byte moved is not free just because it is not a
/// pointer chase).
///
/// The default is chosen from the measurement above rather than from a round number: the
/// probe shows nothing at ~8 KB and a p99 blow-up by ~80 KB, so the cap sits between them.
/// Below it every ordinary message — a request, a reply, a keyword-tagged tuple, a
/// record — keeps the fast path; above it the send is one of the rare multi-KB-to-MB
/// payloads that was doing the stalling.
///
/// `BROOD_L1_BUDGET=<units>` overrides it, and **`0` means unlimited** — the A/B lever,
/// the bisect switch, and the way to restore the pre-cap behaviour without a rebuild.
const L1_COPY_BUDGET: i64 = 4096;

/// The budget counts **nodes**, which bounds the copy only because no single node can carry
/// an unbounded payload. Strings are the one kind that could: a string of
/// [`SHARED_BLOB_THRESHOLD`] bytes or more is an `Arc<SharedBlob>` and crosses by handle
/// (no payload copy), so only a *sub-threshold* string is ever memcpy'd, and its cost is
/// bounded by that threshold.
///
/// Raise the threshold far enough and that stops being true — a node-counting budget would
/// then let a large string copy under the lock, which is exactly the stall KI-56 measured.
/// This fails the build instead.
const _: () = assert!(
    crate::core::blob::SHARED_BLOB_THRESHOLD <= 4096,
    "a node-counting L1 budget assumes an inline string is small; \
     raise SHARED_BLOB_THRESHOLD and the budget must charge string payload again"
);

/// The configured budget, or [`i64::MAX`] when disabled. Read once and cached.
pub(crate) fn l1_copy_budget() -> i64 {
    static B: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *B.get_or_init(|| parse_l1_budget(std::env::var("BROOD_L1_BUDGET").ok().as_deref()))
}

/// `BROOD_L1_BUDGET`'s meaning, split out so it is testable without a process-global env
/// read. Unset keeps the default; `0` means unlimited; **anything unparseable also keeps
/// the default** — a typo must not silently uncap the lock hold, which is the failure this
/// whole mechanism exists to prevent.
fn parse_l1_budget(raw: Option<&str>) -> i64 {
    match raw.map(str::trim) {
        None => L1_COPY_BUDGET,
        Some("0") => i64::MAX,
        Some(s) => match s.parse::<i64>() {
            Ok(n) if n > 0 => n,
            _ => L1_COPY_BUDGET,
        },
    }
}

/// Whether a closure crossing a **same-runtime** local send is handed over as a shared
/// RUNTIME handle (the default) instead of being deep-copied into the receiver.
/// `BROOD_NO_SHARE_FN=1` reverts to the copy — the A/B and bisect lever, and the
/// stopgap if a shared handle is ever implicated in a fault. Read once and cached.
fn share_fn_enabled() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("BROOD_NO_SHARE_FN").is_none())
}

/// Decline because the message exceeds the copy budget, leaving the budget **negative**
/// so the caller can tell this apart from a value kind the copier does not handle. The
/// per-node charge reaches the same state by simply running out; the early-outs, which
/// return before spending anything, have to say so explicitly.
fn over_budget(budget: &mut i64) -> Option<Value> {
    *budget = -1;
    None
}

fn copy_cross_heap_rec(
    src: &Heap,
    dst: &mut Heap,
    v: Value,
    depth: u32,
    budget: &mut i64,
) -> Option<Value> {
    if depth >= MAX_MESSAGE_DEPTH {
        return None;
    }
    // One unit per node visited. Checked *before* the copy so an over-budget walk stops
    // adding work rather than finishing the node it is on.
    *budget -= 1;
    if *budget < 0 {
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
        // Neither arm needs charging beyond its one node, and that is a fact about
        // `SHARED_BLOB_THRESHOLD` rather than a guess: a string big enough to be worth
        // charging for is an `Arc<SharedBlob>` and crosses by handle (an atomic bump, no
        // payload copy at all), and one small enough to be inline is bounded by that
        // threshold. The `const _` below is what keeps that true.
        Value::Str(id) => match src.local_shared_blob(id) {
            Some(blob) => dst.alloc_string_from_shared(blob),
            None => dst.alloc_string(&src.string(id)),
        },
        Value::Pair(_) => {
            // Walk the spine ourselves, charging per cons cell, instead of calling
            // `list_to_vec`: that walks and allocates the WHOLE spine before any
            // per-element check could fire, so a huge list would pay its full O(n) cost
            // under the mailbox lock and only then be declined. A cons cell is a real
            // heap node, so charging one unit for it is the same accounting as anywhere
            // else. An improper list declines exactly as it did before.
            let mut items = Vec::new();
            let mut cur = v;
            loop {
                match cur.unpack() {
                    crate::core::value::ValueRef::Nil => break,
                    crate::core::value::ValueRef::Pair(p) => {
                        *budget -= 1;
                        if *budget < 0 {
                            return None;
                        }
                        let (head, tail) = src.pair(p);
                        items.push(head);
                        cur = tail;
                    }
                    _ => return None, // improper list — the `Message` path's error
                }
            }
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(copy_cross_heap_rec(src, dst, item, depth + 1, budget)?);
            }
            dst.list(out)
        }
        Value::Vector(id) => {
            // Early-out before materialising. `to_vec` copies the whole element array, so
            // a vector that cannot possibly fit would otherwise pay its full O(n) cost
            // under the lock and only then decline — which is the stall this budget
            // exists to bound. Checked against the budget, not charged to it: the
            // elements are charged as the walk actually visits them.
            if src.vector(id).len() as i64 > *budget {
                return over_budget(budget);
            }
            let items = src.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(copy_cross_heap_rec(src, dst, item, depth + 1, budget)?);
            }
            dst.alloc_vector(out)
        }
        // A range stands in for the list of its elements, like `to_message`.
        Value::Range(id) => {
            // `None` here means "cannot be copied", which is exactly what an
            // un-realisable range is (see `range_to_vec`'s element cap) — and now also
            // what an over-budget one is, checked before it is realised.
            if src.range_len(id) > *budget {
                return over_budget(budget);
            }
            let items = src.range_to_vec(id).ok()?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(copy_cross_heap_rec(src, dst, item, depth + 1, budget)?);
            }
            dst.list(out)
        }
        Value::Map(id) => {
            // Two nodes per entry (key and value), and `map_size` is O(1) — so the
            // early-out costs nothing and skips materialising a large map's entry list.
            if (src.map_size(id) as i64).saturating_mul(2) > *budget {
                return over_budget(budget);
            }
            let entries = src.map_entries(id);
            let mut out = Vec::with_capacity(entries.len());
            for (k, val) in entries {
                out.push((
                    copy_cross_heap_rec(src, dst, k, depth + 1, budget)?,
                    copy_cross_heap_rec(src, dst, val, depth + 1, budget)?,
                ));
            }
            dst.map_from_pairs(out)
        }
        Value::Set(id) => {
            if src.map_size(id) as i64 > *budget {
                return over_budget(budget);
            }
            let elems = src.set_elems(id);
            let mut out = Vec::with_capacity(elems.len());
            for e in elems {
                out.push(copy_cross_heap_rec(src, dst, e, depth + 1, budget)?);
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

#[cfg(test)]
mod shipped_module_tests {
    use super::safe_module_name;

    /// What a real module name looks like — the curated stdlib, an intra-package
    /// module, and the punctuation Brood allows in a name.
    #[test]
    fn real_module_names_are_accepted() {
        for name in [
            "math",
            "reflect",
            "json",
            "editor/buffer",
            "std/tool/project",
            "my-app/http-client",
            "a1_b2",
            "vec+",
            "ok?",
        ] {
            assert!(safe_module_name(name), "should accept `{name}`");
        }
    }

    /// The list rides in over a socket and a module name resolves to a FILE PATH, so
    /// anything that could climb out of the module roots — or is simply not a name —
    /// is refused before the loader ever sees it. Authenticated peers get remote eval
    /// by design; that is not a reason to hand them a path.
    #[test]
    fn path_escapes_and_junk_are_refused() {
        for name in [
            "",
            "..",
            "../etc/passwd",
            "a/../../b",
            "/etc/passwd",
            "a//b",
            "a/",
            "with space",
            "quote'd",
            "semi;colon",
            "tilde~",
            "back\\slash",
            "new\nline",
            "nul\0byte",
        ] {
            assert!(!safe_module_name(name), "should refuse `{name}`");
        }
        // And a name no sane module has, so a hostile peer can't drive a huge lookup.
        assert!(!safe_module_name(&"a".repeat(201)));
    }
}

/// The L1 copy budget (KI-56): the cap that keeps a large local send from holding the
/// receiver's mailbox mutex for the length of a deep copy.
///
/// These call [`copy_cross_heap`] with an explicit budget rather than the env-derived one,
/// so they pin the *mechanism* and stay deterministic whatever `BROOD_L1_BUDGET` says.
#[cfg(test)]
mod copy_budget_tests {
    use super::{copy_cross_heap, parse_l1_budget, L1_COPY_BUDGET};
    use crate::core::blob::SHARED_BLOB_THRESHOLD;
    use crate::core::heap::Heap;
    use crate::core::value::Value;

    /// A vector of `n` ints in a fresh heap, plus that heap.
    fn heap_with_vector(n: usize) -> (Heap, Value) {
        let mut h = Heap::new();
        let items: Vec<Value> = (0..n as i64).map(Value::Int).collect();
        let v = h.alloc_vector(items);
        (h, v)
    }

    /// The ordinary case: a small message copies, and copies *correctly*. A budget that
    /// declined everything would pass a "does it stall" test and fail this one.
    #[test]
    fn a_small_value_copies_within_budget_and_round_trips() {
        let (src, v) = heap_with_vector(8);
        let mut dst = Heap::new();
        let mut budget = L1_COPY_BUDGET;
        let copied = copy_cross_heap(&src, &mut dst, v, &mut budget).expect("should copy");
        let Value::Vector(id) = copied else {
            panic!("expected a vector, got {copied:?}")
        };
        let got: Vec<i64> = dst
            .vector(id)
            .iter()
            .map(|e| match e {
                Value::Int(i) => *i,
                other => panic!("expected an int, got {other:?}"),
            })
            .collect();
        assert_eq!(got, (0..8).collect::<Vec<i64>>());
        assert!(budget > 0, "a tiny message must not exhaust the budget");
    }

    /// The cap itself. A message past the budget declines, and the *sign of the budget*
    /// is what tells the caller it was the cap rather than an uncopyable value kind —
    /// `try_deliver_local` reads exactly this to bump the right counter.
    #[test]
    fn an_oversized_value_declines_with_a_negative_budget() {
        let (src, v) = heap_with_vector(10_000);
        let mut dst = Heap::new();
        let mut budget = 128;
        assert!(copy_cross_heap(&src, &mut dst, v, &mut budget).is_none());
        assert!(budget < 0, "the decline must be attributable to the cap");
    }

    /// Every container kind has to decline *before* materialising, and every one has to
    /// stay attributable while doing it. A vector, a list, a map and a set reach that by
    /// four different routes — an O(1) length check, a bounded spine walk, `map_size`
    /// doubled for key+value, and `map_size` — so each is pinned separately.
    #[test]
    fn every_container_kind_declines_attributably() {
        let mut src = Heap::new();
        let items: Vec<Value> = (0..5_000i64).map(Value::Int).collect();
        let vector = src.alloc_vector(items.clone());
        let list = src.list(items.clone());
        let map = src.map_from_pairs(items.iter().map(|k| (*k, *k)).collect());
        let set = src.set_from_elems(items);

        for (what, v) in [
            ("vector", vector),
            ("list", list),
            ("map", map),
            ("set", set),
        ] {
            let mut dst = Heap::new();
            let mut budget = 64;
            assert!(
                copy_cross_heap(&src, &mut dst, v, &mut budget).is_none(),
                "{what} should decline"
            );
            assert!(
                budget < 0,
                "{what}'s decline must be attributable to the cap"
            );
        }
    }

    /// The boundary, both sides of it. `n` elements inside a vector cost `n + 1` units
    /// (the vector node itself), so this also pins that the container is charged.
    #[test]
    fn the_boundary_is_exact() {
        let (src, v) = heap_with_vector(100);
        let mut dst = Heap::new();

        let mut exact = 101;
        assert!(
            copy_cross_heap(&src, &mut dst, v, &mut exact).is_some(),
            "101 units must cover a 100-element vector plus its own node"
        );

        let mut one_short = 100;
        assert!(copy_cross_heap(&src, &mut dst, v, &mut one_short).is_none());
    }

    /// Why counting nodes is enough: a big string is not copied at all. At or above
    /// `SHARED_BLOB_THRESHOLD` it is an `Arc<SharedBlob>` and crosses by handle, so a
    /// 1 MB string costs the same one unit as a short one — and the lock is held for an
    /// atomic increment, not a megabyte of `memcpy`.
    ///
    /// This is the test that fails if that coupling is ever broken (alongside the
    /// `const _` assertion, which catches the threshold moving).
    #[test]
    fn a_large_string_crosses_by_handle_and_costs_one_unit() {
        let mut src = Heap::new();
        let v = src.alloc_string(&"x".repeat(1024 * 1024));
        let mut dst = Heap::new();
        let mut budget = 1;
        assert!(
            copy_cross_heap(&src, &mut dst, v, &mut budget).is_some(),
            "a shared-blob string must not be charged for its payload"
        );
        assert_eq!(budget, 0);
    }

    /// The other side of the same coupling: a string below the threshold *is* copied,
    /// but the threshold is what bounds how much that can cost.
    #[test]
    fn an_inline_string_is_bounded_by_the_blob_threshold() {
        let mut src = Heap::new();
        let v = src.alloc_string(&"x".repeat(SHARED_BLOB_THRESHOLD - 1));
        let mut dst = Heap::new();
        let mut budget = 1;
        assert!(copy_cross_heap(&src, &mut dst, v, &mut budget).is_some());
        // That it *stays* small is the `const _` assertion beside `L1_COPY_BUDGET`, which
        // fails the build rather than a test.
    }

    /// `BROOD_L1_BUDGET`'s three rules. The last one is the important one: a typo must
    /// not silently uncap the lock hold.
    #[test]
    fn the_env_override_reads_as_documented() {
        assert_eq!(parse_l1_budget(None), L1_COPY_BUDGET);
        assert_eq!(parse_l1_budget(Some("0")), i64::MAX); // explicitly unlimited
        assert_eq!(parse_l1_budget(Some("512")), 512);
        assert_eq!(parse_l1_budget(Some("  512  ")), 512);
        for bad in ["", "yes", "-1", "4096x", "1e6"] {
            assert_eq!(parse_l1_budget(Some(bad)), L1_COPY_BUDGET, "`{bad}`");
        }
    }
}

/// [`message_fits`] — the receive-side budget probe (ADR-245). What it has to get right is
/// the *direction* of its errors: judging a big message small leaves the stall in place,
/// while judging a small one big costs only one extra lock acquire.
#[cfg(test)]
mod message_fits_tests {
    use super::{message_fits, Message};

    fn ints(n: usize) -> Vec<Message> {
        (0..n as i64).map(Message::Int).collect()
    }

    #[test]
    fn an_ordinary_message_fits() {
        // The dominant shape: a small keyword-led tuple.
        let m = Message::Vector(vec![
            Message::Keyword(1),
            Message::Int(7),
            Message::Str("ok".into()),
        ]);
        assert!(message_fits(&m, 4096));
    }

    #[test]
    fn a_large_container_does_not_fit() {
        assert!(!message_fits(&Message::Vector(ints(10_000)), 4096));
        assert!(!message_fits(&Message::List(ints(10_000), None), 4096));
        assert!(!message_fits(&Message::Set(ints(10_000)), 4096));
        let entries: Vec<(Message, Message)> = (0..10_000i64)
            .map(|i| (Message::Int(i), Message::Int(i)))
            .collect();
        assert!(!message_fits(&Message::Map(entries), 4096));
    }

    /// Nesting must not hide size: a shallow container of deep ones is still big.
    #[test]
    fn nesting_does_not_hide_the_count() {
        let inner: Vec<Message> = (0..8).map(|_| Message::Vector(ints(1000))).collect();
        assert!(!message_fits(&Message::Vector(inner), 4096));
    }

    /// The probe must stop at the budget rather than walking the whole tree — otherwise
    /// it becomes the stall it exists to prevent. Pinned behaviourally: a message far
    /// past the budget answers the same as one just past it.
    #[test]
    fn the_walk_is_bounded_not_exhaustive() {
        assert!(!message_fits(&Message::Vector(ints(4098)), 4096));
        assert!(!message_fits(&Message::Vector(ints(2_000_000)), 4096));
    }

    /// A `Str` is one node but a `memcpy` on rebuild, and — unlike the send side — a
    /// `Message` can arrive from a remote encoder, so a long inline string is possible.
    #[test]
    fn a_long_inline_string_is_charged_by_its_payload() {
        assert!(message_fits(&Message::Str("x".repeat(4096)), 4096));
        assert!(!message_fits(
            &Message::Str("x".repeat(8 * 1024 * 1024)),
            4096
        ));
    }

    /// A refusal deep inside a container has to propagate out — the walk returns early on
    /// a negative count rather than continuing to the next sibling.
    ///
    /// (`Message::Closure`, the other refusal, is not constructed here: a `ClosureMsg`
    /// needs a real compiled closure. It is covered end-to-end by the closure-shipping
    /// tests, which rebuild one through this same scan.)
    #[test]
    fn a_refusal_inside_a_container_propagates_out() {
        let m = Message::Vector(vec![
            Message::Keyword(1),
            Message::Str("x".repeat(8 * 1024 * 1024)),
            Message::Int(2),
        ]);
        assert!(!message_fits(&m, 4096));
    }
}
