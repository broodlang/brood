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

// ---- MCP progress notifications (the streaming/progress tier) -------------
//
// A long `nest mcp` tool (run-tests, check) can report incremental progress:
// the MCP dispatcher **arms** a sink around a `tools/call` that carried a
// `_meta.progressToken`, and the Brood handler calls `(mcp-progress progress
// total message)` — which lands as a `notifications/progress` JSON-RPC message
// on the (real) stdout stream the client is already reading, *during* the
// call. Off (a no-op) when no token was supplied or when not running under the
// MCP server, so the same handler is safe to call anywhere. The sink writes
// raw JSON-RPC, bypassing the Brood output-capture above (which is a
// port-level redirect, not the OS stdout).

type ProgressSink = Box<dyn Fn(i64, Option<i64>, Option<String>)>;

thread_local! {
    static MCP_PROGRESS: std::cell::RefCell<Option<ProgressSink>> =
        const { std::cell::RefCell::new(None) };
}

/// Arm the MCP progress sink for the duration of one `tools/call`. `f` receives
/// `(progress, total, message)` and emits the `notifications/progress` message
/// (the dispatcher owns the token + the write). Pair with [`disarm_mcp_progress`].
pub fn arm_mcp_progress(f: ProgressSink) {
    MCP_PROGRESS.with(|c| *c.borrow_mut() = Some(f));
}

/// Disarm the MCP progress sink — after this, `(mcp-progress …)` is a no-op again.
pub fn disarm_mcp_progress() {
    MCP_PROGRESS.with(|c| *c.borrow_mut() = None);
}

/// `(%mcp-progress progress total message)` — report progress from a `nest mcp`
/// tool handler. `progress` is an int (units completed); `total` is an int or
/// nil (the math/denominator, if known); `message` is a string or nil (a human
/// label). Returns `true` if a progress notification was actually sent (a token
/// was in scope), `false` if it was a no-op (not under an MCP progress request).
pub(super) fn mcp_progress(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let progress = expect_int(heap, "%mcp-progress", arg(args, 0))?;
    let total = match arg(args, 1) {
        Value::Nil => None,
        v => Some(expect_int(heap, "%mcp-progress", v)?),
    };
    let message = match arg(args, 2) {
        Value::Nil => None,
        v => Some(expect_string(heap, "%mcp-progress", v)?.to_string()),
    };
    let sent = MCP_PROGRESS.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f(progress, total, message);
            true
        } else {
            false
        }
    });
    Ok(Value::boolean(sent))
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

/// Write `s` to real stderr the way `write_stdout` writes stdout: a **broken
/// pipe** (the downstream consumer closed — `nest check … | head`) is not a
/// program error. The `eprint!` macro would panic on it with a Rust backtrace +
/// crash dump (every observed `failed printing to stderr: Broken pipe` crash
/// bottoms out in a bare `eprint!`/`eprintln!`), so instead we restore the
/// terminal and exit quietly, exactly as the default SIGPIPE disposition would.
/// Any other write/flush failure is best-effort-dropped.
pub(super) fn write_stderr(s: &str) {
    use std::io::Write;
    let mut err = std::io::stderr();
    if let Err(e) = err.write_all(s.as_bytes()).and_then(|_| err.flush()) {
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
    write_stderr(&parts.join(" "));
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
    let s = expect_string(heap, "%write-err", arg(args, 0))?;
    write_stderr(&s);
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

/// `(%now)` — wall-clock milliseconds since the Unix epoch, as an integer.
/// Subtract two readings to measure elapsed time (see `std/tool/test.blsp`).
pub(super) fn now(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ms = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::int(ms))
}

/// `(now-ns)` — wall-clock nanoseconds since the Unix epoch, as an integer.
/// The fine-grained partner to `now`; subtract two readings to time sub-
/// millisecond work that `now`'s resolution would round to zero. (i64
/// nanoseconds since 1970 stays in range until the year 2262.)
pub(super) fn now_ns(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ns = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(Value::int(ns))
}

// ---------- memory ----------

/// `(%)` — bytes currently allocated across the whole process.
pub(super) fn mem_bytes(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::live_bytes() as i64))
}

/// `(%)` — high-water mark of allocated bytes since the process started.
pub(super) fn mem_peak(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::peak_bytes() as i64))
}

/// `(%)` — live sizes of this process's four inline-cache tables, the
/// largest single attributed item in the green-process floor (`FRONTIER.md` lever 1
/// puts them at 896 B/process, bigger than the whole `Box<Process>`). Each of
/// `:calls` (`vm_call_ics`), `:links` (`vm_fast_links`), `:globals` (`vm_global_ics`)
/// and `:blocks` (`arm_ic_blocks`) reports `[len capacity bytes]`, with bytes from
/// live CAPACITY x the real element size — a `Vec` grown past its contents holds that
/// memory whatever its length says. `:call-entry-bytes` is `size_of::<Option<CallIcEntry>>()`,
/// the figure a shrink of that struct would move. Per-process; size it at the PARKED
/// state, not at teardown (a teardown slot count read 3.5x too high — see the
/// 2026-08-18 note in `docs/runtime-frontier.md`).
#[cfg(feature = "dev-tools")]
pub(super) fn ic_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (t, entry) = heap.ic_table_stats();
    fn triple(heap: &mut Heap, (l, c, b): (usize, usize, usize)) -> Value {
        heap.alloc_vector(vec![
            Value::int(l as i64),
            Value::int(c as i64),
            Value::int(b as i64),
        ])
    }
    let calls = triple(heap, t[0]);
    let links = triple(heap, t[1]);
    let globals = triple(heap, t[2]);
    let blocks = triple(heap, t[3]);
    let total: usize = t.iter().map(|x| x.2).sum();
    let pairs = vec![
        (value::kw("calls"), calls),
        (value::kw("links"), links),
        (value::kw("globals"), globals),
        (value::kw("blocks"), blocks),
        (value::kw("call-entry-bytes"), Value::int(entry as i64)),
        (value::kw("total-bytes"), Value::int(total as i64)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(%)` — entry counts of the two source-position side tables:
/// `:local-forms` (this process's LOCAL `form_pos`) and `:runtime-forms` (the
/// runtime-shared `positions`). Measurement surface for the position-table cost,
/// which the 2026-08-06 module-load breakdown put at 169 MB of a 933 MB load and
/// 24% of load time. Per-process for the LOCAL half, runtime-wide for the other.
#[cfg(feature = "dev-tools")]
pub(super) fn pos_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (local, local_cap, runtime, rt_cap) = heap.pos_table_stats();
    let (local_bytes, rt_bytes) = heap.pos_table_bytes();
    let pairs = vec![
        (value::kw("local-forms"), Value::int(local as i64)),
        (value::kw("local-cap"), Value::int(local_cap as i64)),
        (value::kw("local-bytes"), Value::int(local_bytes as i64)),
        (value::kw("runtime-forms"), Value::int(runtime as i64)),
        (value::kw("runtime-cap"), Value::int(rt_cap as i64)),
        (value::kw("runtime-bytes"), Value::int(rt_bytes as i64)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(%)` — a snapshot map of this process's garbage-collection activity
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
/// `(dev/runtime-collect)`, so it's not included here.
#[cfg(feature = "dev-tools")]
pub(super) fn gc_stats(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(gc_stats_map(heap))
}

/// Build the `(%)` snapshot map of the calling process's GC activity.
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
        // Pause durations (the observability timing tier): cumulative wall time
        // spent in this process's collections, the worst single pause, and the
        // most recent one — µs so the numbers stay readable ints (a minor
        // collection is µs-scale; a bad pause ms-scale).
        (
            value::kw("pause-total-us"),
            Value::int((heap.gc_pause_ns().0 / 1_000) as i64),
        ),
        (
            value::kw("pause-max-us"),
            Value::int((heap.gc_pause_ns().1 / 1_000) as i64),
        ),
        (
            value::kw("pause-last-us"),
            Value::int((heap.gc_pause_ns().2 / 1_000) as i64),
        ),
        // The shared RUNTIME code region (not per-process — every process sees the
        // same figure). `:runtime-closures` is its total promoted-closure count
        // (cheap — a slab length); it grows with hot-reload churn and the eval
        // safepoint compacts it back toward `:runtime-threshold` (single-process
        // today, ADR-091). The live/reclaimable split is the expensive walk reported
        // by `(dev/runtime-collect)`'s `{:before :after :reclaimed}`, kept out of here.
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

/// `(%)` — a snapshot map of the VM work-attribution counters (the
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

/// `(%)` — zero the work-attribution counters, returning `:enabled`.
///
/// The counters are **process-global and cumulative from process start**, so a snapshot
/// taken after a short program includes the runtime's own boot work — and boot is
/// macro-expansion-heavy, which defers to the tree-walker. Measured: the same
/// list-building program read an 84% defer rate on a cold boot cache and 0.8% on a warm
/// one, purely from whether expansion ran. Anything measuring a *region* rather than a
/// whole process must zero first; `(perf/measure thunk)` in `std/tool/perf.blsp` is that,
/// packaged.
///
/// A no-op returning `:enabled false` without `--features perf-stats`, like `(%)`.
#[cfg(feature = "dev-tools")]
pub(super) fn vm_stats_reset(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::perf::reset();
    let enabled = crate::perf::snapshot().is_some();
    Ok(heap.map_from_pairs(vec![(value::kw("enabled"), Value::boolean(enabled))]))
}

/// `(dev/runtime-collect)` — compact the shared RUNTIME code region now (reclaim
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

/// `(%)` — force a collection of this process's LOCAL heap *now*,
/// returning the post-collection `(%)` map so the effect is visible.
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

/// `(%)` / `(gc-trace on?)` — query or set per-collection GC trace
/// logging for the calling process. With no argument, returns the current state;
/// with one, sets it (truthy = on) and returns the new state. When on, each
/// minor/major collection prints a one-line summary to stderr. Per-process and
/// defaulted from the `BROOD_GC_TRACE` env var (which traces the whole run,
/// including the root process before any `(%)` call).
#[cfg(feature = "dev-tools")]
pub(super) fn gc_trace(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let Some(&v) = args.first() {
        heap.set_gc_trace(crate::eval::truthy(v));
    }
    Ok(Value::boolean(heap.gc_trace()))
}

/// `(%)` — the hard memory ceiling in bytes (0 = unlimited). ADR-043.
pub(super) fn mem_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::hard_limit() as i64))
}

/// `(%)` — the soft memory ceiling in bytes (0 = unlimited). ADR-043.
pub(super) fn mem_soft_limit(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::int(crate::core::alloc::soft_limit() as i64))
}

// ---------- TCP sockets (ADR-062) ----------
//
// Thin non-blocking mechanism over `crate::net`; the active-socket / framing /
// HTTP policy is Brood (std/net/tcp.blsp). A socket is `Value::Socket(id)`.

pub(super) fn expect_socket(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "socket",
        Value::Socket(id) => id,
    )
}

// ---------- in-memory shared table (Brood's ETS, ADR-107) ----------
// A `Value::Table(id)` handle; the store lives in `crate::core::table`. These builtins are
// thin wrappers — all the storage / locking / clone-in-clone-out lives there.

pub(super) fn expect_table(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "table",
        Value::Table(id) => id,
    )
}

pub(super) fn table_new(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::table(crate::core::table::create()))
}

pub(super) fn table_put(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-put", arg(args, 0))?;
    crate::core::table::check_key("table-put", arg(args, 1))?;
    crate::core::table::put(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-get", arg(args, 0))?;
    crate::core::table::check_key("table-get", arg(args, 1))?;
    crate::core::table::get(heap, id, arg(args, 1), arg(args, 2))
}

pub(super) fn table_has(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-has?", arg(args, 0))?;
    crate::core::table::check_key("table-has?", arg(args, 1))?;
    Ok(Value::boolean(crate::core::table::has(
        heap,
        id,
        arg(args, 1),
    )?))
}

pub(super) fn table_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-delete", arg(args, 0))?;
    crate::core::table::check_key("table-delete", arg(args, 1))?;
    crate::core::table::delete(heap, id, arg(args, 1))
}

pub(super) fn table_incr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-incr", arg(args, 0))?;
    crate::core::table::check_key("table-incr", arg(args, 1))?;
    let delta = match arg(args, 2) {
        Value::Nil => 1, // (table-incr t k) defaults the delta to 1
        v => expect_int(heap, "table-incr", v)?,
    };
    crate::core::table::incr(heap, id, arg(args, 1), delta)
}

pub(super) fn table_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-count", arg(args, 0))?;
    Ok(Value::int(crate::core::table::count(id)?))
}

pub(super) fn table_snapshot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-snapshot", arg(args, 0))?;
    crate::core::table::snapshot(heap, id)
}

pub(super) fn table_drop(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_table(heap, "table-drop", arg(args, 0))?;
    Ok(Value::boolean(crate::core::table::drop_table(id)))
}

pub(super) fn socket_port(who: &str, p: i64) -> Result<u16, LispError> {
    u16::try_from(p)
        .map_err(|_| LispError::runtime(format!("{}: port {} out of range 0..=65535", who, p)))
}

pub(super) fn tcp_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "%tcp-connect", arg(args, 0))?;
    let port = socket_port(
        "%tcp-connect",
        expect_int(heap, "%tcp-connect", arg(args, 1))?,
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
    let host = expect_string(heap, "%tcp-listen", arg(args, 0))?;
    let port = socket_port(
        "%tcp-listen",
        expect_int(heap, "%tcp-listen", arg(args, 1))?,
    )?;
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
    let host = expect_string(heap, "%tls-listen", arg(args, 0))?;
    let port = socket_port(
        "%tls-listen",
        expect_int(heap, "%tls-listen", arg(args, 1))?,
    )?;
    let cert = expect_string(heap, "%tls-listen", arg(args, 2))?;
    let key = expect_string(heap, "%tls-listen", arg(args, 3))?;
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
    let host = expect_string(heap, "%tls-self-signed", arg(args, 0))?.to_string();
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
    let host = expect_string(heap, "%tls-request", arg(args, 0))?.to_string();
    let port = socket_port(
        "%tls-request",
        expect_int(heap, "%tls-request", arg(args, 1))?,
    )?;
    // The request is any iolist (ADR-141/143) — a string, bytes, or a nested
    // tree — flattened once here, so binary https request bodies work.
    let mut request = Vec::new();
    flatten_iolist(heap, "%tls-request", arg(args, 2), &mut request)?;
    // Optional 4th arg: a PEM trust anchor replacing the Mozilla roots for
    // this request (private CAs, tls-self-signed dev servers).
    let ca = match args.get(3) {
        Some(v) if !matches!(v, Value::Nil) => {
            Some(expect_string(heap, "%tls-request", *v)?.to_string())
        }
        _ => None,
    };
    let owner = crate::process::self_pid();
    let id = crate::net::tls_request(&host, port, request, ca, owner)
        .map_err(|e| LispError::runtime(format!("tls-request: {}", e)))?;
    Ok(Value::socket(id))
}

/// Flatten an **iolist** into `out` at a write boundary (ADR-139): a leaf — a
/// string, a `bytes`, or a byte int 0–255 — or an arbitrarily nested proper
/// list/vector of iolists (`nil` = empty; an improper tail is a final leaf, as
/// in Erlang). Callers describe output as a tree
/// (`[status-line headers "\r\n\r\n" body]`) and nothing is copied until this
/// single flatten at the device write — which deletes the O(n²)
/// `(str acc chunk)` accumulation class at its root. A string leaf is **always
/// its UTF-8 bytes**, whatever the device's mode — raw bytes are what `bytes`
/// values are for (the pre-`bytes` "Latin-1 byte-string" send rule is gone with
/// the carrier-string era, ADR-141). Iterative worklist, so nesting depth is
/// heap-bounded — and immutable data cannot be cyclic, so termination is
/// structural, no visited set needed.
pub(super) fn flatten_iolist(
    heap: &Heap,
    who: &str,
    root: Value,
    out: &mut Vec<u8>,
) -> Result<(), LispError> {
    let mut stack: Vec<Value> = vec![root];
    while let Some(v) = stack.pop() {
        match v {
            Value::Nil => {}
            Value::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
            Value::Bytes(b) => out.extend_from_slice(heap.bytes(b).as_bytes()),
            Value::Str(id) => {
                let s = heap.string(id);
                out.extend_from_slice(s.as_bytes());
            }
            Value::Pair(p) => {
                // Process car first; the cdr is the rest of the iolist (a leaf
                // there is Erlang's improper tail).
                let (car, cdr) = {
                    let cell = heap.pair(p);
                    (cell.0, cell.1)
                };
                stack.push(cdr);
                stack.push(car);
            }
            Value::Vector(id) => {
                let items = heap.vector(id).to_vec();
                for &item in items.iter().rev() {
                    stack.push(item);
                }
            }
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    who,
                    "iolist (string, bytes, byte int 0-255, or a nested list/vector of those)",
                    other,
                ))
            }
        }
    }
    Ok(())
}

/// Lower a tcp-send/proc-send payload to raw bytes: any **iolist** (ADR-139).
/// String leaves are always UTF-8 — the device's binary flag affects only the
/// inbound decode (ADR-141); raw bytes go out as `bytes` values.
fn send_payload(heap: &Heap, who: &str, v: Value) -> Result<Vec<u8>, LispError> {
    let mut out = Vec::new();
    flatten_iolist(heap, who, v, &mut out)?;
    Ok(out)
}

pub(super) fn tcp_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-send", arg(args, 0))?;
    let out = send_payload(heap, "%tcp-send", arg(args, 1))?;
    crate::net::send(id, &out).map_err(|e| LispError::runtime(format!("tcp-send: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-set-binary", arg(args, 0))?;
    let on = !matches!(arg(args, 1), Value::Nil | Value::Bool(false));
    crate::net::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("tcp-set-binary: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_set_idle_timeout(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-set-idle-timeout", arg(args, 0))?;
    let ms = expect_int(heap, "%tcp-set-idle-timeout", arg(args, 1))?;
    if ms < 0 {
        return Err(LispError::runtime(
            "tcp-set-idle-timeout: ms must be >= 0 (0 disarms)",
        ));
    }
    crate::net::set_idle_timeout(id, ms as u64)
        .map_err(|e| LispError::runtime(format!("tcp-set-idle-timeout: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_controlling_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-controlling-process", arg(args, 0))?;
    let pid = match arg(args, 1) {
        Value::Pid { id, .. } => id,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%tcp-controlling-process",
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
    let id = expect_socket(heap, "%tcp-close", arg(args, 0))?;
    crate::net::close(id);
    Ok(Value::nil())
}

pub(super) fn tcp_local_port(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-local-port", arg(args, 0))?;
    Ok(crate::net::local_port(id)
        .map(|p| Value::int(p as i64))
        .unwrap_or(Value::nil()))
}

// ----- persistent child processes (ADR-104) ----------------------------------
//
// Thin mechanism over `crate::subprocess`: spawn a long-lived child with piped stdio,
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
    match crate::subprocess::spawn(&prog, &argv, cwd.as_deref(), &env, owner) {
        Ok(id) => Ok(Value::subprocess(id)),
        Err(e) => Err(LispError::runtime(format!("proc-spawn {}: {}", prog, e))
            .with_code(crate::error::error_codes::SUBPROCESS_FAILED)),
    }
}

pub(super) fn proc_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-send", arg(args, 0))?;
    let out = send_payload(heap, "proc-send", arg(args, 1))?;
    crate::subprocess::send(id, &out)
        .map_err(|e| LispError::runtime(format!("proc-send: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-set-binary", arg(args, 0))?;
    let on = crate::eval::truthy(arg(args, 1));
    crate::subprocess::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("proc-set-binary: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn proc_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_subprocess(heap, "proc-close", arg(args, 0))?;
    crate::subprocess::close(id);
    Ok(Value::nil())
}

// ----- process introspection (ADR-051) ---------------------------------------
//
// Kernel-internal per-process state an observer needs but Brood can't reach:
// `mailbox-size` and the `process-info` snapshot, assembled here from the
// registry / scheduler / name / monitor tables. `std/tool/observer.blsp` builds
// everything else on top. (The terminal/GUI frontend lives in terminal.rs.)

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

/// `(%process-info pid)` — a snapshot map of a **live local** process, or `nil`
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

/// `(string/->number s)` — parse `s` as an integer if it is one, else as a float,
/// else `nil`. The inverse of `str`. A robust parse-or-nil can't be
/// expressed over `read-string` (which would read `"3abc"` as `3` and stop), so
/// the strict parse is a primitive. Surrounding whitespace is not accepted —
/// `trim` first if the input may carry any.

pub(super) fn string_to_number(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string/->number", arg(args, 0))?;
    if let Ok(i) = s.parse::<i64>() {
        Ok(Value::int(i))
    } else if let Ok(n) = s.parse::<num_bigint::BigInt>() {
        // An integer too big for i64 is a bignum — mirroring the reader's
        // over-range literal path — NOT a lossy f64 (which silently rounded
        // `(str big)` away from round-tripping, kernel audit).
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
// manipulation and all policy live in Brood (`std/prelude.blsp`, `std/tool/project.blsp`).

/// `(file/cwd)` — the process's current working directory as a string.
/// `(%exe-path)` — the absolute path of the RUNNING executable, or nil when the platform
/// won't say (a sandbox with no `/proc/self/exe`-equivalent). Nil rather than an error: a
/// program asking where it lives is asking opportunistically, and the answer is allowed to
/// be "cannot tell".
///
/// What it is for: locating something installed ALONGSIDE this binary. A shipped app cannot
/// assume `PATH` — a desktop launch inherits the session's, which routinely lacks
/// `~/.local/bin` — so "the runtime that installed me is my sibling" is the reliable lookup,
/// and it needs this. (myedit's eval sandbox spawns a Brood runtime for its child; from a
/// dash-launched editor, PATH alone finds nothing.)
pub(super) fn exe_path(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_exe() {
        Ok(p) => Ok(heap.alloc_string(&p.to_string_lossy())),
        Err(_) => Ok(Value::nil()),
    }
}

pub(super) fn cwd(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_dir() {
        Ok(p) => Ok(heap.alloc_string(&p.to_string_lossy())),
        Err(e) => {
            Err(LispError::runtime(format!("cwd: {}", e))
                .with_code(crate::error::error_codes::FILE_IO))
        }
    }
}

/// `(file/exists? path)` — true if a file or directory exists at `path`.
pub(super) fn file_exists(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/exists?", arg(args, 0))?;
    Ok(Value::boolean(std::path::Path::new(&path).exists()))
}

/// `(%canonicalize path)` — the real absolute path of `path` with **symlinks and
/// `.`/`..` fully resolved**. Works for a not-yet-existing target: the longest
/// existing prefix is `fs::canonicalize`d (which resolves every symlink in it,
/// the POSIX-correct way — a `..` after a symlink resolves against the symlink's
/// target, not lexically), then the non-existent tail (which has no symlinks) is
/// resolved lexically against that canonical prefix (`..` pops, `.` drops). So
/// the result never contains a `..`/`.` and is safe for a plain `starts_with`
/// sandbox check. Relative paths are taken against the cwd. Returns nil only if
/// the cwd can't be read. Backs symlink-escape-proof path sandboxing
/// (`std/tool/mcp.blsp`).
pub(super) fn path_canonicalize(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::path::{Component, Path, PathBuf};
    let path = expect_string(heap, "canonicalize", arg(args, 0))?;
    let raw = Path::new(&path);
    let abs = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(raw),
            Err(_) => return Ok(Value::nil()),
        }
    };
    // Apply the (symlink-free) non-existent tail components to `base` with real
    // `..`/`.` semantics — the tail can't contain symlinks (it doesn't exist),
    // so lexical resolution against the canonical `base` is correct.
    let apply_tail = |mut base: PathBuf, tail: &[Component]| -> PathBuf {
        for c in tail {
            match c {
                Component::ParentDir => {
                    base.pop();
                }
                Component::CurDir => {}
                other => base.push(other.as_os_str()),
            }
        }
        base
    };
    // Find the longest existing PREFIX (by component count) and canonicalize it;
    // fs::canonicalize needs the whole path to exist, so shrink until it does.
    // For an absolute path this always succeeds by k=1 (the root). Rebuilding the
    // prefix each step is O(n²) in components, but paths are short.
    let comps: Vec<Component> = abs.components().collect();
    for k in (1..=comps.len()).rev() {
        let mut prefix = PathBuf::new();
        for c in &comps[..k] {
            prefix.push(c.as_os_str());
        }
        if let Ok(real) = std::fs::canonicalize(&prefix) {
            let out = apply_tail(real, &comps[k..]);
            return Ok(heap.alloc_string(&out.to_string_lossy()));
        }
    }
    // Nothing (not even the root) canonicalized — a broken mount. Fall back to a
    // purely lexical normalization so callers still get a stable, `..`-free path.
    let out = apply_tail(PathBuf::new(), &comps);
    Ok(heap.alloc_string(&out.to_string_lossy()))
}

/// `(file/dir? path)` — true if `path` exists and is a directory.
pub(super) fn is_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/dir?", arg(args, 0))?;
    Ok(Value::boolean(std::path::Path::new(&path).is_dir()))
}

/// `(file/ls path)` — the entry names (not full paths) directly under a
/// directory, sorted for determinism. Errors if `path` isn't a readable directory.
pub(super) fn list_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/ls", arg(args, 0))?;
    let mut names: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) => {
            return Err(LispError::runtime(format!("file/ls: {}: {}", path, e))
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

/// `(file/mkdir path)` — create `path` and any missing parents (like `mkdir -p`).
/// Returns nil. Used by the project scaffolder (`nest new`).
pub(super) fn make_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/mkdir", arg(args, 0))?;
    std::fs::create_dir_all(&path).map_err(|e| {
        LispError::runtime(format!("file/mkdir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file/spit path content)` — write `content` (a string) to `path`, replacing any
/// existing file. Returns nil. The write-side counterpart to `load`.
pub(super) fn spit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let path = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "file/spit", "string path", pv)),
    };
    // Content is any iolist (ADR-139): describe the file as a tree of
    // strings/bytes and it is flattened exactly once, here at the write.
    let mut content = Vec::new();
    flatten_iolist(heap, "file/spit", arg(args, 1), &mut content)?;
    std::fs::write(&path, content).map_err(|e| {
        LispError::runtime(format!("file/spit: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file/spit-append path content)` — append `content` (a string) to the file at
/// `path`, creating it if absent. Returns nil. Unlike `spit` (which truncates),
/// this opens in append mode, so each call's write lands at end-of-file — the
/// atomic-append the OS guarantees for an `O_APPEND` handle, which is what makes a
/// log file safe to write from several processes concurrently. The string sibling
/// of `append-bytes`.
pub(super) fn spit_append(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write;
    let path = expect_string(heap, "file/spit-append", arg(args, 0))?;
    // Content is any iolist (ADR-139) — one flatten, one O_APPEND write.
    let mut content = Vec::new();
    flatten_iolist(heap, "file/spit-append", arg(args, 1), &mut content)?;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .map_err(|e| {
            LispError::runtime(format!("file/spit-append: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO)
        })?;
    f.write_all(&content).map_err(|e| {
        LispError::runtime(format!("file/spit-append: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(file/spit-bytes path bytes)` — write a byte sequence (a `bytes` value, a vector,
/// or a list of byte ints 0–255) to `path` byte-faithfully, replacing any
/// existing file. Returns nil. The binary write-side counterpart to `slurp-bytes`:
/// `spit` is UTF-8 string-only and would reject (or corrupt) raw bytes, so this is
/// what materialises a received image / archive / any binary asset to disk.
pub(super) fn spit_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/spit-bytes", arg(args, 0))?;
    // Any iolist (ADR-139) — a strict superset of the old bytes/vector/list-of-ints
    // surface (byte ints are iolist leaves), plus strings (UTF-8) and nesting.
    let mut bytes = Vec::new();
    flatten_iolist(heap, "file/spit-bytes", arg(args, 1), &mut bytes)?;
    std::fs::write(&path, &bytes).map_err(|e| {
        LispError::runtime(format!("file/spit-bytes: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

pub(super) fn append_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::io::Write;
    let path = expect_string(heap, "append-bytes", arg(args, 0))?;
    // Any iolist (ADR-139) — see `spit_bytes`.
    let mut bytes = Vec::new();
    flatten_iolist(heap, "append-bytes", arg(args, 1), &mut bytes)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| {
            LispError::runtime(format!("append-bytes: {}: {}", path, e))
                .with_code(crate::error::error_codes::FILE_IO)
        })?;
    f.write_all(&bytes).map_err(|e| {
        LispError::runtime(format!("append-bytes: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
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

/// `(read-line)` — read one line from stdin, returning it as a string with the
/// trailing newline stripped, or `nil` at end of input (EOF / Ctrl-D). The one
/// irreducible I/O mechanism the Brood-hosted REPL (`std/tool/repl.blsp`) can't
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

/// `(file/slurp path)` — read the whole file at `path` and return it as a string. The
/// read-side counterpart to `spit`; unlike `load` it does not evaluate, so the
/// doc tooling can inspect a module's source (e.g. its leading docstring form).
pub(super) fn slurp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/slurp", arg(args, 0))?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        LispError::runtime(format!("file/slurp: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(heap.alloc_string(&content))
}

/// `(file/slurp-bytes path)` — read the whole file at `path` as a bytes value. The
/// byte-faithful read `slurp` can't be: `slurp` is UTF-8 and throws
/// on a non-text file, whereas this reads any bytes (images, archives, a binary
/// asset to hash via `hash/sha256-bytes`). Pairs with `hash/sha256-bytes` /
/// `hash/sha256-raw` and the `encoding` byte variants.
pub(super) fn slurp_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/slurp-bytes", arg(args, 0))?;
    let bytes = std::fs::read(&path).map_err(|e| {
        LispError::runtime(format!("file/slurp-bytes: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(bytes_to_value(&bytes, heap))
}

/// `(file-size path)` — the size of `path` in bytes, or nil if it's missing.
/// GC-safe: the arg is copied to an owned `String` up front and the result is a
/// scalar — no `Value` handle is held across an allocation or eval (and a builtin
/// never fires GC mid-execution; see `docs/memory-model.md`).
pub(super) fn file_size(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/size", arg(args, 0))?;
    match std::fs::metadata(&path) {
        Ok(meta) => Ok(Value::int(meta.len() as i64)),
        Err(_) => Ok(Value::nil()),
    }
}

/// `(file/rm path)` — remove the file at `path`. Idempotent (nil if already
/// absent); errors on a real I/O failure (e.g. it's a directory, or permission).
pub(super) fn delete_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/rm", arg(args, 0))?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::nil()),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::nil()),
        Err(e) => Err(LispError::runtime(format!("file/rm: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(delete-dir path)` — remove a directory and everything under it. The
/// recursive sibling of `delete-file`; idempotent (nil if already absent),
/// errors on a real I/O failure. The mechanism behind test-fixture teardown.
pub(super) fn delete_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/rmdir", arg(args, 0))?;
    match std::fs::remove_dir_all(&path) {
        Ok(()) => Ok(Value::nil()),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::nil()),
        Err(e) => Err(LispError::runtime(format!("file/rmdir: {}: {}", path, e))
            .with_code(crate::error::error_codes::FILE_IO)),
    }
}

/// `(rename-file from to)` — rename/move `from` to `to` (replacing `to` if it
/// exists, per the platform). Returns nil; errors on failure.
pub(super) fn rename_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "file/rename", arg(args, 0))?;
    let to = expect_string(heap, "file/rename", arg(args, 1))?;
    std::fs::rename(&from, &to).map_err(|e| {
        LispError::runtime(format!("file/rename: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(copy-file from to)` — copy the file `from` to `to` (replacing `to` if it
/// exists), preserving the contents byte-for-byte and the permission bits.
/// Returns nil; errors on failure. The binary-safe counterpart to a `slurp`+`spit`
/// (which is UTF-8 string I/O and would corrupt non-text files / drop the mode).
pub(super) fn copy_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_string(heap, "file/cp", arg(args, 0))?;
    let to = expect_string(heap, "file/cp", arg(args, 1))?;
    std::fs::copy(&from, &to).map_err(|e| {
        LispError::runtime(format!("file/cp: {} -> {}: {}", from, to, e))
            .with_code(crate::error::error_codes::FILE_IO)
    })?;
    Ok(Value::nil())
}

/// `(image-thumb bytes max-w max-h)` — decode an encoded image (PNG / JPEG / GIF /
/// WebP / BMP) from a byte sequence and downscale it to fit within `max-w`×`max-h`
/// pixels (aspect ratio preserved), returning `{:width :height :rgba}` where `:rgba`
/// is a `width*height*4` bytes value (row-major RGBA8). Returns nil when the bytes
/// aren't a decodable image or the dims are non-positive — untrusted input degrades
/// to "not an image" rather than throwing. Per-call decode `Limits` bound a
/// decompression bomb (≤ 16384² px, ≤ 512 MB alloc). The one image primitive;
/// rendering (half-block cells, a GUI texture, …) is Brood policy over this buffer.
pub(super) fn image_thumb(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let bytes = collect_bytes("image-thumb", arg(args, 0), heap)?;
    let max_w = expect_int(heap, "image-thumb", arg(args, 1))?;
    let max_h = expect_int(heap, "image-thumb", arg(args, 2))?;
    if max_w <= 0 || max_h <= 0 {
        return Ok(Value::nil());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    let mut reader =
        match image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format() {
            Ok(r) => r,
            Err(_) => return Ok(Value::nil()),
        };
    reader.limits(limits);
    let Ok(img) = reader.decode() else {
        return Ok(Value::nil());
    };
    // Downscale-only: a source already within the box keeps its native size (never
    // upscaled — `thumbnail`/`resize` would blow a small image up to fill the box).
    let thumb = if img.width() <= max_w as u32 && img.height() <= max_h as u32 {
        img.to_rgba8()
    } else {
        img.thumbnail(max_w as u32, max_h as u32).to_rgba8()
    };
    let (w, h) = (thumb.width(), thumb.height());
    // GC-safe: no eval between this alloc and map_from_pairs (a builtin never fires
    // GC mid-execution), mirroring file_stat holding its string handles.
    let rgba = bytes_to_value(thumb.as_raw(), heap);
    let kw = |k: &'static str| Value::keyword(value::intern(k));
    let pairs = vec![
        (kw("width"), Value::int(w as i64)),
        (kw("height"), Value::int(h as i64)),
        (kw("rgba"), rgba),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(file-mtime path)` — last-modified time of `path` as epoch-milliseconds, or
/// `nil` if the file is missing or its mtime can't be read. A cheap `stat`, not a
/// read — pairs with `load` to drive a hot-reloader: poll `file-mtime`, reload
/// only when it changes. Resolution is platform-dependent (typically nanoseconds
/// on Linux, truncated to ms here).
pub(super) fn file_mtime(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "file/mtime", arg(args, 0))?;
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
    let path = expect_string(heap, "file/stat", arg(args, 0))?;
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

/// `(%file-swap lock-path data-path expected new)` — replace the ENTIRE contents of
/// `data-path` with `new`, but only if they currently equal `expected`. Returns
/// true when the swap happened, false when the contents differed (the caller should
/// re-read, recompute, and try again).
///
/// This is the mechanism behind a safe read-modify-write of a file whose "modify"
/// step is Brood code — `nest add` editing `project.blsp`, say. Without it, two
/// concurrent editors both read the original, both append, and the second write
/// erases the first while both report success (measured: three concurrent
/// `nest add`s landed between one and three of them).
///
/// Two properties make it work, and both are load-bearing:
///
///   * **Serialised** by a blocking exclusive `flock` on `lock-path` — a separate
///     file, never the data file, because the data file is replaced by `rename`
///     below and a lock on a since-unlinked inode excludes nobody. The lock is held
///     only for the duration of this call, so it cannot leak, and the OS drops it if
///     the process dies.
///   * **Crash-atomic** in its write: the new contents go to a temp file in the same
///     directory and are `rename`d over `data-path`, so a crash mid-call leaves the
///     old file intact rather than a truncated one. (A half-written manifest is
///     exactly the "project no longer parses" failure this is meant to prevent.)
///
/// A missing `data-path` reads as `""`, so the same call creates it when `expected`
/// is `""`.
pub(super) fn file_swap(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let lock_path = expect_string(heap, "%file-swap", arg(args, 0))?;
    let data_path = expect_string(heap, "%file-swap", arg(args, 1))?;
    let expected = expect_string(heap, "%file-swap", arg(args, 2))?;
    let new = expect_string(heap, "%file-swap", arg(args, 3))?;

    let io_err = |what: &str, path: &str, e: &std::io::Error| {
        LispError::runtime(format!("%file-swap: {what} {path}: {e}"))
            .with_code(crate::error::error_codes::FILE_IO)
    };

    // The lock file's own directory must exist; the caller picks a durable
    // location (the project's cache dir), so a missing parent is a real error.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| io_err("cannot open lock file", &lock_path, &e))?;
    let _guard = FileLock::acquire(&lock).map_err(|e| io_err("cannot lock", &lock_path, &e))?;

    // Read under the lock: this is the re-validation that makes the caller's
    // earlier (unlocked) read safe to act on.
    let current = match std::fs::read_to_string(&data_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io_err("cannot read", &data_path, &e)),
    };
    if current != expected {
        return Ok(Value::boolean(false));
    }

    // The temp file is created with O_EXCL (`create_new`) at an *unguessable* path,
    // and both halves matter. `fs::write` to `{data}.swap.{pid}` is
    // `O_CREAT|O_WRONLY|O_TRUNC` with no `O_EXCL` and no symlink check, so in a
    // directory an attacker can write to, a symlink pre-planted at that entirely
    // predictable name makes this swap overwrite whatever it points at — `O_CREAT`
    // follows symlinks, `O_EXCL` refuses them. The random suffix removes the
    // pre-planting; `create_new` removes the follow. Retried because a random name can
    // (astronomically rarely) already exist.
    let (mut temp_file, temp_path) = {
        let mut made = None;
        let mut last: Option<std::io::Error> = None;
        for _ in 0..8 {
            let mut rnd = [0u8; 8];
            getrandom::fill(&mut rnd).map_err(|e| {
                LispError::runtime(format!("%file-swap: cannot get randomness: {e}"))
                    .with_code(crate::error::error_codes::FILE_IO)
            })?;
            let suffix = rnd.iter().fold(String::new(), |mut s, b| {
                use std::fmt::Write as _;
                let _ = write!(s, "{b:02x}");
                s
            });
            let path = format!("{data_path}.swap.{suffix}");
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(f) => {
                    made = Some((f, path));
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        match made {
            Some(pair) => pair,
            None => {
                let e =
                    last.unwrap_or_else(|| std::io::Error::other("no temp path could be created"));
                return Err(io_err("cannot create temp file beside", &data_path, &e));
            }
        }
    };
    {
        use std::io::Write as _;
        if let Err(e) = temp_file.write_all(new.as_bytes()) {
            drop(temp_file);
            let _ = std::fs::remove_file(&temp_path);
            return Err(io_err("cannot write", &temp_path, &e));
        }
    }
    drop(temp_file);
    if let Err(e) = std::fs::rename(&temp_path, &data_path) {
        // Don't leave the temp file behind on a failed rename.
        let _ = std::fs::remove_file(&temp_path);
        return Err(io_err("cannot replace", &data_path, &e));
    }
    Ok(Value::boolean(true))
}

/// An exclusive advisory lock held for a scope, released on drop (and by the OS if
/// the process dies, which is what keeps a crash from leaving a stale lock).
struct FileLock<'a> {
    #[cfg(unix)]
    file: &'a std::fs::File,
    #[cfg(not(unix))]
    _file: &'a std::fs::File,
}

impl<'a> FileLock<'a> {
    #[cfg(unix)]
    fn acquire(file: &'a std::fs::File) -> std::io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        loop {
            // SAFETY: `fd` is a live descriptor owned by `file` for this scope.
            let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if rc == 0 {
                return Ok(FileLock { file });
            }
            let err = std::io::Error::last_os_error();
            // A signal can interrupt the blocking wait; that is not a failure.
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(err);
            }
        }
    }

    // Non-unix has no `flock`. The compare-and-swap still runs (so behaviour is
    // unchanged for a single process) but is NOT serialised across processes; the
    // platforms this project builds for are unix.
    #[cfg(not(unix))]
    fn acquire(file: &'a std::fs::File) -> std::io::Result<Self> {
        Ok(FileLock { _file: file })
    }
}

#[cfg(unix)]
impl Drop for FileLock<'_> {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: same live descriptor; failure to unlock is not actionable here,
        // and closing the fd would release it regardless.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}
