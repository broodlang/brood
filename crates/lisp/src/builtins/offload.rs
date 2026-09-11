//! The dirty-native offload pool (ADR-144): run a blocking native call on a helper OS
//! thread and deliver its result back as a message, so a slow `os/run-process` or a
//! blocking read never stalls a scheduler worker.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::arg;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // The dirty-native offload pool (ADR-144): the `offload` wrapper in the
    // prelude is the policy; this is the mechanism.
    primitives.def(
        "%offload",
        Arity::exact(2),
        Sig::new(vec![any, any], int),
        &["f", "args"],
        "Run the blocking native `f` with `args` (a vector) on the dirty-offload OS pool (ADR-144) instead of this process's scheduler worker. Returns a token int immediately; the pool later delivers [:offload token result] or [:offload-error token err] to the calling process's mailbox. Only long/blocking data-in/data-out natives are allowed (%git-clone, %git-resolve-ref, %git-list-tags, %pbkdf2-sha256-bytes, %digest, %hmac, file/slurp, file/slurp-bytes, file/spit, file/spit-bytes, file/spit-append, append-bytes, tls-self-signed) — anything heap-sharing or env-reading is refused. Prefer the prelude `offload` wrapper, which parks in a selective receive and rethrows errors.",
        offload_start);
}

/// Natives a green process may run on the offload pool (`%offload`):
/// long/blocking, data-in/data-out — each touches only the scratch heap it is
/// handed (no globals, no env lookups, no process identity), so running it
/// off-process is sound. Everything else is refused: offloading a
/// heap-sharing or env-reading native would race the caller's world.
const OFFLOAD_ALLOWED: &[&str] = &[
    "%git-clone",
    "%git-resolve-ref",
    "%git-list-tags",
    "%untar-gz",
    "%pbkdf2-sha256-bytes",
    "%digest",
    "%hmac",
    "%gzip",
    "%gunzip",
    "%zlib-compress",
    "%zlib-uncompress",
    "%deflate",
    "%inflate",
    "file/slurp",
    "file/slurp-bytes",
    "file/spit",
    "file/spit-bytes",
    "file/spit-append",
    "append-bytes",
    "tls-self-signed",
    // A long guest call is exactly what the pool is for (docs/interop.md):
    // the handle is an int token, args/results are data, and the instance
    // registry is global with a per-instance mutex — off-worker is sound.
    "%wasm-call",
];

struct OffloadJob {
    func: value::NativeFnPtr,
    args: Vec<crate::process::Message>,
    sink: crate::process::MailboxSink,
    token: i64,
}

static OFFLOAD_TOKEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// The pool: a few OS threads sharing one job queue (dirty work is the
/// exception, not the load — BEAM's dirty schedulers are similarly few).
/// Workers block on the shared receiver; the mutex is held only across the
/// dequeue, so jobs run concurrently.
fn offload_pool() -> &'static std::sync::Mutex<std::sync::mpsc::Sender<OffloadJob>> {
    use std::sync::{mpsc, Arc, Mutex, OnceLock};
    static POOL: OnceLock<Mutex<mpsc::Sender<OffloadJob>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<OffloadJob>();
        let rx = Arc::new(Mutex::new(rx));
        let n = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);
        let workers = (n / 4).max(2);
        for i in 0..workers {
            let rx = Arc::clone(&rx);
            std::thread::Builder::new()
                .name(format!("brood-offload-{i}"))
                .spawn(move || loop {
                    let job = rx.lock().expect("offload queue").recv();
                    match job {
                        Ok(j) => run_offload_job(j),
                        Err(_) => break,
                    }
                })
                .expect("spawn offload worker");
        }
        Mutex::new(tx)
    })
}

/// Run one job on a pool thread: rebuild the args in a private scratch heap,
/// call the native, ship the result (or the structured error) back as a
/// mailbox message. The scratch heap dies with the job — nothing is shared.
fn run_offload_job(job: OffloadJob) {
    let OffloadJob {
        func,
        args,
        sink,
        token,
    } = job;
    // Contain a *panic* in the native (an interpreter bug, not a Brood `Err`)
    // like the scheduler contains a panicking green process: without this, a
    // panic unwinds and kills the worker thread permanently, and — with only
    // ~nproc/4 workers — a couple of them drain the pool so every future
    // `offload` (incl. `nest fetch`'s `%git-clone`) hangs forever on its
    // `receive`. The per-job scratch heap is local to the closure, so a torn
    // heap is discarded either way. On a caught panic the caller gets a
    // structured `[:offload-error …]`, not silence.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut heap = Heap::new();
        let env = heap.new_env(None);
        let mut vals = Vec::with_capacity(args.len());
        for m in &args {
            vals.push(crate::process::from_message(&mut heap, m));
        }
        match func(&vals, env, &mut heap) {
            Ok(v) => match crate::process::to_message(&heap, v) {
                Ok(m) => offload_msg("offload", token, m),
                Err(e) => offload_msg("offload-error", token, crate::process::error_reason(&e)),
            },
            Err(e) => offload_msg("offload-error", token, crate::process::error_reason(&e)),
        }
    }));
    let msg = outcome.unwrap_or_else(|payload| {
        let detail = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        let e = LispError::runtime(format!(
            "offload: the native panicked (interpreter bug): {detail}"
        ));
        offload_msg("offload-error", token, crate::process::error_reason(&e))
    });
    sink.emit(msg);
}

fn offload_msg(tag: &str, token: i64, payload: crate::process::Message) -> crate::process::Message {
    use crate::process::Message;
    Message::Vector(vec![
        Message::Keyword(value::intern(tag)),
        Message::Int(token),
        payload,
    ])
}

/// `(%offload f args)` — run the allowed blocking native `f` with `args` (a
/// vector) on the offload pool. Returns a token int at once; the pool later
/// delivers `[:offload token result]` or `[:offload-error token err]` to the
/// calling process's mailbox. Policy lives in the prelude `offload` wrapper.
pub(super) fn offload_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (func, name) = match arg(args, 0) {
        Value::Native(id) => {
            let n = heap.native(id);
            (n.func, n.name.clone())
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%offload",
                "native function",
                other,
            ))
        }
    };
    if !OFFLOAD_ALLOWED.contains(&name.as_str()) {
        return Err(LispError::runtime(format!(
            "%offload: `{name}` is not offload-safe — only long/blocking data-in/data-out natives run on the pool (see (doc '%offload))"
        )));
    }
    let call_args: Vec<Value> = match arg(args, 1) {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil => Vec::new(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%offload",
                "vector of arguments",
                other,
            ))
        }
    };
    let mut msgs = Vec::with_capacity(call_args.len());
    for v in call_args {
        msgs.push(crate::process::to_message(heap, v)?);
    }
    let token = OFFLOAD_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (sink, _cell) = crate::process::sink_pair(crate::process::self_pid());
    let job = OffloadJob {
        func,
        args: msgs,
        sink,
        token,
    };
    let _ = offload_pool().lock().expect("offload queue").send(job);
    Ok(Value::Int(token))
}
