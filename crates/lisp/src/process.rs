//! Processes: share-nothing green-ish processes communicating by message
//! passing (`spawn`/`send`/`receive`/`self`).
//!
//! This is **step 4a** of the concurrency plan (see `docs/concurrency.md`): each
//! process is backed by its own OS thread with its own [`Heap`] — real
//! parallelism, real isolation. Turning these into lightweight green threads on
//! a small worker pool (M:N, work-stealing, a 2-core cap) is step 4b and needs
//! coroutine suspension; the `spawn`/`send`/`receive` *surface* won't change.
//!
//! Because a [`Value`] is a handle into one process's heap, it cannot be read by
//! another. So messages cross as a self-contained, `Send` [`Message`] (a deep
//! copy), rebuilt into the receiver's heap. Symbols travel as their global
//! interned id (the interner is process-wide), so they stay consistent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{LazyLock, Mutex};

use crate::error::LispError;
use crate::eval;
use crate::heap::Heap;
use crate::value::{Closure, ClosureId, EnvId, Symbol, Value};
use crate::Interp;

/// A `Send`, self-contained copy of a value, for crossing heaps.
pub enum Message {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Sym(Symbol),
    Keyword(Symbol),
    List(Vec<Message>),
    Vector(Vec<Message>),
}

/// Deep-copy a value out of `heap` into a `Send` message. Functions can't be
/// sent (they're per-heap closures).
pub fn to_message(heap: &Heap, v: Value) -> Result<Message, LispError> {
    Ok(match v {
        Value::Nil => Message::Nil,
        Value::Bool(b) => Message::Bool(b),
        Value::Int(n) => Message::Int(n),
        Value::Float(f) => Message::Float(f),
        Value::Sym(s) => Message::Sym(s),
        Value::Keyword(s) => Message::Keyword(s),
        Value::Str(id) => Message::Str(heap.string(id).to_string()),
        Value::Pair(_) => {
            let items = heap.list_to_vec(v)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message(heap, item)?);
            }
            Message::List(out)
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message(heap, item)?);
            }
            Message::Vector(out)
        }
        Value::Fn(_) | Value::Macro(_) | Value::Native(_) => {
            return Err(LispError::type_err("cannot send a function in a message"))
        }
    })
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
        Message::List(items) => {
            let mut vals = Vec::with_capacity(items.len());
            for item in items {
                vals.push(from_message(heap, item));
            }
            heap.list(vals)
        }
        Message::Vector(items) => {
            let mut vals = Vec::with_capacity(items.len());
            for item in items {
                vals.push(from_message(heap, item));
            }
            heap.alloc_vector(vals)
        }
    }
}

/// A closure shipped to a new process: its code (which is data — symbols and
/// lists) travels as messages and is rebuilt against the child's global env.
/// Works for self-contained functions (those referencing only the prelude,
/// builtins, and their own parameters).
struct ShippedClosure {
    params: Vec<Symbol>,
    optionals: Vec<(Symbol, Message)>,
    rest: Option<Symbol>,
    body: Vec<Message>,
}

fn ship_closure(heap: &Heap, id: ClosureId) -> Result<ShippedClosure, LispError> {
    let cl = heap.closure(id);
    let params = cl.params.clone();
    let rest = cl.rest;
    let mut optionals = Vec::with_capacity(cl.optionals.len());
    for &(s, default) in &cl.optionals {
        optionals.push((s, to_message(heap, default)?));
    }
    let mut body = Vec::with_capacity(cl.body.len());
    for &form in &cl.body {
        body.push(to_message(heap, form)?);
    }
    Ok(ShippedClosure { params, optionals, rest, body })
}

fn install_closure(heap: &mut Heap, shipped: &ShippedClosure, env: EnvId) -> Value {
    let mut optionals = Vec::with_capacity(shipped.optionals.len());
    for (s, default) in &shipped.optionals {
        let d = from_message(heap, default);
        optionals.push((*s, d));
    }
    let mut body = Vec::with_capacity(shipped.body.len());
    for m in &shipped.body {
        body.push(from_message(heap, m));
    }
    let id = heap.alloc_closure(Closure {
        name: None,
        params: shipped.params.clone(),
        optionals,
        rest: shipped.rest,
        body,
        env,
    });
    Value::Fn(id)
}

// ----- the process table -----

static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static REGISTRY: LazyLock<Mutex<HashMap<u64, Sender<Message>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct ProcCtx {
    pid: u64,
    inbox: Receiver<Message>,
}

thread_local! {
    static CURRENT: RefCell<Option<ProcCtx>> = const { RefCell::new(None) };
}

/// Ensure the current thread has a process context (pid + mailbox). The first
/// time `self`/`receive`/`send` is used on a thread (e.g. the REPL), this
/// registers it as a process so it can participate in message passing.
fn ensure_ctx() -> u64 {
    CURRENT.with(|cell| {
        if let Some(ctx) = cell.borrow().as_ref() {
            return ctx.pid;
        }
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        REGISTRY.lock().unwrap().insert(pid, tx);
        *cell.borrow_mut() = Some(ProcCtx { pid, inbox: rx });
        pid
    })
}

fn deregister(pid: u64) {
    REGISTRY.lock().unwrap().remove(&pid);
}

/// `(spawn f arg...)` — run `f` (a function) in a new process with copied args.
/// Returns the new pid.
pub fn spawn(heap: &Heap, f: Value, args: &[Value]) -> Result<u64, LispError> {
    let cl_id = match f {
        Value::Fn(id) => id,
        _ => return Err(LispError::type_err("spawn: first argument must be a function")),
    };
    let shipped = ship_closure(heap, cl_id)?;
    let mut arg_msgs = Vec::with_capacity(args.len());
    for &a in args {
        arg_msgs.push(to_message(heap, a)?);
    }

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    // Register the mailbox in the *parent*, before the thread starts, so a
    // `send` immediately after `spawn` can't race ahead of registration.
    let (tx, rx) = mpsc::channel();
    REGISTRY.lock().unwrap().insert(pid, tx);

    std::thread::spawn(move || {
        // This thread *is* the process: adopt the pid + mailbox.
        CURRENT.with(|cell| *cell.borrow_mut() = Some(ProcCtx { pid, inbox: rx }));
        let mut interp = Interp::new();
        let f = install_closure(&mut interp.heap, &shipped, interp.root);
        let mut argv = Vec::with_capacity(arg_msgs.len());
        for m in &arg_msgs {
            argv.push(from_message(&mut interp.heap, m));
        }
        let root = interp.root;
        if let Err(e) = eval::apply(&mut interp.heap, f, &argv, root) {
            eprintln!("process {} died: {}", pid, e);
        }
        deregister(pid);
    });

    Ok(pid)
}

/// `(send pid msg)` — copy `msg` into `pid`'s mailbox. Sending to a dead pid is
/// a silent no-op (Erlang semantics).
pub fn send(heap: &Heap, pid_val: Value, msg_val: Value) -> Result<(), LispError> {
    let pid = match pid_val {
        Value::Int(n) if n >= 0 => n as u64,
        _ => return Err(LispError::type_err("send: first argument must be a pid (integer)")),
    };
    let msg = to_message(heap, msg_val)?;
    let tx = REGISTRY.lock().unwrap().get(&pid).cloned();
    if let Some(tx) = tx {
        let _ = tx.send(msg);
    }
    Ok(())
}

/// `(receive)` — take the next message from this process's mailbox, blocking
/// until one arrives.
pub fn receive(heap: &mut Heap) -> Result<Value, LispError> {
    ensure_ctx();
    let received = CURRENT.with(|cell| cell.borrow().as_ref().unwrap().inbox.recv());
    match received {
        Ok(m) => Ok(from_message(heap, &m)),
        Err(_) => Err(LispError::runtime("receive: mailbox closed")),
    }
}

/// `(self)` — this process's pid.
pub fn self_pid() -> u64 {
    ensure_ctx()
}
