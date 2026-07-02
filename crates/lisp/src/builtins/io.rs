use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::printer;

use super::numeric::{arg, expect_int, expect_string};
use super::sequences::realize_seqviews;
use super::terminal::restore_terminal_on_exit;
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

/// Start capturing the current process's output into a fresh buffer. While active,
/// `print` / terminal output ([`write_term_bytes`]) appends there instead of real
/// stdout — and so does output from any process this one `spawn`s (the capture is
/// **process-scoped and inherited**, living in the process `Ctx`; see
/// `scheduler::begin_capture`). The `nest mcp` dispatcher installs one around each
/// `tools/call` so a handler's output — even a handler run in a spawned, killable
/// process under a timeout — can't corrupt the JSON-RPC stdout stream; the captured
/// text rides back in the result envelope. Pair with [`take_captured_stdout`].
pub fn begin_stdout_capture() {
    crate::process::begin_capture();
}

/// Stop capturing and return what was written since [`begin_stdout_capture`] —
/// `Some(text)` (possibly empty) if capture was active, `None` otherwise.
pub fn take_captured_stdout() -> Option<String> {
    crate::process::take_capture()
}

/// If a capture is active on the current process, append `s` to it and return
/// `true`; otherwise `false`. The single divert point shared by `print` and
/// `write_term_bytes`.
pub(super) fn capture_write(s: &str) -> bool {
    crate::process::capture_append(s)
}

/// `(%capture-begin)` — push a fresh output-capture buffer (see
/// [`begin_stdout_capture`]). The low half of the `with-out-str` macro; pairs with
/// `%capture-take`. Captures nest, so this composes with an outer MCP capture.
pub(super) fn capture_begin(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    begin_stdout_capture();
    Ok(Value::nil())
}

/// `(%capture-take)` — pop the current capture buffer and return its text as a
/// string (empty string if nothing was written), or `nil` if no capture was active
/// (see [`take_captured_stdout`]). The high half of the `with-out-str` macro.
pub(super) fn capture_take(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(match take_captured_stdout() {
        Some(s) => heap.alloc_string(&s),
        None => Value::nil(),
    })
}

pub(super) fn print(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let args = realize_seqviews(heap, env, args)?;
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    let text = parts.join(" ");
    // Divert to the capture buffer if one is active (the MCP channel must stay pure
    // JSON-RPC); otherwise write real stdout.
    let captured = capture_write(&text);
    if !captured {
        write_stdout(&text);
    }
    Ok(Value::nil())
}

/// Write `s` to real stdout the way a well-behaved Unix tool does. A **broken
/// pipe** (the downstream consumer closed — `brood … | head`) is not a program
/// error: the `print!` macro would panic on it with a Rust backtrace + crash
/// dump (every observed `failed printing to stdout: Broken pipe` crash bottoms
/// out here), so instead we restore the terminal and exit quietly, exactly as
/// the default SIGPIPE disposition would. Any other write/flush failure is
/// best-effort-dropped (matches the old `.flush().ok()`).
pub(super) fn write_stdout(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    if let Err(e) = out.write_all(s.as_bytes()).and_then(|_| out.flush()) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            restore_terminal_on_exit();
            std::process::exit(0);
        }
        // Other errors: nothing useful to do from a print primitive; drop it.
    }
}

pub(super) fn eprint(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let args = realize_seqviews(heap, env, args)?;
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    eprint!("{}", parts.join(" "));
    use std::io::Write;
    std::io::stderr().flush().ok();
    Ok(Value::nil())
}

/// `(%render & xs)` — the space-joined display forms of the arguments as a single
/// string (no output). The rendering half of `print`, split out so Brood's
/// `print`/`println` — which route the result through the dynamic `*out*` port —
/// hand a non-stdout sink (a buffer, a process) the exact text stdout would show.
pub(super) fn render(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let args = realize_seqviews(heap, env, args)?;
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    Ok(heap.alloc_string(&parts.join(" ")))
}

/// `(%write-out s)` — write the ready string `s` to the current stdout sink: the
/// active capture buffer if one is set (`with-out-str`, the MCP channel), else
/// real stdout. The write half of `print` and the default value of the `*out*`
/// port — keeping it the default is what lets `with-out-str` still capture
/// un-redirected output.
pub(super) fn write_out(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "%write-out", arg(args, 0))?;
    if !capture_write(&s) {
        write_stdout(&s);
    }
    Ok(Value::nil())
}

/// `(%write-err s)` — write the ready string `s` to real stderr (never captured,
/// matching `eprint`). The default value of the `*err*` port.
pub(super) fn write_err(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write;
    let s = expect_string(heap, "%write-err", arg(args, 0))?;
    eprint!("{}", s);
    std::io::stderr().flush().ok();
    Ok(Value::nil())
}

/// `(stdout-tty?)` — true when stdout is an interactive terminal, false when it's
/// captured (a pipe, a file, `cargo test`). The test framework uses this to emit
/// ANSI colour only when a human is watching, so captured output (what an LLM or
/// CI reads) stays clean plain text.
pub(super) fn stdout_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::boolean(std::io::stdout().is_terminal()))
}

/// `(stdin-tty?)` — true when stdin is an interactive terminal, false when it's
/// redirected (a pipe, a file). The REPL gates raw-mode line editing on this:
/// `echo … | brood` has a piped stdin (even with a TTY stdout), so it must take
/// the plain `read-line` path, not the interactive editor.
pub(super) fn stdin_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::boolean(std::io::stdin().is_terminal()))
}

// ---------- time ----------

/// `(now)` — wall-clock milliseconds since the Unix epoch, as an integer.
/// Subtract two readings to measure elapsed time (see `std/test.blsp`).
pub(super) fn now(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::int(ms))
}

/// `(now-ns)` — wall-clock nanoseconds since the Unix epoch, as an integer.
/// The fine-grained partner to `now`; subtract two readings to time sub-
/// millisecond work that `now`'s resolution would round to zero. (i64
/// nanoseconds since 1970 stays in range until the year 2262.)
pub(super) fn now_ns(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(Value::int(ns))
}

// ---------- memory ----------

/// `(mem-bytes)` — bytes currently allocated across the whole process.
pub(super) fn mem_bytes(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::live_bytes() as i64))
}

/// `(mem-peak)` — high-water mark of allocated bytes since the process started.
pub(super) fn mem_peak(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::peak_bytes() as i64))
}

/// `(gc-stats)` — a snapshot map of this process's garbage-collection activity
/// (Tier-1 observability; `docs/memory-review.md` §7). Per-process: it reports
/// the *calling* process's own LOCAL heap, never another's. Keys:
/// `:collections` (collections run since start — the automatic Stage-B
/// safepoint copies), `:copied` (cumulative LOCAL
/// objects relocated by those collections), `:reclaimed` (cumulative LOCAL
/// objects dropped), `:live` (LOCAL objects live right now), `:live-bytes` (a
/// cheap byte estimate of the LOCAL slabs — see `mem-bytes` for the process-wide
/// figure), and `:threshold` (the live count that triggers the next collection —
/// the slow/stable dial). Plus two figures for the *shared* RUNTIME code region
/// (the same for every process, not per-process): `:runtime-closures` (its total
/// promoted-closure count — grows with hot-reload churn, compacted back by the
/// safepoint, ADR-091) and `:runtime-threshold` (the count that triggers the next
/// auto-compaction). The live/reclaimable split is the expensive walk reported by
/// `(runtime-collect)`, so it's not included here.
#[cfg(feature = "dev-tools")]
pub(super) fn gc_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(gc_stats_map(heap))
}

/// Build the `(gc-stats)` snapshot map of the calling process's GC activity.
/// Shared by `gc-stats` and `gc-collect` (which reports the same shape *after*
/// forcing a collection, so the delta is visible).
#[cfg(feature = "dev-tools")]
pub(super) fn gc_stats_map(heap: &mut Heap) -> Value {
    let (runs, copied, reclaimed) = heap.gc_counters();
    let pairs = vec![
        (value::kw("collections"), Value::int(runs as i64)),
        (value::kw("copied"), Value::int(copied as i64)),
        (value::kw("reclaimed"), Value::int(reclaimed as i64)),
        (
            value::kw("live"),
            Value::int(heap.local_live_count() as i64),
        ),
        (
            value::kw("live-bytes"),
            Value::int(heap.local_bytes() as i64),
        ),
        (
            value::kw("threshold"),
            Value::int(heap.gc_threshold() as i64),
        ),
        // The shared RUNTIME code region (not per-process — every process sees the
        // same figure). `:runtime-closures` is its total promoted-closure count
        // (cheap — a slab length); it grows with hot-reload churn and the eval
        // safepoint compacts it back toward `:runtime-threshold` (single-process
        // today, ADR-091). The live/reclaimable split is the expensive walk reported
        // by `(runtime-collect)`'s `{:before :after :reclaimed}`, kept out of here.
        (
            value::kw("runtime-closures"),
            Value::int(heap.runtime_closure_count() as i64),
        ),
        (
            value::kw("runtime-threshold"),
            Value::int(heap.rt_gc_threshold() as i64),
        ),
        // True iff this binary was built with debug assertions (the GC tripwire /
        // verifier / poison bits are compiled in) — so a benchmark can confirm
        // it's measuring a clean release build, not a debug-armed one. `false`
        // for `make install` / `cargo build --release`.
        (
            value::kw("debug-build"),
            Value::boolean(cfg!(debug_assertions)),
        ),
    ];
    heap.map_from_pairs(pairs)
}

/// `(vm-stats)` — a snapshot map of the VM work-attribution counters (the
/// `perf-stats` feature; see `docs/benchmarking.md`). `:enabled` is `false` when
/// the binary was built without `--features perf-stats` (every other key absent —
/// the counters compiled to nothing). With the feature on: `:enabled true` plus a
/// key per counter (`:vm-apply`, `:tail-call`, `:self-tail`, `:tw-defer`,
/// `:call-ic-hit`/`:call-ic-miss`, `:global-ic-hit`/`:global-ic-miss`,
/// `:prim2-inline`/`:prim2-fallback`, `:prim1-inline`/`:prim1-fallback`,
/// `:env-get`, `:env-hops`, `:alloc`) — process-global cumulative totals across
/// every green process. The data behind the bytecode-lowering gate (ADR-096): is
/// the VM dispatch-, env-, or alloc-bound? A *counting* tool, not a timing one.
#[cfg(feature = "dev-tools")]
pub(super) fn vm_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pairs = match crate::perf::snapshot() {
        Some(counters) => {
            let mut v = Vec::with_capacity(counters.len() + 1);
            v.push((value::kw("enabled"), Value::boolean(true)));
            for (name, val) in counters {
                // counter idents are snake_case; expose idiomatic kebab keywords.
                v.push((value::kw(&name.replace('_', "-")), Value::int(val as i64)));
            }
            v
        }
        None => vec![(value::kw("enabled"), Value::boolean(false))],
    };
    Ok(heap.map_from_pairs(pairs))
}

/// `(runtime-collect)` — compact the shared RUNTIME code region now (reclaim
/// superseded hot-reload versions), returning `{:before :after :reclaimed :ran}`.
/// `:ran` is false (and nothing changes) when the runtime is shared with another
/// live process — see [`Heap::runtime_collect`]'s safety gate. Rarely needed: the
/// eval safepoint auto-compacts ([`Heap::maybe_runtime_collect`]) once churn
/// crosses the threshold; this is the explicit/force form.
#[cfg(feature = "dev-tools")]
pub(super) fn runtime_collect(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (before, after, ran) = match heap.runtime_collect() {
        Some((b, a)) => (b, a, true),
        None => {
            let n = heap.runtime_closure_count();
            (n, n, false)
        }
    };
    let pairs = vec![
        (value::kw("before"), Value::int(before as i64)),
        (value::kw("after"), Value::int(after as i64)),
        (value::kw("reclaimed"), Value::int((before - after) as i64)),
        (value::kw("ran"), Value::boolean(ran)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(gc-collect)` — force a collection of this process's LOCAL heap *now*,
/// returning the post-collection `(gc-stats)` map so the effect is visible.
/// An observability/test aid, **not** a load-bearing trigger: automatic
/// collection at the eval safepoint keeps memory bounded with no help from the
/// program (the removed `(hibernate)` was the load-bearing manual trigger — this
/// is not its return). Safe at any eval depth: a nullary builtin holds no
/// un-rooted LOCAL values across the collection, and every live ancestor frame
/// is already on the operand stack (ADR-061), so `collect` relocates everything
/// reachable and the freshly-built result map is allocated post-collection.
#[cfg(feature = "dev-tools")]
pub(super) fn gc_collect(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    heap.collect(&mut [], &mut []);
    Ok(gc_stats_map(heap))
}

/// `(gc-trace)` / `(gc-trace on?)` — query or set per-collection GC trace
/// logging for the calling process. With no argument, returns the current state;
/// with one, sets it (truthy = on) and returns the new state. When on, each
/// minor/major collection prints a one-line summary to stderr. Per-process and
/// defaulted from the `BROOD_GC_TRACE` env var (which traces the whole run,
/// including the root process before any `(gc-trace)` call).
#[cfg(feature = "dev-tools")]
pub(super) fn gc_trace(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let Some(&v) = args.first() {
        heap.set_gc_trace(crate::eval::truthy(v));
    }
    Ok(Value::boolean(heap.gc_trace()))
}

/// `(mem-limit)` — the hard memory ceiling in bytes (0 = unlimited). ADR-043.
pub(super) fn mem_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::hard_limit() as i64))
}

/// `(mem-soft-limit)` — the soft memory ceiling in bytes (0 = unlimited). ADR-043.
pub(super) fn mem_soft_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::soft_limit() as i64))
}

// ---------- TCP sockets (ADR-062) ----------
//
// Thin non-blocking mechanism over `crate::net`; the active-socket / framing /
// HTTP policy is Brood (std/tcp.blsp). A socket is `Value::Socket(id)`.

pub(super) fn expect_socket(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "socket",
        Value::Socket(id) => id,
    )
}

// ---------- in-memory shared table (Brood's ETS, ADR-107) ----------
// A `Value::Table(id)` handle; the store lives in `crate::table`. These builtins are
// thin wrappers — all the storage / locking / clone-in-clone-out lives there.

pub(super) fn expect_table(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "table",
        Value::Table(id) => id,
    )
}

pub(super) fn table_new(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::table(crate::table::create()))
}

pub(super) fn table_put(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-put", arg(args, 0))?;
    crate::table::check_key("table-put", arg(args, 1))?;
    crate::table::put(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-get", arg(args, 0))?;
    crate::table::check_key("table-get", arg(args, 1))?;
    crate::table::get(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_has(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-has?", arg(args, 0))?;
    crate::table::check_key("table-has?", arg(args, 1))?;
    Ok(Value::boolean(crate::table::has(heap, id, arg(args, 1))?))
}

pub(super) fn table_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-delete", arg(args, 0))?;
    crate::table::check_key("table-delete", arg(args, 1))?;
    crate::table::delete(heap, id, arg(args, 1))
}

pub(super) fn table_incr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-incr", arg(args, 0))?;
    crate::table::check_key("table-incr", arg(args, 1))?;
    let delta = match arg(args, 2) {
        Value::Nil => 1, // (table-incr t k) defaults the delta to 1
        v => expect_int(heap, "table-incr", v)?,
    };
    crate::table::incr(heap, id, arg(args, 1), delta)
}

pub(super) fn table_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-count", arg(args, 0))?;
    Ok(Value::int(crate::table::count(id)?))
}

pub(super) fn table_snapshot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-snapshot", arg(args, 0))?;
    crate::table::snapshot(heap, id)
}

pub(super) fn table_drop(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-drop", arg(args, 0))?;
    Ok(Value::boolean(crate::table::drop_table(id)))
}

pub(super) fn socket_port(who: &str, p: i64) -> Result<u16, LispError> {
    u16::try_from(p)
        .map_err(|_| LispError::runtime(format!("{}: port {} out of range 0..=65535", who, p)))
}

pub(super) fn tcp_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tcp-connect", arg(args, 0))?;
    let port = socket_port(
        "tcp-connect",
        expect_int(heap, "tcp-connect", arg(args, 1))?,
    )?;
    let owner = crate::process::self_pid();
    match crate::net::connect(&host, port, owner) {
        Ok(id) => Ok(Value::socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tcp-connect {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

pub(super) fn tcp_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tcp-listen", arg(args, 0))?;
    let port = socket_port("tcp-listen", expect_int(heap, "tcp-listen", arg(args, 1))?)?;
    let owner = crate::process::self_pid();
    match crate::net::listen(&host, port, owner) {
        Ok(id) => Ok(Value::socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tcp-listen {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

pub(super) fn tls_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tls-listen", arg(args, 0))?;
    let port = socket_port("tls-listen", expect_int(heap, "tls-listen", arg(args, 1))?)?;
    let cert = expect_string(heap, "tls-listen", arg(args, 2))?;
    let key = expect_string(heap, "tls-listen", arg(args, 3))?;
    let owner = crate::process::self_pid();
    match crate::net::tls_listen(&host, port, &cert, &key, owner) {
        Ok(id) => Ok(Value::socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tls-listen {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

pub(super) fn tls_self_signed(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tls-self-signed", arg(args, 0))?.to_string();
    match crate::net::tls_self_signed(vec![host]) {
        Ok((cert, key)) => {
            let c = heap.alloc_string(&cert);
            let k = heap.alloc_string(&key);
            Ok(heap.alloc_vector(vec![c, k]))
        }
        Err(e) => Err(LispError::runtime(format!("tls-self-signed: {}", e))),
    }
}

pub(super) fn tls_request(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "tls-request", arg(args, 0))?;
    let port = socket_port(
        "tls-request",
        expect_int(heap, "tls-request", arg(args, 1))?,
    )?;
    let request = expect_string(heap, "tls-request", arg(args, 2))?;
    let owner = crate::process::self_pid();
    let id = crate::net::tls_request(&host, port, request.to_string(), owner);
    Ok(Value::socket(id))
}

/// Lower a tcp-send/proc-send payload to raw bytes. A `bytes` value is written
/// verbatim. A string is UTF-8 in text mode; in a **binary**-mode socket/child it
/// is the Latin-1 byte-string form (codepoints 0–255) — kept for back-compat with
/// callers that build byte-strings, alongside the preferred `bytes` value.
fn send_payload(
    heap: &Heap,
    who: &str,
    kind: &str,
    v: Value,
    binary: bool,
) -> Result<Vec<u8>, LispError> {
    match v {
        Value::Bytes(b) => Ok(heap.bytes(b).as_bytes().to_vec()),
        Value::Str(_) => {
            let s = expect_string(heap, who, v)?;
            if binary {
                let mut out = Vec::with_capacity(s.len());
                for c in s.chars() {
                    let n = c as u32;
                    if n > 0xFF {
                        return Err(LispError::runtime(format!(
                            "{who}: codepoint U+{n:04X} is not a byte (0–255); a binary-mode {kind} sends raw bytes (a `bytes` value or a 0–255 codepoint string)",
                        )));
                    }
                    out.push(n as u8);
                }
                Ok(out)
            } else {
                Ok(s.into_bytes())
            }
        }
        other => Err(LispError::wrong_type(heap, who, "bytes or string", other)),
    }
}

pub(super) fn tcp_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-send", arg(args, 0))?;
    let out = send_payload(
        heap,
        "tcp-send",
        "socket",
        arg(args, 1),
        crate::net::is_binary(id),
    )?;
    crate::net::send(id, &out).map_err(|e| LispError::runtime(format!("tcp-send: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-set-binary", arg(args, 0))?;
    let on = !matches!(arg(args, 1), Value::Nil | Value::Bool(false));
    crate::net::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("tcp-set-binary: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_controlling_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-controlling-process", arg(args, 0))?;
    let pid = match arg(args, 1) {
        Value::Pid { id, .. } => id,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "tcp-controlling-process",
                "pid",
                other,
            ))
        }
    };
    crate::net::controlling_process(id, pid)
        .map_err(|e| LispError::runtime(format!("tcp-controlling-process: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-close", arg(args, 0))?;
    crate::net::close(id);
    Ok(Value::nil())
}

pub(super) fn tcp_local_port(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "tcp-local-port", arg(args, 0))?;
    Ok(crate::net::local_port(id)
        .map(|p| Value::int(p as i64))
        .unwrap_or(Value::nil()))
}

// ----- persistent child processes (ADR-104) ----------------------------------
//
// Thin mechanism over `crate::proc`: spawn a long-lived child with piped stdio,
// write its stdin, and receive its output as `[:proc …]` mailbox messages. The
// framing/protocol policy (e.g. JSON-RPC for an LSP client) is Brood. A child is
// `Value::Subprocess(id)`. Contrast `%os-cmd`/`run-process`, which run to exit.

pub(super) fn expect_subprocess(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "subprocess",
        Value::Subprocess(id) => id,
    )
}

pub(super) fn proc_spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "proc-spawn", arg(args, 0))?;
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        argv.push(expect_string(heap, "proc-spawn", a)?);
    }
    // Optional 3rd argument: an options map `{:cwd "dir" :env {"K" "V" …}}`.
    let mut cwd: Option<String> = None;
    let mut env: Vec<(String, String)> = Vec::new();
    if let Value::Map(opts) = arg(args, 2) {
        if let Some(v) = heap.map_get(opts, Value::keyword(value::intern("cwd"))) {
            if !matches!(v, Value::Nil) {
                cwd = Some(expect_string(heap, "proc-spawn :cwd", v)?);
            }
        }
        if let Some(Value::Map(e)) = heap.map_get(opts, Value::keyword(value::intern("env"))) {
            for (k, v) in heap.map_entries(e) {
                env.push((
                    expect_string(heap, "proc-spawn :env key", k)?,
                    expect_string(heap, "proc-spawn :env value", v)?,
                ));
            }
        }
    }
    let owner = crate::process::self_pid();
    match crate::proc::spawn(&prog, &argv, cwd.as_deref(), &env, owner) {
        Ok(id) => Ok(Value::subprocess(id)),
        Err(e) => Err(LispError::runtime(format!("proc-spawn {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)),
    }
}

pub(super) fn proc_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-send", arg(args, 0))?;
    let out = send_payload(
        heap,
        "proc-send",
        "subprocess",
        arg(args, 1),
        crate::proc::is_binary(id),
    )?;
    crate::proc::send(id, &out).map_err(|e| LispError::runtime(format!("proc-send: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-set-binary", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::proc::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("proc-set-binary: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-close", arg(args, 0))?;
    crate::proc::close(id);
    Ok(Value::nil())
}

// ----- terminal frontend (ADR-046) -------------------------------------------
//
// The thin crossterm seam: enter/leave the alternate screen, read keys, and
// paint a *frame* — a Brood vector of render ops. The protocol's meaning is
// data (the ops); these primitives are the in-process frontend that interprets
// it, so a remote/web frontend can implement the identical op vocabulary later.
// Errors surface as clean `LispError`s (never a crossterm panic), mirroring the
// rope primitives' discipline.

pub(super) fn mailbox_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            Ok(crate::process::mailbox_len(id)
                .map(|n| Value::int(n as i64))
                .unwrap_or(Value::nil()))
        }
        Value::Pid { .. } => Ok(Value::nil()),
        other => Err(LispError::wrong_type(heap, "mailbox-size", "pid", other)),
    }
}

/// `(process-info pid)` — a snapshot map of a **live local** process, or `nil`
/// for a remote/dead pid (a non-pid is a type error). The fields are all
/// kernel-internal, so the map is assembled here from the registry / scheduler /
/// name / monitor tables (ADR-051):
///
///   `{:id <int> :node <kw> :name <kw|nil> :status <kw> :mailbox <int>
///     :monitored-by <int> :parent <int|nil>}`
///
/// `:status` is `:running` / `:waiting` (parked in `receive`). `:name` is the
/// registered name or nil. `:parent` is the spawner's id (nil for the root).
/// `:memory` (per-process bytes) joins once the kernel tracks it, and `:status`
/// sharpens when an explicit state enum lands (the observer tolerates the gap).
/// Each accessor takes one lock independently, so no two are held at once.
pub(super) fn process_info(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Dead/unknown pid → nil (matches `mailbox-size`).
            if !crate::process::is_alive(id) {
                return Ok(Value::nil());
            }
            let name = crate::dist::name_for_pid(id)
                .map(Value::Keyword)
                .unwrap_or(Value::nil());
            let status = crate::process::process_status(id)
                .map(value::kw)
                .unwrap_or(Value::nil());
            let mailbox = Value::int(crate::process::mailbox_len(id).unwrap_or(0) as i64);
            let monitored = Value::int(crate::process::monitored_by(id) as i64);
            // `:parent` is the spawner's id, or nil for the root.
            let parent = crate::process::parent_of(id)
                .map(|p| Value::int(p as i64))
                .unwrap_or(Value::nil());
            // `:memory` — the process's LOCAL heap footprint (bytes), published on
            // its last `receive`; 0 for a process that has never received.
            let memory = Value::int(crate::process::process_mem(id).unwrap_or(0) as i64);
            // `:collections` — the process's cumulative GC count, republished on
            // its last `receive` (0 for one that has never received). The signal
            // for "is this process churning memory?" in the observer.
            let collections = Value::int(crate::process::process_gc_runs(id).unwrap_or(0) as i64);
            // `:reductions` — the process's cumulative reduction count (Erlang's
            // scheduling unit), updated every scheduling quantum. The observer's
            // "is this process doing work / busy?" signal. Exact for spawned
            // processes; coarse (whole-budget increments) for the root.
            let reductions = Value::int(crate::process::process_reductions(id).unwrap_or(0) as i64);
            let pairs = vec![
                (value::kw("id"), Value::int(id as i64)),
                // The process's actual pid value (not just its numeric id), so a
                // caller — e.g. the observer's kill command — can act on the
                // process directly with `exit`/`send`/`monitor`.
                (value::kw("pid"), Value::pid(node, id)),
                (value::kw("node"), Value::keyword(node)),
                (value::kw("name"), name),
                (value::kw("status"), status),
                (value::kw("mailbox"), mailbox),
                (value::kw("monitored-by"), monitored),
                (value::kw("parent"), parent),
                (value::kw("memory"), memory),
                (value::kw("collections"), collections),
                (value::kw("reductions"), reductions),
            ];
            Ok(heap.map_from_pairs(pairs))
        }
        Value::Pid { .. } => Ok(Value::nil()),
        other => Err(LispError::wrong_type(heap, "process-info", "pid", other)),
    }
}

/// `(string->number s)` — parse `s` as an integer if it is one, else as a float,
/// else `nil`. The inverse of `number->string`. A robust parse-or-nil can't be
/// expressed over `read-string` (which would read `"3abc"` as `3` and stop), so
/// the strict parse is a primitive. Surrounding whitespace is not accepted —
/// `trim` first if the input may carry any.

pub(super) fn string_to_number(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->number", arg(args, 0))?;
    if let Ok(i) = s.parse::<i64>() {
        Ok(Value::int(i))
    } else if let Ok(n) = s.parse::<num_bigint::BigInt>() {
        // An integer too big for i64 is a bignum — mirroring the reader's
        // over-range literal path — NOT a lossy f64 (which silently rounded
        // `(number->string big)` away from round-tripping, kernel audit).
        // Reaching here means the i64 parse failed, so `n` is out of range
        // and `alloc_bigint`'s no-demotion invariant holds.
        Ok(heap.alloc_bigint(n))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(Value::float(f))
    } else {
        Ok(Value::nil())
    }
}

// ---------- filesystem ----------
// Mechanism only: existence / directory reflection so the Brood module system and
// the project test runner can resolve load paths and discover test files. Path
// manipulation and all policy live in Brood (`std/prelude.blsp`, `std/project.blsp`).

/// `(cwd)` — the process's current working directory as a string.
pub(super) fn cwd(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_dir() {
        Ok(p) => Ok(heap.alloc_string(&p.to_string_lossy())),
        Err(e) => {
            Err(LispError::runtime(format!("cwd: {}", e))
                .with_code(crate::error::error_codes::FILE_IO))
        }
    }
}

/// `(file-exists? path)` — true if a file or directory exists at `path`.
pub(super) fn file_exists(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-exists?", arg(args, 0))?;
    Ok(Value::boolean(std::path::Path::new(&path).exists()))
}

/// `(dir? path)` — true if `path` exists and is a directory.
pub(super) fn is_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "dir?", arg(args, 0))?;
    Ok(Value::boolean(std::path::Path::new(&path).is_dir()))
}

/// `(list-dir path)` — the entry names (not full paths) directly under a
/// directory, sorted for determinism. Errors if `path` isn't a readable directory.
pub(super) fn list_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "list-dir", arg(args, 0))?;
    let mut names: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) => {
            return Err(LispError::runtime(format!("list-dir: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO))
        }
    };
    names.sort();
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(heap.alloc_string(n));
    }
    Ok(heap.list(items))
}

/// `(make-dir path)` — create `path` and any missing parents (like `mkdir -p`).
/// Returns nil. Used by the project scaffolder (`nest new`).
pub(super) fn make_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "make-dir", arg(args, 0))?;
    std::fs::create_dir_all(&path).map_err(|e| {
        LispError::runtime(format!("make-dir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(spit path content)` — write `content` (a string) to `path`, replacing any
/// existing file. Returns nil. The write-side counterpart to `load`.
pub(super) fn spit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let path = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string path", pv)),
    };
    let cv = arg(args, 1);
    let content = match cv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string content", cv)),
    };
    std::fs::write(&path, content).map_err(|e| {
        LispError::runtime(format!("spit: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(spit-bytes path bytes)` — write a byte sequence (a `bytes` value, a vector,
/// or a list of byte ints 0–255) to `path` byte-faithfully, replacing any
/// existing file. Returns nil. The binary write-side counterpart to `slurp-bytes`:
/// `spit` is UTF-8 string-only and would reject (or corrupt) raw bytes, so this is
/// what materialises a received image / archive / any binary asset to disk.
pub(super) fn spit_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "spit-bytes", arg(args, 0))?;
    let bytes = collect_bytes("spit-bytes", arg(args, 1), heap)?;
    std::fs::write(&path, &bytes).map_err(|e| {
        LispError::runtime(format!("spit-bytes: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// Hash algorithm selector for `%digest` / `%hmac`, decoded from the leading
/// keyword arg. This is the single place the kernel enumerates digest
/// algorithms; all string-input and hex-output shaping is Brood policy in
/// `std/hash.blsp` (over `string->utf8-bytes` and `bytes->hex`).
#[derive(Clone, Copy)]
enum HashAlgo {
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

fn hash_algo(name: &'static str, kw: Value, heap: &mut Heap) -> Result<HashAlgo, LispError> {
    let sym = match kw {
        Value::Keyword(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                name,
                "algorithm keyword",
                other,
            ))
        }
    };
    match value::symbol_name(sym).as_str() {
        "md5" => Ok(HashAlgo::Md5),
        "sha1" => Ok(HashAlgo::Sha1),
        "sha256" => Ok(HashAlgo::Sha256),
        "sha384" => Ok(HashAlgo::Sha384),
        "sha512" => Ok(HashAlgo::Sha512),
        other => Err(LispError::runtime(format!(
            "{name}: unknown algorithm :{other} (want :md5 :sha1 :sha256 :sha384 :sha512)"
        ))),
    }
}

/// `(%digest algo bytes)` — raw digest of a byte sequence (`bytes` value, vector,
/// or list of byte ints) under algorithm keyword `algo`, returned as a bytes
/// value. The single digest primitive: string-input and hex-output variants are
/// Brood wrappers in `std/hash.blsp` (collapsed the former 15 `%sha*`/`%md5`
/// prims to this one — ADR-006 / dogfooding).
pub(super) fn digest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let algo = hash_algo("%digest", arg(args, 0), heap)?;
    let bytes = collect_bytes("%digest", arg(args, 1), heap)?;
    let out: Vec<u8> = match algo {
        HashAlgo::Md5 => {
            use md5::{Digest, Md5};
            Md5::digest(&bytes).to_vec()
        }
        HashAlgo::Sha1 => {
            use sha1::{Digest, Sha1};
            Sha1::digest(&bytes).to_vec()
        }
        HashAlgo::Sha256 => {
            use sha2::{Digest, Sha256};
            Sha256::digest(&bytes).to_vec()
        }
        HashAlgo::Sha384 => {
            use sha2::{Digest, Sha384};
            Sha384::digest(&bytes).to_vec()
        }
        HashAlgo::Sha512 => {
            use sha2::{Digest, Sha512};
            Sha512::digest(&bytes).to_vec()
        }
    };
    Ok(bytes_to_value(&out, heap))
}

/// `(%hmac algo key-bytes msg-bytes)` — HMAC of `msg-bytes` keyed by `key-bytes`
/// (both byte sequences) under algorithm keyword `algo`, returned as a bytes
/// value. String-keyed / hex-output variants are Brood wrappers in
/// `std/hash.blsp` (collapsed the former 6 `%hmac-*` prims to this one).
pub(super) fn hmac(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    let algo = hash_algo("%hmac", arg(args, 0), heap)?;
    let key = collect_bytes("%hmac", arg(args, 1), heap)?;
    let msg = collect_bytes("%hmac", arg(args, 2), heap)?;
    let mac_err = |e| LispError::runtime(format!("%hmac: {e}"));
    let out: Vec<u8> = match algo {
        HashAlgo::Md5 => {
            use md5::Md5;
            let mut mac = Hmac::<Md5>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha1 => {
            use sha1::Sha1;
            let mut mac = Hmac::<Sha1>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha256 => {
            use sha2::Sha256;
            let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha384 => {
            use sha2::Sha384;
            let mut mac = Hmac::<Sha384>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha512 => {
            use sha2::Sha512;
            let mut mac = Hmac::<Sha512>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
    };
    Ok(bytes_to_value(&out, heap))
}

/// Extract raw bytes from a `Value`: a `bytes` value, or (leniently) a vector
/// or list of byte ints (0–255).
pub(super) fn collect_bytes(
    name: &'static str,
    bv: Value,
    heap: &mut Heap,
) -> Result<Vec<u8>, LispError> {
    match bv {
        Value::Bytes(id) => Ok(heap.bytes(id).as_bytes().to_vec()),
        Value::Vector(id) => {
            let vec = heap.vector(id).to_vec();
            vec.iter()
                .map(|v| match v {
                    Value::Int(n) if *n >= 0 && *n <= 255 => Ok(*n as u8),
                    other => Err(LispError::wrong_type(
                        heap,
                        name,
                        "byte int (0-255)",
                        *other,
                    )),
                })
                .collect::<Result<Vec<u8>, LispError>>()
        }
        Value::Pair(_) | Value::Nil => {
            let mut out = Vec::new();
            let mut cur = bv;
            loop {
                match cur {
                    Value::Nil => break,
                    Value::Pair(id) => {
                        let (h, t) = heap.pair(id);
                        match h {
                            Value::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
                            other => {
                                return Err(LispError::wrong_type(
                                    heap,
                                    name,
                                    "byte int (0-255)",
                                    other,
                                ))
                            }
                        }
                        cur = t;
                    }
                    other => return Err(LispError::wrong_type(heap, name, "proper list", other)),
                }
            }
            Ok(out)
        }
        other => Err(LispError::wrong_type(heap, name, "vector or list", other)),
    }
}

/// Allocate a raw-byte result (digest, HMAC, derived key) as a Brood `bytes`
/// value — the raw-byte counterpart of the Brood `bytes->hex` shaping. The byte-oriented
/// crypto layer (store-driver findings 2/3) returns these so digests can be
/// chained over bytes without a hex round-trip at each step.
pub(super) fn bytes_to_value(bytes: impl AsRef<[u8]>, heap: &mut Heap) -> Value {
    heap.alloc_bytes(crate::core::blob::SharedBlob::new(bytes.as_ref()))
}

/// Run `git` with `args` (optionally in `cwd`), capturing stdout+stderr. The
/// shared mechanism behind the package manager's git primitives (ADR-037).
pub(super) fn run_git(args: &[&str], cwd: Option<&str>) -> Result<std::process::Output, LispError> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.output().map_err(|e| {
        LispError::runtime(format!("git {}: {}", args.join(" "), e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
            .with_hint("is `git` installed and on PATH?")
    })
}

/// Run a `git` subcommand that's expected to succeed; turn a non-zero exit into a
/// `LispError` carrying git's stderr.
pub(super) fn git_or_err(args: &[&str], cwd: Option<&str>) -> Result<(), LispError> {
    let out = run_git(args, cwd)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(LispError::runtime(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED))
    }
}

/// `(%random-bytes n)` — `n` cryptographically-strong random bytes as a Brood
/// bytes value. Useful for generating keys, nonces, and salts.
pub(super) fn random_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "%random-bytes", arg(args, 0))?;
    if !(0..=65536).contains(&n) {
        return Err(LispError::runtime(
            "%random-bytes: byte count must be in 0..=65536",
        ));
    }
    let mut bytes = vec![0u8; n as usize];
    getrandom::fill(&mut bytes)
        .map_err(|e| LispError::runtime(format!("%random-bytes: OS RNG unavailable: {e}")))?;
    Ok(bytes_to_value(&bytes, heap))
}

/// `(%chacha20-encrypt key-bytes nonce-bytes plaintext-bytes)` — authenticated
/// encryption (ChaCha20-Poly1305). `key-bytes` must be exactly 32 bytes;
/// `nonce-bytes` must be exactly 12 bytes. Returns the ciphertext (plaintext
/// length + 16-byte Poly1305 authentication tag) as a byte vector.
///
/// **NEVER reuse a (key, nonce) pair.** A fresh nonce is required per message —
/// reuse breaks both confidentiality *and* the Poly1305 integrity guarantee.
/// Nonce generation is the caller's responsibility (see `crypto/random-nonce`).
pub(super) fn chacha20_encrypt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
    let key_bytes = collect_bytes("%chacha20-encrypt", arg(args, 0), heap)?;
    let nonce_bytes = collect_bytes("%chacha20-encrypt", arg(args, 1), heap)?;
    let plaintext = collect_bytes("%chacha20-encrypt", arg(args, 2), heap)?;
    if key_bytes.len() != 32 {
        return Err(LispError::runtime(format!(
            "%chacha20-encrypt: key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    if nonce_bytes.len() != 12 {
        return Err(LispError::runtime(format!(
            "%chacha20-encrypt: nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        )));
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| LispError::runtime(format!("%chacha20-encrypt: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_slice())
        .map_err(|e| LispError::runtime(format!("%chacha20-encrypt: {e}")))?;
    Ok(bytes_to_value(&ciphertext, heap))
}

/// `(%chacha20-decrypt key-bytes nonce-bytes ciphertext-bytes)` — authenticated
/// decryption (ChaCha20-Poly1305). Returns the plaintext as a byte vector, or
/// `:error` if the authentication tag fails (tampered or wrong key/nonce).
pub(super) fn chacha20_decrypt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
    let key_bytes = collect_bytes("%chacha20-decrypt", arg(args, 0), heap)?;
    let nonce_bytes = collect_bytes("%chacha20-decrypt", arg(args, 1), heap)?;
    let ciphertext = collect_bytes("%chacha20-decrypt", arg(args, 2), heap)?;
    if key_bytes.len() != 32 {
        return Err(LispError::runtime(format!(
            "%chacha20-decrypt: key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    if nonce_bytes.len() != 12 {
        return Err(LispError::runtime(format!(
            "%chacha20-decrypt: nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        )));
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| LispError::runtime(format!("%chacha20-decrypt: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.decrypt(nonce, ciphertext.as_slice()) {
        Ok(plaintext) => Ok(bytes_to_value(&plaintext, heap)),
        Err(_) => Ok(Value::keyword(value::intern("error"))),
    }
}

/// `(%pbkdf2-sha256-bytes password-bytes salt-bytes iterations key-len)` — derive
/// a key from a password using PBKDF2-HMAC-SHA256 (RFC 2898). `password-bytes`
/// and `salt-bytes` are byte vectors (raw bytes, not UTF-8-decoded strings — so
/// a base64-decoded binary salt round-trips faithfully, store-driver finding #4).
/// Returns a bytes value of `key-len` bytes. Use `iterations` ≥ 600,000 for
/// password storage (NIST SP 800-132 2023). Implemented over the `hmac` + `sha2`
/// crates — microseconds where the pure-Brood version cost ~2s/connection (#5).
pub(super) fn pbkdf2_sha256_fn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let pw = collect_bytes("%pbkdf2-sha256-bytes", arg(args, 0), heap)?;
    let salt = collect_bytes("%pbkdf2-sha256-bytes", arg(args, 1), heap)?;
    let iterations = expect_int(heap, "%pbkdf2-sha256-bytes", arg(args, 2))?;
    let key_len = expect_int(heap, "%pbkdf2-sha256-bytes", arg(args, 3))?;
    if iterations <= 0 {
        return Err(LispError::runtime(
            "%pbkdf2-sha256-bytes: iterations must be positive",
        ));
    }
    if !(1..=512).contains(&key_len) {
        return Err(LispError::runtime(
            "%pbkdf2-sha256-bytes: key-len must be in 1..=512",
        ));
    }
    let hlen = 32usize; // SHA-256 output bytes
    let block_count = (key_len as usize).div_ceil(hlen);
    let mut dk = Vec::with_capacity(key_len as usize);
    for i in 1u32..=(block_count as u32) {
        // U_1 = HMAC(password, salt || INT(i))
        let mut mac = HmacSha256::new_from_slice(&pw)
            .map_err(|e| LispError::runtime(format!("%pbkdf2-sha256-bytes: {e}")))?;
        mac.update(&salt);
        mac.update(&i.to_be_bytes());
        let mut u: Vec<u8> = mac.finalize().into_bytes().to_vec();
        let mut t = u.clone();
        // U_n = HMAC(password, U_{n-1}); T_i = XOR of all U_j
        for _ in 1..(iterations as u32) {
            let mut mac2 = HmacSha256::new_from_slice(&pw)
                .map_err(|e| LispError::runtime(format!("%pbkdf2-sha256-bytes: {e}")))?;
            mac2.update(&u);
            u = mac2.finalize().into_bytes().to_vec();
            for j in 0..hlen {
                t[j] ^= u[j];
            }
        }
        dk.extend_from_slice(&t);
    }
    dk.truncate(key_len as usize);
    Ok(bytes_to_value(&dk, heap))
}

/// `(%git-resolve-ref url ref)` — resolve `ref` (a tag, branch, or commit) at the
/// remote `url` to a full commit hash via `git ls-remote`, or `nil` if no such
/// ref exists. For an annotated tag, prefers the peeled `^{}` line (the commit the
/// tag points to). When `ref` is already a commit SHA the remote doesn't advertise
/// (ls-remote returns nothing), it's returned as-is — a commit pins itself.
/// The package manager's ref-pinning mechanism (ADR-037); pinning policy is Brood.
pub(super) fn git_resolve_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-resolve-ref", arg(args, 0))?;
    let r = expect_string(heap, "%git-resolve-ref", arg(args, 1))?;
    let out = run_git(&["ls-remote", &url, &r], None)?;
    if !out.status.success() {
        return Err(LispError::runtime(format!(
            "%git-resolve-ref: git ls-remote {} {} failed: {}",
            url,
            r,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut first: Option<&str> = None;
    let mut peeled: Option<&str> = None;
    for line in stdout.lines() {
        let sha = line.split_whitespace().next();
        if first.is_none() {
            first = sha;
        }
        if line.trim_end().ends_with("^{}") {
            peeled = sha;
        }
    }
    if let Some(s) = peeled.or(first) {
        return Ok(heap.alloc_string(s));
    }
    // No advertised ref: if `ref` itself looks like a commit SHA, it pins itself.
    let looks_like_sha = r.len() >= 7 && r.len() <= 40 && r.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_sha {
        Ok(heap.alloc_string(&r))
    } else {
        Ok(Value::nil())
    }
}

/// `(%git-clone url dest ref commit)` — populate `dest` with a shallow clone of
/// `url` checked out at the exact `commit` (detached HEAD). Tries to fetch the
/// commit directly (servers that allow SHA-in-want, e.g. GitHub); falls back to
/// fetching `ref` then checking out `commit`. Returns `:ok`, or throws with git's
/// stderr. The package manager's fetch mechanism (ADR-037); the cache layout and
/// when-to-reclone policy are Brood (std/package.blsp).
pub(super) fn git_clone(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let url = expect_string(heap, "%git-clone", arg(args, 0))?;
    let dest = expect_string(heap, "%git-clone", arg(args, 1))?;
    let gref = expect_string(heap, "%git-clone", arg(args, 2))?;
    let commit = expect_string(heap, "%git-clone", arg(args, 3))?;

    if let Some(parent) = std::path::Path::new(&dest).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                LispError::runtime(format!(
                    "%git-clone: cannot create {}: {}",
                    parent.display(),
                    e
                ))
                .with_code(crate::error::error_codes::FILE_IO)
            })?;
        }
    }

    git_or_err(&["init", "-q", &dest], None)?;
    git_or_err(&["-C", &dest, "remote", "add", "origin", &url], None)?;

    // Fast path: fetch the exact commit shallowly. Many servers (GitHub) allow it.
    let direct = run_git(
        &[
            "-C", &dest, "fetch", "-q", "--depth", "1", "origin", &commit,
        ],
        None,
    )?;
    if !direct.status.success() {
        // Fallback: fetch the named ref (shallow first, then full if the server
        // rejects a shallow ref fetch), which must contain the locked commit.
        if git_or_err(
            &["-C", &dest, "fetch", "-q", "--depth", "1", "origin", &gref],
            None,
        )
        .is_err()
        {
            git_or_err(&["-C", &dest, "fetch", "-q", "origin", &gref], None)?;
        }
    }

    if git_or_err(&["-C", &dest, "checkout", "-q", "--detach", &commit], None).is_err() {
        return Err(LispError::runtime(format!(
            "%git-clone: commit {} is not reachable from {} at {}",
            commit, gref, url
        ))
        .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
        .with_hint("the ref may have moved since it was locked — try `nest update`"));
    }
    Ok(crate::core::value::kw("ok"))
}

/// `(%rm-rf path)` — recursively delete `path`. **Bounded to `_deps/`**: refuses
/// any path without a `_deps` component, so a mis-computed cache path can't delete
/// something outside the package cache. Idempotent (`:ok` if already absent). The
/// package manager's cache-eviction mechanism (ADR-037); `nest update` re-clones.
pub(super) fn rm_rf(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%rm-rf", arg(args, 0))?;
    let under_deps = std::path::Path::new(&path)
        .components()
        .any(|c| c.as_os_str() == "_deps");
    if !under_deps {
        return Err(LispError::runtime(format!(
            "%rm-rf: refusing to delete {} — only paths under _deps/ may be removed",
            path
        ))
        .with_code(crate::error::error_codes::FILE_IO));
    }
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(crate::core::value::kw("ok")),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(crate::core::value::kw("ok")),
        Err(e) => Err(LispError::runtime(format!("%rm-rf: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(read-line)` — read one line from stdin, returning it as a string with the
/// trailing newline stripped, or `nil` at end of input (EOF / Ctrl-D). The one
/// irreducible I/O mechanism the Brood-hosted REPL (`std/repl.blsp`) can't
/// bootstrap; line *editing* on a TTY comes free from the terminal's cooked
/// mode, so this stays a plain blocking read.
pub(super) fn read_line(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::BufRead;
    let mut line = String::new();
    let n = std::io::stdin().lock().read_line(&mut line).map_err(|e| {
        LispError::runtime(format!("read-line: {}", e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    if n == 0 {
        return Ok(Value::nil()); // EOF
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(heap.alloc_string(&line))
}

/// `(slurp path)` — read the whole file at `path` and return it as a string. The
/// read-side counterpart to `spit`; unlike `load` it does not evaluate, so the
/// doc tooling can inspect a module's source (e.g. its leading docstring form).
pub(super) fn slurp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "slurp", arg(args, 0))?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("slurp: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(heap.alloc_string(&content))
}

/// `(slurp-bytes path)` — read the whole file at `path` as a bytes value. The
/// byte-faithful read `slurp` can't be: `slurp` is UTF-8 and throws
/// on a non-text file, whereas this reads any bytes (images, archives, a binary
/// asset to hash via `hash/sha256-bytes`). Pairs with `hash/sha256-bytes` /
/// `hash/sha256-raw` and the `encoding` byte variants.
pub(super) fn slurp_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "slurp-bytes", arg(args, 0))?;
    let bytes = std::fs::read(&path).map_err(|e| {
        LispError::runtime(format!("slurp-bytes: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(bytes_to_value(&bytes, heap))
}

/// `(file-size path)` — the size of `path` in bytes, or nil if it's missing.
/// GC-safe: the arg is copied to an owned `String` up front and the result is a
/// scalar — no `Value` handle is held across an allocation or eval (and a builtin
/// never fires GC mid-execution; see `docs/memory-model.md`).
pub(super) fn file_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-size", arg(args, 0))?;
    match std::fs::metadata(&path) {
        Ok(meta) => Ok(Value::int(meta.len() as i64)),
        Err(_) => Ok(Value::nil()),
    }
}

/// `(delete-file path)` — remove the file at `path`. Idempotent (nil if already
/// absent); errors on a real I/O failure (e.g. it's a directory, or permission).
pub(super) fn delete_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "delete-file", arg(args, 0))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::nil()),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::nil()),
        Err(e) => Err(LispError::runtime(format!("delete-file: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(delete-dir path)` — remove a directory and everything under it. The
/// recursive sibling of `delete-file`; idempotent (nil if already absent),
/// errors on a real I/O failure. The mechanism behind test-fixture teardown.
pub(super) fn delete_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "delete-dir", arg(args, 0))?;
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(Value::nil()),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::nil()),
        Err(e) => Err(LispError::runtime(format!("delete-dir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(rename-file from to)` — rename/move `from` to `to` (replacing `to` if it
/// exists, per the platform). Returns nil; errors on failure.
pub(super) fn rename_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "rename-file", arg(args, 0))?;
    let to = expect_string(heap, "rename-file", arg(args, 1))?;
    std::fs::rename(&from, &to).map_err(|e| {
        LispError::runtime(format!("rename-file: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(copy-file from to)` — copy the file `from` to `to` (replacing `to` if it
/// exists), preserving the contents byte-for-byte and the permission bits.
/// Returns nil; errors on failure. The binary-safe counterpart to a `slurp`+`spit`
/// (which is UTF-8 string I/O and would corrupt non-text files / drop the mode).
pub(super) fn copy_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "copy-file", arg(args, 0))?;
    let to = expect_string(heap, "copy-file", arg(args, 1))?;
    std::fs::copy(&from, &to).map_err(|e| {
        LispError::runtime(format!("copy-file: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file-mtime path)` — last-modified time of `path` as epoch-milliseconds, or
/// `nil` if the file is missing or its mtime can't be read. A cheap `stat`, not a
/// read — pairs with `load` to drive a hot-reloader: poll `file-mtime`, reload
/// only when it changes. Resolution is platform-dependent (typically nanoseconds
/// on Linux, truncated to ms here).
pub(super) fn file_mtime(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-mtime", arg(args, 0))?;
    let Ok(meta) = std::fs::metadata(&path) else {
        return Ok(Value::nil());
    };
    let Ok(modified) = meta.modified() else {
        return Ok(Value::nil());
    };
    let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return Ok(Value::nil());
    };
    Ok(Value::int(since.as_millis() as i64))
}

/// `(file-stat path)` — one `stat` for `path` as a map, or `nil` if it is missing.
/// Collapses the `dir?` / `file-size` / `file-mtime` trio (each its own syscall)
/// into a single metadata read — the shape a directory lister (dired) wants per
/// entry. `:symlink?` and `:mode` describe the link itself (`symlink_metadata`),
/// while `:dir?` / `:size` / `:mtime` follow it (a symlink to a directory reports
/// `:dir? true` so it's navigable, yet `:symlink? true` so it can be marked). Off
/// unix there are no permission bits, so `:mode` is 0 and `:exec?` is false.
pub(super) fn file_stat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file-stat", arg(args, 0))?;
    // lstat for the link's own nature; stat (follows) for size/mtime/dir?-of-target.
    let Ok(lmeta) = std::fs::symlink_metadata(&path) else {
        return Ok(Value::nil());
    };
    let symlink = lmeta.file_type().is_symlink();
    // Follow the link for the navigable facts; fall back to the link itself for a
    // dangling symlink (so a broken link still lists rather than vanishing).
    let meta = std::fs::metadata(&path).unwrap_or(lmeta);

    let epoch_ms = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| Value::int(d.as_millis() as i64))
            .unwrap_or(Value::nil())
    };
    let mtime = epoch_ms(meta.modified());
    let atime = epoch_ms(meta.accessed());

    #[cfg(unix)]
    let (mode, exec, nlink, uid, gid) = {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let m = meta.permissions().mode();
        (
            m as i64 & 0o7777,
            m & 0o111 != 0,
            meta.nlink() as i64,
            meta.uid(),
            meta.gid(),
        )
    };
    #[cfg(not(unix))]
    let (mode, exec, nlink, uid, gid) = (0_i64, false, 1_i64, 0_u32, 0_u32);

    let kw = |k: &'static str| Value::keyword(value::intern(k));
    // Owner/group names (getpwuid/getgrgid), falling back to the numeric id as a string.
    let owner = uid_name(uid).unwrap_or_else(|| uid.to_string());
    let group = gid_name(gid).unwrap_or_else(|| gid.to_string());
    let owner_v = heap.alloc_string(&owner);
    let group_v = heap.alloc_string(&group);
    let pairs = vec![
        (kw("dir?"), Value::boolean(meta.is_dir())),
        (kw("size"), Value::int(meta.len() as i64)),
        (kw("mtime"), mtime),
        (kw("atime"), atime),
        (kw("symlink?"), Value::boolean(symlink)),
        (kw("exec?"), Value::boolean(exec)),
        (kw("mode"), Value::int(mode)),
        (kw("nlink"), Value::int(nlink)),
        (kw("uid"), Value::int(uid as i64)),
        (kw("gid"), Value::int(gid as i64)),
        (kw("owner"), owner_v),
        (kw("group"), group_v),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// The user name for `uid` via `getpwuid`, or `None` if it doesn't resolve. The libc
/// call returns a pointer into a shared static buffer, so a process-wide lock serialises
/// our calls (Brood schedules green processes across OS threads); the name is copied out
/// before the lock drops. `None` off unix.
#[cfg(unix)]
pub(super) fn uid_name(uid: u32) -> Option<String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap();
    unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if pw.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*pw).pw_name)
            .to_str()
            .ok()
            .map(|s| s.to_string())
    }
}

/// The group name for `gid` via `getgrgid` (see `uid_name` for the locking note).
#[cfg(unix)]
pub(super) fn gid_name(gid: u32) -> Option<String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap();
    unsafe {
        let gr = libc::getgrgid(gid as libc::gid_t);
        if gr.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*gr).gr_name)
            .to_str()
            .ok()
            .map(|s| s.to_string())
    }
}

#[cfg(not(unix))]
pub(super) fn uid_name(_uid: u32) -> Option<String> {
    None
}
#[cfg(not(unix))]
pub(super) fn gid_name(_gid: u32) -> Option<String> {
    None
}

/// `(getenv name)` — the value of environment variable `name` as a string, or nil
/// if it is unset. Lets Brood locate things like the user config directory.
pub(super) fn getenv(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_string(heap, "getenv", arg(args, 0))?;
    match std::env::var(&name) {
        Ok(val) => Ok(heap.alloc_string(&val)),
        Err(_) => Ok(Value::nil()),
    }
}

/// `(hostname)` — this machine's short hostname (no domain), used to qualify a
/// node name as `name@host` (ADR-073). Reads `/proc/sys/kernel/hostname`,
/// falling back to `$HOSTNAME` then `"localhost"` — never errors, since a node
/// must always get *some* identity. Long/FQDN names are had by passing an
/// already-qualified name to `node-start` (`:foo@my.fqdn`), so we don't resolve
/// the FQDN here.
pub(super) fn hostname(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let h = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "localhost".to_string());
    Ok(heap.alloc_string(&h))
}

/// `(%env-all)` — all environment variables as a `{string → string}` map.
pub(super) fn env_all(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let env: Vec<(String, String)> = std::env::vars().collect();
    let pairs: Vec<(Value, Value)> = env
        .iter()
        .map(|(k, v)| (heap.alloc_string(k), heap.alloc_string(v)))
        .collect();
    Ok(heap.map_from_pairs(pairs))
}

/// `(%argv)` — command-line arguments as a vector of strings, including argv[0].
pub(super) fn argv_builtin(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let args: Vec<String> = std::env::args().collect();
    let vals: Vec<Value> = args.iter().map(|a| heap.alloc_string(a)).collect();
    Ok(heap.alloc_vector(vals))
}

/// `(%os-type)` — the current OS as a keyword: `:linux`, `:macos`, or `:windows`.
pub(super) fn os_type_builtin(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    #[cfg(target_os = "linux")]
    return Ok(Value::keyword(value::intern("linux")));
    #[cfg(target_os = "macos")]
    return Ok(Value::keyword(value::intern("macos")));
    #[cfg(target_os = "windows")]
    return Ok(Value::keyword(value::intern("windows")));
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return Ok(Value::keyword(value::intern("unknown")));
}

/// `(%os-cmd prog args)` — run `prog` with `args` (list or vector of strings),
/// capturing stdout and stderr. Returns `{:stdout s :stderr s :exit n}`.
pub(super) fn os_cmd(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prog = expect_string(heap, "%os-cmd", arg(args, 0))?;
    let mut cmd = std::process::Command::new(&prog);
    if args.len() > 1 {
        let raw = heap.seq_items(arg(args, 1))?;
        for a in &raw {
            cmd.arg(expect_string(heap, "%os-cmd", *a)?);
        }
    }
    let output = cmd.output().map_err(|e| {
        LispError::runtime(format!("%os-cmd: {prog}: {e}"))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
    })?;
    let stdout = heap.alloc_string(&String::from_utf8_lossy(&output.stdout));
    let stderr = heap.alloc_string(&String::from_utf8_lossy(&output.stderr));
    let exit_code = output.status.code().unwrap_or(-1) as i64;
    let kw = |k: &'static str| Value::keyword(value::intern(k));
    Ok(heap.map_from_pairs(vec![
        (kw("stdout"), stdout),
        (kw("stderr"), stderr),
        (kw("exit"), Value::int(exit_code)),
    ]))
}

/// `(%halt code)` — terminate the process immediately with `code`.
pub(super) fn halt_builtin(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let code = expect_int(heap, "%halt", arg(args, 0))?;
    std::process::exit(code as i32);
}

/// `(run-process prog args)` — run external program `prog` with `args` (a list or
/// vector of strings), inheriting stdio, and return its exit code as an integer
/// (-1 if killed by a signal). The Emacs `call-process` analogue: the general
/// subprocess mechanism (used by the project scaffolder's `git init`).
pub(super) fn run_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let prog = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "run-process",
                "string program",
                pv,
            ))
        }
    };
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        match a {
            Value::Str(id) => argv.push(heap.string(id).to_string()),
            _ => {
                return Err(LispError::type_err(
                    "run-process: arguments must be strings",
                ))
            }
        }
    }
    match std::process::Command::new(&prog).args(&argv).status() {
        Ok(status) => Ok(Value::int(status.code().unwrap_or(-1) as i64)),
        Err(e) => Err(LispError::runtime(format!("run-process: {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)
            .with_hint("check that the program is on PATH and the args are well-formed")),
    }
}
