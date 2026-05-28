//! Processes: share-nothing green processes communicating by message passing
//! (`spawn`/`send`/`receive`/`self`).
//!
//! **Step 4b** (see `docs/scheduler.md`, ADR-018): processes are lightweight
//! *green* threads, not OS threads. Each runs inside a [`corosensei`] stackful
//! coroutine — its own parkable stack — so the native recursive evaluator runs
//! unchanged and `receive` on an empty mailbox **suspends** the coroutine instead
//! of blocking a thread. A small pool of worker OS threads (≈ `nproc`, a setting)
//! runs ready processes off a shared run queue; `send` wakes a parked process.
//!
//! **Code is shared, data is not.** A spawned process shares the runtime's code +
//! global table (the `Arc`s in its `Heap`), so a `def` reaches it; but its data
//! lives in its own LOCAL heap, so messages cross as a self-contained, `Send`
//! [`Message`] (a deep copy), rebuilt into the receiver's heap. Symbols travel as
//! their global interned id (the interner is process-wide).
//!
//! The thread that started the program (the REPL / file runner) is a *root*
//! process: it is not a coroutine, so its `receive` **blocks** on its mailbox
//! rather than yielding. Everything `spawn`ed is a green process that yields.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Once};
use std::time::{Duration, Instant};

use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::core::heap::Heap;
use crate::core::value::{self, Closure, ClosureId, EnvId, MapId, Symbol, Value};
use crate::error::{LispError, LispResult, Pos};
use crate::eval;

/// A `Send`, self-contained copy of a value, for crossing heaps.
#[derive(Clone)]
pub enum Message {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
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
    Pid { node: Symbol, id: u64 },
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
    pub(crate) params: Vec<Symbol>,
    /// `&optional` params with their default *forms* (data).
    pub(crate) optionals: Vec<(Symbol, Message)>,
    pub(crate) rest: Option<Symbol>,
    /// The body forms (data — this is the code, homoiconically).
    pub(crate) body: Vec<Message>,
    pub(crate) doc: Option<String>,
    /// The closure's *free variables* that resolve to a **local** binding, flattened
    /// to one frame (name → value). Empty = a global-capturing closure (the common
    /// case, e.g. a `(spawn (* (+ 1 1)))` thunk). We copy only what the body actually
    /// references from its lexical scope — not the whole frame chain — so unrelated
    /// (and possibly unsendable) siblings don't ride along, and a closure capturing a
    /// sibling closure can't form a serialisation cycle through its defining frame.
    pub(crate) captured: Vec<(Symbol, Message)>,
}

/// Deep-copy a value out of `heap` into a `Send` message. A closure is sent as
/// data (see [`ClosureMsg`]); builtins and macros can't be.
pub fn to_message(heap: &Heap, v: Value) -> Result<Message, LispError> {
    to_message_rec(heap, v, &mut Vec::new())
}

/// `visited` carries the closures currently being serialised, so a self- or
/// mutually-recursive *local* closure is rejected cleanly instead of looping.
fn to_message_rec(heap: &Heap, v: Value, visited: &mut Vec<ClosureId>) -> Result<Message, LispError> {
    Ok(match v {
        Value::Nil => Message::Nil,
        Value::Bool(b) => Message::Bool(b),
        Value::Int(n) => Message::Int(n),
        Value::Float(f) => Message::Float(f),
        Value::Sym(s) => Message::Sym(s),
        Value::Keyword(s) => Message::Keyword(s),
        Value::Str(id) => Message::Str(heap.string(id).to_string()),
        Value::Pair(_) => {
            let pos = heap.form_pos(v);
            let items = heap.list_to_vec(v)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(heap, item, visited)?);
            }
            Message::List(out, pos)
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message_rec(heap, item, visited)?);
            }
            Message::Vector(out)
        }
        Value::Map(id) => {
            let entries = heap.map(id).to_vec();
            let mut out = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                out.push((to_message_rec(heap, k, visited)?, to_message_rec(heap, v, visited)?));
            }
            Message::Map(out)
        }
        Value::Ref(n) => Message::Ref(n),
        Value::Pid { node, id } => Message::Pid { node, id },
        Value::Fn(id) => Message::Closure(Box::new(closure_to_message(heap, id, visited)?)),
        Value::Macro(_) => {
            return Err(LispError::type_err("cannot send a macro in a message"))
        }
        Value::Native(_) => {
            // A builtin is a Rust function pointer with no portable form — and on
            // another node the receiver has its own copy anyway. Reference it by
            // the symbol it's bound to instead of capturing its value.
            return Err(LispError::type_err(
                "cannot send a builtin in a message; reference it by name (code is shared)",
            ))
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
        for &form in &cl.body {
            collect_symbols(heap, form, &mut mentioned);
        }
        for &(_, d) in &cl.optionals {
            collect_symbols(heap, d, &mut mentioned);
        }
        for sym in mentioned {
            if let Some(val) = local_lookup(heap, env, sym) {
                captured.push((sym, to_message_rec(heap, val, visited)?));
            }
        }
    }

    let optionals = cl
        .optionals
        .iter()
        .map(|&(s, d)| Ok((s, to_message_rec(heap, d, visited)?)))
        .collect::<Result<Vec<_>, LispError>>()?;
    let body = cl
        .body
        .iter()
        .map(|&f| to_message_rec(heap, f, visited))
        .collect::<Result<Vec<_>, LispError>>()?;

    visited.pop();
    Ok(ClosureMsg {
        name: cl.name,
        params: cl.params.clone(),
        optionals,
        rest: cl.rest,
        body,
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
            for (k, v) in heap.map(id).to_vec() {
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
        Message::Float(f) => Value::Float(*f),
        Message::Sym(s) => Value::Sym(*s),
        Message::Keyword(s) => Value::Keyword(*s),
        Message::Str(s) => heap.alloc_string(s),
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
        Message::Closure(c) => closure_from_message(heap, c),
    }
}

/// Rebuild a serialised closure into `heap`. Body/optional-default forms are
/// reconstructed as local data; captured frames are recreated (outermost first)
/// and chained onto this process's global scope, so the closure's free globals
/// resolve here. The result is a fresh, independent copy — a later redefinition
/// of *this* function won't reach it, but globals it *references* still do.
fn closure_from_message(heap: &mut Heap, c: &ClosureMsg) -> Value {
    let optionals = c
        .optionals
        .iter()
        .map(|(s, d)| (*s, from_message(heap, d)))
        .collect();
    let body = c.body.iter().map(|f| from_message(heap, f)).collect();
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
        params: c.params.clone(),
        optionals,
        rest: c.rest,
        body,
        doc: c.doc.clone(),
        env,
    });
    Value::Fn(id)
}

// ----- mailboxes & processes -------------------------------------------------

/// Why a green process's coroutine yielded control back to its worker.
enum Suspend {
    /// Blocked in `receive` on an empty mailbox — park until a message (or timer).
    Receive,
    /// Preempted by the reduction counter — still runnable, re-queue immediately.
    Preempt,
}

type Yielder0 = Yielder<(), Suspend>;
type Coro = Coroutine<(), Suspend, ()>;

/// A process's mailbox. Guarded by one mutex so the "check empty → park" and
/// "deliver → wake" handshakes stay race-free (see `receive`/`send`/`run_one`).
struct Mailbox {
    state: Mutex<MailboxState>,
    /// Wakes a *root* process blocked in `receive` (greens are woken by being
    /// re-queued instead).
    cv: Condvar,
}

struct MailboxState {
    queue: VecDeque<Message>,
    /// The parked green process waiting on this mailbox, if any. `send` takes it
    /// and re-queues it. (A short-lived `Process → Arc<Mailbox> → Process` cycle
    /// while parked; broken the moment it's re-queued or the process ends.)
    waiter: Option<Box<Process>>,
    /// How many leading messages the parked waiter already scanned and rejected
    /// (selective receive). The worker re-runs it only when a message arrives
    /// *beyond* this — not for ones it already skipped. 0 for a plain FIFO receive.
    scanned: usize,
}

impl Mailbox {
    fn new() -> Arc<Mailbox> {
        Arc::new(Mailbox {
            state: Mutex::new(MailboxState {
                queue: VecDeque::new(),
                waiter: None,
                scanned: 0,
            }),
            cv: Condvar::new(),
        })
    }
}

/// A green process: its mailbox plus the coroutine carrying its computation
/// (which owns its `Heap`). `Send`, so any worker can run it.
struct Process {
    pid: u64,
    mailbox: Arc<Mailbox>,
    coro: Coro,
}

// SAFETY: corosensei marks `Coroutine` `!Send` conservatively. We move a process
// between worker threads only via the run queue, which owns it exclusively — it is
// never resumed on two threads at once, and corosensei supports resuming a
// coroutine on a different thread than the one it suspended on. Its captured state
// (heap, Arcs, message values) is all `Send`. So migrating a *parked* process is
// sound. (See docs/scheduler.md; swappable if we drop corosensei.)
unsafe impl Send for Process {}

/// What a running coroutine needs to find from deep inside `eval` (for
/// `receive`/`self`). Stored in a thread-local, set by the coroutine at start and
/// re-established after every suspend (so it survives the worker multiplexing
/// other processes, and migration to another worker).
#[derive(Clone)]
struct Ctx {
    pid: u64,
    mailbox: Arc<Mailbox>,
    /// `Some` for a green process (suspend via this yielder); `None` for the root
    /// thread (block on the mailbox condvar instead).
    yielder: Option<*const Yielder0>,
}

thread_local! {
    static CURRENT: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

// ----- reduction-counted preemption ------------------------------------------

thread_local! {
    /// Reductions left in the current process's scheduling quantum. The worker
    /// resets it to `REDUCTION_BUDGET` before each `resume` (see `run_one`); `eval`
    /// decrements it via `tick`, and the process yields when it hits zero.
    static REDUCTIONS: Cell<u32> = const { Cell::new(0) };

    /// GC-block depth: how many `eval` / `macroexpand_all` frames are active on
    /// this thread. The eval safepoint runs GC iff this is **1** ("we are the
    /// outermost contributor — no other eval/macroexpand frame holds an
    /// unrooted LOCAL transient"). See `docs/memory-model.md` and the
    /// rooting-completeness argument in `eval::eval`.
    ///
    /// Per-process: reset to 0 at coroutine entry and saved/restored around
    /// suspend (see `spawn` / `preempt` / `wait_for_message`), so workers
    /// multiplexing several processes don't leak each other's depths. The root
    /// thread doesn't multiplex, so its depth flows naturally.
    static GC_BLOCK: Cell<u32> = const { Cell::new(0) };
}

/// Current GC-block depth — `eval::eval`'s safepoint compares this against 1.
#[inline]
pub fn gc_block_depth() -> u32 {
    GC_BLOCK.with(|d| d.get())
}

/// Read the GC-block depth for save/restore around a coroutine suspend (we want
/// to capture this process's value so a resume on any worker restores it).
#[inline]
fn gc_block_save() -> u32 {
    GC_BLOCK.with(|d| d.get())
}

/// Write the GC-block depth — paired with `gc_block_save` around a suspend,
/// and used by a fresh coroutine to wipe the residual value left on the worker.
#[inline]
fn gc_block_set(n: u32) {
    GC_BLOCK.with(|d| d.set(n));
}

/// RAII guard: increments `GC_BLOCK` on construction, decrements on `Drop`.
/// Acquired at the top of every `eval` call and every `macroexpand_all` call —
/// the two contexts that hold unrooted LOCAL transients between safepoints.
/// `Drop` runs on a normal return *and* on a panic unwind, so the depth never
/// leaks past a frame's lifetime.
pub struct GcBlockGuard;

impl GcBlockGuard {
    #[inline]
    pub fn enter() -> Self {
        GC_BLOCK.with(|d| d.set(d.get() + 1));
        GcBlockGuard
    }
}

impl Drop for GcBlockGuard {
    #[inline]
    fn drop(&mut self) {
        GC_BLOCK.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// How many `eval` loop iterations a process runs before it must yield its worker
/// (cooperative fairness — the BEAM's mechanism). ~2000 ≈ the BEAM default; tunable.
const REDUCTION_BUDGET: u32 = 2000;

/// Called once per `eval` `'tail:` iteration. Cheap: a thread-local decrement; only
/// when the budget is exhausted does it touch `CURRENT` and (for a green process)
/// suspend. Bounds the work any one process does before peers get the worker, so a
/// CPU-bound process can't monopolise a core. The root thread is never preempted
/// (it has no yielder) — it just refreshes the budget.
#[inline]
pub fn tick() {
    REDUCTIONS.with(|r| {
        let n = r.get();
        if n == 0 {
            preempt();
        } else {
            r.set(n - 1);
        }
    });
}

/// Budget exhausted: refresh it, then — if we're a green process — yield the worker
/// (re-queued Ready by `run_one`). Re-establishes `CURRENT` after resume, since the
/// worker may have run other processes meanwhile (cf. `receive`).
fn preempt() {
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    let ctx = match CURRENT.with(|c| c.borrow().clone()) {
        Some(c) => c,
        None => return, // no process context (e.g. prelude build) — nothing to yield to
    };
    if let Some(yptr) = ctx.yielder {
        // Save this process's GC-block depth before yielding: a worker may pick
        // up another process whose eval/macroexpand changes the thread-local,
        // and we need ours back when we resume.
        let saved_block = gc_block_save();
        // SAFETY: same invariant as `receive` — the yielder is valid while this
        // coroutine is running, which is now (tick runs inside eval, inside the
        // coroutine body). Suspending returns control to the worker (`run_one`).
        unsafe { (*yptr).suspend(Suspend::Preempt) };
        CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        gc_block_set(saved_block);
    }
    // Root thread (yielder None): budget refreshed, never suspends.
}

// ----- the run queue + worker pool -------------------------------------------

static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static SPAWNED: AtomicU64 = AtomicU64::new(0);
static RUNNING: AtomicUsize = AtomicUsize::new(0); // processes inside `resume` right now
static PEAK_RUNNING: AtomicUsize = AtomicUsize::new(0);
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0); // 0 = default (≈ nproc)
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0); // worker threads actually started
static WORKERS_STARTED: Once = Once::new();

/// pid → mailbox, for `send` to find a target from any thread.
static REGISTRY: LazyLock<Mutex<HashMap<u64, Arc<Mailbox>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A monitor's *watcher* side — who gets the `[:down …]` when the watched pid
/// dies. The same enum carries a process on this runtime (`Local`) and a
/// process on a peer runtime (`Remote`), so one `MONITORS` table holds both
/// shapes and the same `deregister` / `demonitor` code drives them. The peer
/// learns we want a watch via the `dist::Frame::Monitor` frame, the down
/// notification rides back as an ordinary `Message::Vector([:down …])` send.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Watcher {
    /// A process on *this* runtime.
    Local { pid: u64, mref: u64 },
    /// A process on a peer runtime — `node` names that runtime, `pid` is the
    /// watcher's pid *over there* (so a peer's `[:down …]` lands in its own
    /// mailbox), `mref` is the watcher's monitor reference (opaque to us).
    Remote { node: Symbol, pid: u64, mref: u64 },
}

/// Monitors: watched-pid → the watchers to notify when it dies. Each watcher
/// gets a `[:down <mref> <pid> <reason>]` message (Erlang `monitor`,
/// unidirectional and one-shot). No links yet — a monitor never affects the
/// watched process. A single table holds both `Local` and `Remote` watchers,
/// so the local-monitor path and the cross-node-monitor path share the same
/// "is the target alive? add or fire :noproc" logic and the same fan-out from
/// `deregister`.
static MONITORS: LazyLock<Mutex<HashMap<u64, Vec<Watcher>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// **Pending remote monitors** — the *sender* side of `(monitor remote-pid)`.
/// Keyed by the peer's node-name, valued by the local triples we'd need to
/// fire `[:down mref pid :noconnection]` should the link to that peer die
/// (Erlang semantics: a monitor fires on net-split). Compact mirror of
/// `MONITORS` — no down-delivery state, just enough to wake the watcher
/// when the wire goes away.
static PENDING_REMOTE: LazyLock<Mutex<HashMap<Symbol, Vec<PendingRemote>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct PendingRemote {
    /// The local watcher to deliver `[:down …]` to.
    watcher_pid: u64,
    /// The monitor reference the watcher will pattern-match on.
    mref: u64,
    /// The remote pid being watched — the message's `pid` field.
    target_node: Symbol,
    target_pid: u64,
}

/// Source of unique reference ids, shared by `(ref)` and `(monitor …)` so every
/// ref — token or monitor handle — is distinct across the whole runtime.
static NEXT_REF: AtomicU64 = AtomicU64::new(0);

/// A fresh, globally-unique reference id. Backs `Value::Ref`.
pub fn next_ref() -> u64 {
    NEXT_REF.fetch_add(1, Ordering::Relaxed)
}

thread_local! {
    /// Set by a process's coroutine just before it returns, so `run_one` can read
    /// the exit reason (for monitor `[:down …]` delivery) once `resume` returns on
    /// this same worker thread. Cleared at the start of every scheduling quantum.
    static EXIT_REASON: std::cell::RefCell<Option<Message>> = const { std::cell::RefCell::new(None) };
}

/// The shared run queue of ready processes + a condvar workers wait on.
static RUN: LazyLock<(Mutex<VecDeque<Box<Process>>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(VecDeque::new()), Condvar::new()));

/// Total processes spawned since program start (read by `(spawn-count)`).
pub fn spawn_count() -> u64 {
    SPAWNED.load(Ordering::SeqCst)
}

/// Set the worker-pool size (0 = default ≈ `nproc`). Call once at startup, before
/// any spawning. (Replaces the old per-spawn thread cap.)
pub fn set_max_parallel(n: usize) {
    WORKER_COUNT.store(n, Ordering::SeqCst);
}

/// High-water mark of processes running simultaneously (≤ pool size).
pub fn peak_threads() -> u64 {
    PEAK_RUNNING.load(Ordering::SeqCst) as u64
}

/// Worker OS threads in the scheduler pool (0 until the first `spawn` starts them).
pub fn worker_threads() -> u64 {
    ACTIVE_WORKERS.load(Ordering::SeqCst) as u64
}

fn worker_count() -> usize {
    match WORKER_COUNT.load(Ordering::SeqCst) {
        0 => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        n => n,
    }
}

/// A process has finished (or crashed): drop its mailbox and fire any
/// monitors, delivering `[:down <mref> <pid> <reason>]` to each watcher —
/// `Local` watchers via `deliver` (in-process mailbox push), `Remote`
/// watchers via the dist layer (an ordinary `send` to a remote pid, which
/// routes over the link). Same `[:down …]` shape in both cases — the
/// receiver code on the wire side is unchanged from local.
fn deregister(pid: u64, reason: Message) {
    REGISTRY.lock().unwrap().remove(&pid);
    let watchers = MONITORS.lock().unwrap().remove(&pid).unwrap_or_default();
    for w in watchers {
        fire_down(w, pid, reason.clone());
    }
}

/// Deliver a `[:down …]` to one watcher — the single fan-out point both
/// `deregister` (target died) and `add_monitor` (target was already dead)
/// use. Local watchers get an in-process mailbox push; remote watchers get
/// a routed `send`, so the wire-format `[:down …]` is exactly the message a
/// peer's process would receive locally.
fn fire_down(w: Watcher, dying_pid: u64, reason: Message) {
    let msg = down_message(local_node_pid_msg(dying_pid), w.mref(), reason);
    match w {
        Watcher::Local { pid, .. } => deliver(pid, msg),
        Watcher::Remote { node, pid, .. } => {
            crate::dist::route(node, crate::dist::Target::Pid(pid), msg);
        }
    }
}

impl Watcher {
    /// Both variants carry a monitor ref; surface it for shared code paths.
    fn mref(&self) -> u64 {
        match *self {
            Watcher::Local { mref, .. } | Watcher::Remote { mref, .. } => mref,
        }
    }
}

/// The `Message::Pid` for a process that lives on **this** runtime — the
/// `pid` field of a `[:down …]` we're firing. Wraps the (always-local-here)
/// node-name lookup so the call site reads as "make the pid value".
fn local_node_pid_msg(pid: u64) -> Message {
    Message::Pid {
        node: crate::dist::local_node(),
        id: pid,
    }
}

/// The `[:down <pid> <mref> <reason>]` message a monitor fires. `pid_msg`
/// is the dying process's pid as a `Message::Pid` — `Local` watchers see
/// this runtime's name there, `Remote` watchers see the same thing (still
/// correct: the dying pid lives on us).
fn down_message(pid_msg: Message, mref: u64, reason: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("down")),
        Message::Ref(mref),
        pid_msg,
        reason,
    ])
}

/// Push a (already-`Send`) message into local process `pid`'s mailbox and wake it;
/// a no-op if `pid` is gone. The shared tail of `send`, monitor `[:down …]`
/// delivery, and inbound node-link messages (`crate::dist`).
pub(crate) fn deliver(pid: u64, msg: Message) {
    let mailbox = REGISTRY.lock().unwrap().get(&pid).cloned();
    if let Some(mb) = mailbox {
        let mut st = mb.state.lock().unwrap();
        st.queue.push_back(msg);
        if let Some(proc) = st.waiter.take() {
            drop(st);
            enqueue(proc); // wake a parked green process
        } else {
            mb.cv.notify_one(); // wake the root thread, if it's blocked in receive
        }
    }
}

/// `(monitor pid)` — start watching the **local** `pid`; returns a fresh
/// monitor `ref`. When `pid` dies, the caller receives `[:down <this-ref>
/// <pid> <reason>]`. If `pid` is already dead, the DOWN (`reason` `:noproc`)
/// is delivered immediately. The monitor is unidirectional and one-shot.
///
/// Routes through [`add_monitor`] with a `Watcher::Local`; the cross-node
/// path (`dist::Frame::Monitor`) calls the same function with a
/// `Watcher::Remote`, so the alive/dead branch and the `:noproc` fast path
/// are shared.
pub fn monitor(target: u64) -> Value {
    let mref = next_ref();
    add_monitor(
        target,
        Watcher::Local {
            pid: self_pid(),
            mref,
        },
    );
    Value::Ref(mref)
}

/// The shared "register a watcher" core — used by the local `monitor` builtin
/// and by `dist`'s `Frame::Monitor` handler. If `target` is alive, append
/// `watcher` to its monitor list; otherwise fire a synthetic `:noproc` down
/// to that same watcher immediately. The "delivery channel" of the down
/// (in-process mailbox vs. routed `send`) is decided inside [`fire_down`].
pub(crate) fn add_monitor(target: u64, watcher: Watcher) {
    let alive = REGISTRY.lock().unwrap().contains_key(&target);
    if alive {
        MONITORS
            .lock()
            .unwrap()
            .entry(target)
            .or_default()
            .push(watcher);
    } else {
        fire_down(watcher, target, Message::Keyword(value::intern("noproc")));
    }
}

/// `(demonitor mref)` — drop the calling process's monitor with that ref. Best
/// effort: a `[:down …]` already queued is not recalled.
pub fn demonitor(mref: u64) {
    let me = self_pid();
    drop_monitor(|w| matches!(*w, Watcher::Local { pid, mref: r } if pid == me && r == mref));
}

/// Remove every `Watcher` matching `pred` from `MONITORS`. The shared dropper
/// behind local `(demonitor mref)`, remote `Frame::Demonitor`, and the
/// node-down cleanup that flushes a peer's remote watchers from every target's
/// list.
pub(crate) fn drop_monitor(pred: impl Fn(&Watcher) -> bool) {
    let mut mons = MONITORS.lock().unwrap();
    for watchers in mons.values_mut() {
        watchers.retain(|w| !pred(w));
    }
}

// ---- pending remote monitors: the *sender* side ----------------------------
// When `(monitor remote-pid)` runs, the target lives on a peer; the entry that
// fires when the link dies (net-split = `:noconnection`) needs to be findable
// here. PENDING_REMOTE is the dual of MONITORS — same shape, watched-from
// instead of watched-by.

/// Remember "this local watcher is monitoring `target_node:target_pid`",
/// keyed by the peer node so net-split can find and fire it. Mirrors what
/// `add_monitor` does for local watchers, in a separate table because the
/// failure mode (link drop) is independent of any local target's death.
pub(crate) fn record_pending_remote(
    target_node: Symbol,
    target_pid: u64,
    watcher_pid: u64,
    mref: u64,
) {
    PENDING_REMOTE
        .lock()
        .unwrap()
        .entry(target_node)
        .or_default()
        .push(PendingRemote {
            watcher_pid,
            mref,
            target_node,
            target_pid,
        });
}

/// Forget a pending remote monitor — the sender-side counterpart to
/// `drop_monitor`, called from `dist::demonitor_remote`. Identified by
/// (target_node, watcher_pid, mref) — the same triple `record_pending_remote`
/// stored.
pub(crate) fn drop_pending_remote(target_node: Symbol, watcher_pid: u64, mref: u64) {
    let mut t = PENDING_REMOTE.lock().unwrap();
    if let Some(v) = t.get_mut(&target_node) {
        v.retain(|p| !(p.watcher_pid == watcher_pid && p.mref == mref));
    }
}

/// `(demonitor mref)` on a ref the local table didn't claim: scan
/// `PENDING_REMOTE` for entries matching `(self_pid, mref)`, dispatch one
/// `Demonitor` frame per unique peer holding such an entry, and prune the
/// local pending side. The fan-out happens here (not in the builtin) so the
/// peer-set discovery and `drop_pending_remote` cleanup stay co-located.
pub(crate) fn demonitor_remote_fanout(mref: u64) {
    let me = self_pid();
    let peers: Vec<Symbol> = {
        let table = PENDING_REMOTE.lock().unwrap();
        table
            .iter()
            .filter(|(_, ps)| ps.iter().any(|p| p.watcher_pid == me && p.mref == mref))
            .map(|(node, _)| *node)
            .collect()
    };
    for node in peers {
        crate::dist::demonitor_remote(node, me, mref);
    }
}

/// The link to `node` just died. Drop **two** sets of monitors:
///   1. **Pending remote**: monitors *we* asked the peer to keep for us. Each
///      fires `[:down mref pid :noconnection]` to the local watcher (Erlang
///      semantics on net-split).
///   2. **Inbound remote**: watchers the peer registered on our local pids.
///      No notification — the peer is gone — but the entries would otherwise
///      leak and a future reconnect would still try to deliver to a fresh
///      generation of that peer.
pub(crate) fn handle_node_down(node: Symbol) {
    let pendings = PENDING_REMOTE.lock().unwrap().remove(&node).unwrap_or_default();
    for p in pendings {
        deliver(
            p.watcher_pid,
            down_message(
                Message::Pid {
                    node: p.target_node,
                    id: p.target_pid,
                },
                p.mref,
                Message::Keyword(value::intern("noconnection")),
            ),
        );
    }
    drop_monitor(|w| matches!(*w, Watcher::Remote { node: n, .. } if n == node));
}

/// Fire `:noconnection` to one watcher (the link isn't up, so we can't ask
/// the peer to monitor for us). Shared with `dist::monitor_remote`. Uses the
/// same `down_message` shape as a real DOWN so the watcher's `receive` clause
/// doesn't have to special-case anything.
pub(crate) fn fire_noconnection(target_node: Symbol, target_pid: u64, watcher_pid: u64, mref: u64) {
    deliver(
        watcher_pid,
        down_message(
            Message::Pid {
                node: target_node,
                id: target_pid,
            },
            mref,
            Message::Keyword(value::intern("noconnection")),
        ),
    );
}

/// Push a ready process onto the run queue and wake a worker.
fn enqueue(proc: Box<Process>) {
    let (lock, cv) = &*RUN;
    lock.lock().unwrap().push_back(proc);
    cv.notify_one();
}

/// Start the worker pool exactly once (on the first `spawn`).
fn ensure_workers() {
    WORKERS_STARTED.call_once(|| {
        let n = worker_count();
        ACTIVE_WORKERS.store(n, Ordering::SeqCst);
        for _ in 0..n {
            std::thread::spawn(worker_loop);
        }
    });
}

fn worker_loop() {
    loop {
        let proc = {
            let (lock, cv) = &*RUN;
            let mut q = lock.lock().unwrap();
            loop {
                if let Some(p) = q.pop_front() {
                    break p;
                }
                q = cv.wait(q).unwrap();
            }
        };
        run_one(proc);
    }
}

/// Resume a process once, then either retire it (it finished) or, if it suspended
/// at `receive`, park it on its mailbox (or re-queue it if a message raced in).
fn run_one(mut proc: Box<Process>) {
    let mailbox = Arc::clone(&proc.mailbox);

    let live = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
    PEAK_RUNNING.fetch_max(live, Ordering::SeqCst);
    // Fresh reduction budget for this scheduling quantum (decremented in eval's loop
    // via `tick`; at zero the process preempts itself — see `tick`/`preempt`).
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    EXIT_REASON.with(|r| *r.borrow_mut() = None); // stale from a prior process on this worker
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.coro.resume(())));
    RUNNING.fetch_sub(1, Ordering::SeqCst);

    match outcome {
        Ok(CoroutineResult::Return(())) => {
            // The coroutine set its exit reason just before returning (see `spawn`).
            let reason = EXIT_REASON
                .with(|r| r.borrow_mut().take())
                .unwrap_or_else(|| Message::Keyword(value::intern("normal")));
            deregister(proc.pid, reason);
        }
        Ok(CoroutineResult::Yield(Suspend::Receive)) => {
            // The coroutine suspended in `receive`: it scanned the first `scanned`
            // messages with no match. Re-check under the lock — if a *new*
            // (unscanned) message arrived during the suspend window, run again;
            // otherwise park here for `send` (or the timer) to wake.
            let mut st = mailbox.state.lock().unwrap();
            if st.queue.len() > st.scanned {
                drop(st);
                enqueue(proc);
            } else {
                st.waiter = Some(proc);
            }
        }
        Ok(CoroutineResult::Yield(Suspend::Preempt)) => {
            // Preempted mid-computation (reduction budget hit). Still runnable —
            // re-queue at the back so peers get a turn on this worker (fairness).
            enqueue(proc);
        }
        Err(_) => {
            eprintln!("process {} panicked", proc.pid);
            deregister(proc.pid, Message::Keyword(value::intern("killed")));
        }
    }
}

// ----- spawn / send / receive / self ----------------------------------------

/// `(%spawn thunk)` — run `thunk` (a 0-arg function) as a new green process.
/// Returns the new pid. The user-facing `spawn` macro wraps an arbitrary
/// expression into such a thunk (`(spawn e)` → `(%spawn (fn () e))`), so the
/// expression's free locals are captured lexically rather than passed as args.
pub fn spawn(heap: &Heap, f: Value) -> Result<u64, LispError> {
    // Promote the thunk into the shared RUNTIME region so its handle (and any
    // captured local scope) is valid in the child, which shares this runtime's
    // code via the Arcs below. A top-level function is already shared (no-op).
    let f = heap.promote(f);
    if !matches!(f, Value::Fn(_)) {
        return Err(LispError::type_err("spawn: argument must be a function"));
    }
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    let mailbox = Mailbox::new();
    REGISTRY.lock().unwrap().insert(pid, Arc::clone(&mailbox));

    let coro_mailbox = Arc::clone(&mailbox);
    let coro = Coroutine::new(move |yielder: &Yielder0, _input: ()| {
        // Establish this process's context so `receive`/`self` can find it.
        CURRENT.with(|c| {
            *c.borrow_mut() = Some(Ctx {
                pid,
                mailbox: Arc::clone(&coro_mailbox),
                yielder: Some(yielder as *const Yielder0),
            });
        });
        // Wipe the worker's residual GC-block depth — a previous coroutine on
        // this worker may have left it nonzero. Our depth starts fresh at 0
        // (incremented by the eval guard below).
        gc_block_set(0);
        let mut heap = Heap::with_regions(prelude, runtime);
        heap.set_global(EnvId::GLOBAL);
        let reason = match eval::apply(&mut heap, f, &[], EnvId::GLOBAL) {
            Ok(_) => Message::Keyword(value::intern("normal")),
            Err(e) => {
                eprintln!("process {} died: {}", pid, e);
                // A crash reason monitors can inspect; the message string for now
                // (a richer structured reason can come with links later).
                Message::Vector(vec![
                    Message::Keyword(value::intern("error")),
                    Message::Str(e.to_string()),
                ])
            }
        };
        EXIT_REASON.with(|r| *r.borrow_mut() = Some(reason));
        CURRENT.with(|c| *c.borrow_mut() = None);
    });

    ensure_workers();
    enqueue(Box::new(Process { pid, mailbox, coro }));
    Ok(pid)
}

/// `(send target msg)` — copy `msg` into `target`'s mailbox and wake it. `target`
/// is a **pid** (local or remote — it carries node identity) or a `{:name :node}`
/// **registered-name address** for bootstrapping a peer before you hold its pid.
/// Routing is location-transparent: a local target delivers in-process; a remote
/// one is forwarded over the node link (`crate::dist`). Sending to a dead/unknown
/// target is a silent no-op (Erlang semantics).
pub fn send(heap: &Heap, target_val: Value, msg_val: Value) -> Result<(), LispError> {
    let msg = to_message(heap, msg_val)?;
    match target_val {
        Value::Pid { node, id } => crate::dist::route(node, crate::dist::Target::Pid(id), msg),
        Value::Map(mid) => {
            let (name, node) = read_name_address(heap, mid)?;
            crate::dist::route(node, crate::dist::Target::Name(name), msg);
        }
        _ => {
            return Err(LispError::type_err(
                "send: target must be a pid or a {:name :node} address",
            ))
        }
    }
    Ok(())
}

/// Read a `{:name <kw> :node <kw>}` registered-name address out of a map, returning
/// the `(name, node)` symbols. Accepts keyword or symbol values for each field.
fn read_name_address(heap: &Heap, mid: MapId) -> Result<(Symbol, Symbol), LispError> {
    let field = |key: &str| -> Result<Symbol, LispError> {
        let v = heap
            .map_get(mid, Value::Keyword(value::intern(key)))
            .ok_or_else(|| {
                LispError::type_err("send: name address needs :name and :node keys")
            })?;
        match v {
            Value::Keyword(s) | Value::Sym(s) => Ok(s),
            _ => Err(LispError::type_err(
                "send: :name and :node must be keywords or symbols",
            )),
        }
    };
    Ok((field("name")?, field("node")?))
}

/// `(%receive matcher timeout on-timeout)` — selective receive. `matcher` is a unary
/// function: given a message value it returns a 0-arg thunk (the clause body, closing
/// over its bindings) on a match, or `nil` on no match. Scan the mailbox in order;
/// the first message a clause matches is removed and its body thunk **returned** —
/// not run here. The `receive` macro applies it in tail position (`((%receive …))`),
/// so a loop that tail-calls back into `receive` trampolines through eval's TCO and
/// stays O(1) native stack (running it here would nest a `receive_match` per message
/// and overflow the green-process coroutine stack). Non-matching messages stay queued
/// (Erlang selective receive). `timeout` is `nil` (wait forever) or an integer of
/// milliseconds; on expiry the `on-timeout` thunk is returned the same way (a `throw`
/// inside it still propagates through `try`/`catch`). A green process suspends while
/// waiting; the root thread blocks.
pub fn receive_match(
    heap: &mut Heap,
    matcher: Value,
    timeout: Value,
    on_timeout: Value,
) -> LispResult {
    let deadline = match timeout {
        Value::Nil => None,
        Value::Int(ms) if ms >= 0 => Some(Instant::now() + Duration::from_millis(ms as u64)),
        _ => {
            return Err(LispError::type_err(
                "receive: timeout must be an integer (milliseconds) or nil",
            ))
        }
    };
    let ctx = ensure_ctx();
    let mut i = 0usize;
    loop {
        // Rebuild candidate `i` into the heap, then run the matcher *without* holding
        // the mailbox lock (the matcher calls eval). Only this process removes from
        // its own mailbox, so the scanned prefix is stable; `send` only appends.
        let candidate = {
            let st = ctx.mailbox.state.lock().unwrap();
            if i < st.queue.len() {
                Some(from_message(heap, &st.queue[i]))
            } else {
                None
            }
        };
        match candidate {
            Some(v) => {
                let thunk = eval::apply(heap, matcher, &[v], EnvId::GLOBAL)?;
                if matches!(thunk, Value::Fn(_)) {
                    // Matched — remove exactly this message, then hand the body thunk
                    // *back* (don't run it here). The `receive` macro applies it in
                    // TAIL position — `((%receive …))` — so a loop that tail-calls
                    // back into `receive` trampolines through eval's TCO and stays
                    // O(1) native stack (running it here instead nests a `receive_match`
                    // per message → green-process coroutine-stack overflow).
                    ctx.mailbox.state.lock().unwrap().queue.remove(i);
                    return Ok(thunk);
                }
                i += 1; // no clause matched — leave it queued, try the next message
            }
            None => {
                // Scanned every queued message with no match.
                if let Some(d) = deadline {
                    if Instant::now() >= d {
                        // Same trampoline: return the timeout thunk to be applied in
                        // tail position (the `receive` macro always supplies a fn).
                        return Ok(on_timeout);
                    }
                }
                wait_for_message(&ctx, i, deadline);
            }
        }
    }
}

/// Wait until a message beyond index `i` might be available, honouring `deadline`.
/// Green: record the scan position and suspend (arming a timer if there's a deadline,
/// so it wakes to check). Root: block on the mailbox condvar (with timeout). Returns
/// when the caller should re-scan from `i`.
fn wait_for_message(ctx: &Ctx, i: usize, deadline: Option<Instant>) {
    match ctx.yielder {
        // Root thread: block on the condvar (with timeout) until a send or deadline.
        None => {
            let st = ctx.mailbox.state.lock().unwrap();
            if st.queue.len() > i {
                return; // a message arrived between the scan and here — re-scan
            }
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now < d {
                        // Re-acquired guard dropped at end of scope (before we return).
                        let _g = ctx.mailbox.cv.wait_timeout(st, d - now);
                    }
                }
                None => {
                    let _g = ctx.mailbox.cv.wait(st);
                }
            }
        }
        // Green process: record how far we scanned (so the worker re-runs us only on
        // a *new* message — see `run_one`), then suspend. A timer wakes us at the
        // deadline; `send` wakes us on a new message.
        Some(yptr) => {
            {
                let mut st = ctx.mailbox.state.lock().unwrap();
                if st.queue.len() > i {
                    return; // raced — a message arrived; re-scan without suspending
                }
                st.scanned = i;
            }
            if let Some(d) = deadline {
                arm_timer(ctx.pid, d);
            }
            // Save this process's GC-block depth before yielding (same rationale
            // as `preempt`): the worker may run other processes whose eval/
            // macroexpand changes the thread-local before we resume.
            let saved_block = gc_block_save();
            // SAFETY: the yielder is valid while this coroutine runs — which is now
            // (called from within eval, within the coroutine body). Suspending
            // returns control to the worker (`run_one`), which parks us.
            unsafe { (*yptr).suspend(Suspend::Receive) };
            // Resumed (by send or timer): the worker may have run others or migrated
            // us to another worker — re-establish the context and depth.
            CURRENT.with(|c| *c.borrow_mut() = Some(ctx.clone()));
            gc_block_set(saved_block);
        }
    }
}

// ----- timers (receive deadlines) --------------------------------------------

/// Min-heap of `(deadline, pid)`: `Reverse` turns the max-heap into earliest-first.
type TimerQueue = BinaryHeap<Reverse<(Instant, u64)>>;

/// Pending `receive` deadlines for green processes. A dedicated thread wakes each at
/// its deadline so it can fire its `after` clause.
static TIMERS: LazyLock<(Mutex<TimerQueue>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
static TIMER_STARTED: Once = Once::new();

/// Arrange to wake green process `pid` at `deadline`. Lazily starts the timer thread
/// on first use (programs that never use a `receive` timeout never spawn it).
fn arm_timer(pid: u64, deadline: Instant) {
    TIMER_STARTED.call_once(|| {
        std::thread::spawn(timer_loop);
    });
    let (lock, cv) = &*TIMERS;
    lock.lock().unwrap().push(Reverse((deadline, pid)));
    cv.notify_one();
}

/// Sleep until the nearest deadline, then wake every process whose deadline passed.
fn timer_loop() {
    let (lock, cv) = &*TIMERS;
    let mut q = lock.lock().unwrap();
    loop {
        match q.peek().copied() {
            None => q = cv.wait(q).unwrap(),
            Some(Reverse((deadline, _))) => {
                let now = Instant::now();
                if now < deadline {
                    q = cv.wait_timeout(q, deadline - now).unwrap().0;
                } else {
                    let mut due = Vec::new();
                    while let Some(&Reverse((d, pid))) = q.peek() {
                        if d <= now {
                            q.pop();
                            due.push(pid);
                        } else {
                            break;
                        }
                    }
                    drop(q);
                    for pid in due {
                        wake_for_timeout(pid);
                    }
                    q = lock.lock().unwrap();
                }
            }
        }
    }
}

/// Re-queue green process `pid` if it's still parked, so it wakes, re-scans, and —
/// finding its deadline passed — runs its `after` clause. A no-op if `send` already
/// woke it or it re-parked on another receive; the process always re-validates its
/// own deadline, so a stale timer is harmless (at most one spurious wakeup).
fn wake_for_timeout(pid: u64) {
    let mailbox = REGISTRY.lock().unwrap().get(&pid).cloned();
    if let Some(mb) = mailbox {
        let mut st = mb.state.lock().unwrap();
        if let Some(proc) = st.waiter.take() {
            drop(st);
            enqueue(proc);
        }
    }
}

/// `(self)` — this process's pid.
pub fn self_pid() -> u64 {
    ensure_ctx().pid
}

/// Wrap a local process id in a [`Value::Pid`] tagged with this runtime's node
/// identity — what `self`/`spawn` hand back. The node part makes the pid routable
/// off-node once the holder is on another runtime.
pub fn pid_value(id: u64) -> Value {
    Value::Pid {
        node: crate::dist::local_node(),
        id,
    }
}

/// The current process's context. A coroutine sets this itself; the first time a
/// *root* thread (the REPL / file runner) uses `self`/`receive`, register it as a
/// blocking-mailbox process so it can participate in message passing.
fn ensure_ctx() -> Ctx {
    CURRENT.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            return ctx.clone();
        }
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let mailbox = Mailbox::new();
        REGISTRY.lock().unwrap().insert(pid, Arc::clone(&mailbox));
        let ctx = Ctx {
            pid,
            mailbox,
            yielder: None,
        };
        *c.borrow_mut() = Some(ctx.clone());
        ctx
    })
}
