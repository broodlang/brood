// The header doc block uses column-aligned continuation lines for the
// protocol-surface table; that's deliberately wider than the lint's 2-space
// indent rule expects.
#![allow(clippy::doc_overindented_list_items)]

//! `nest mcp` — the Model Context Protocol dispatcher for a Brood project.
//!
//! A synchronous JSON-RPC loop over stdio (newline-delimited JSON — the MCP
//! stdio transport, *not* LSP's `Content-Length` framing) scoped strictly to a
//! single project (ADR-036, ADR-028).
//! The caller in `main.rs` walks up to `project.blsp`, builds + bootstraps an
//! [`Interp`], and hands it here; `run` owns the protocol from that point on.
//!
//! ## Protocol surface (v0 / step 2)
//!
//! - `initialize`            — return server info + capabilities.
//! - `initialized` (notif)    — acknowledged, no reply.
//! - `tools/list`            — call `(mcp/tools)` in the session's Brood image
//!                             and project the catalogue to MCP's
//!                             `{name, description, inputSchema}` shape.
//! - `tools/call`            — convert the JSON `arguments` to a Brood map,
//!                             [`brood::eval::apply`] the named handler, and
//!                             render the returned Brood value as JSON
//!                             wrapped in MCP's `content: [{type:"text"}]`.
//! - `resources/list`,
//!   `resources/read`        — static doc/source URIs baked in via
//!                             `include_str!` (see [`RESOURCES`]).
//! - `prompts/list`,
//!   `prompts/get`           — the `brood-task` prompt (ADR-036).
//! - `ping`, `shutdown`,
//!   `exit`                  — the boring lifecycle pieces.
//!
//! ## State + hot reload
//!
//! One [`Interp`] for the connection's lifetime; the `def`s a `tools/call`
//! creates promote into RUNTIME and survive between calls (the hot-reload
//! contract, ADR-013). `(mcp/tools)` is re-evaluated on every `tools/list`
//! and `tools/call`, so an agent that redefines the catalogue mid-session
//! sees its own changes — agreed by design (`docs/mcp.md`).
//!
//! ## Architecture
//!
//! Everything that touches the heap funnels through the typed entry points
//! [`list_tools`], [`call_tool`], [`json_to_value`], [`value_to_json`]. They
//! own the LOCAL-heap discipline (`checkpoint` / `reset_local_to` around any
//! `eval_str`) and the GC-rooting discipline (anything held across an
//! eval-driving call is pushed with `push_root` first). The transport
//! (framing + loop) takes `impl BufRead` / `impl Write`, so tests drive it
//! with `Cursor<Vec<u8>>` / `Vec<u8>` rather than real stdio.

use std::error::Error;
use std::io::{BufRead, BufReader, Write};

use brood::core::heap::Heap;
use brood::core::value::{self, MapId, Value};
use brood::Interp;

use serde_json::{json, Map as JsonMap, Value as Json};

// ============================================================================
// Public entry
// ============================================================================

/// Run the MCP dispatcher over real stdio until the peer closes the channel
/// or sends `exit`. The caller has already bootstrapped `interp` for this
/// project (the LSP's [`bootstrap_project`] pattern — see `nest/src/main.rs`).
pub fn run(interp: &mut Interp) -> Result<(), Box<dyn Error>> {
    // Lock stdin/stdout once: writing back the response while reading the next
    // request races otherwise, and Rust's stdio locks are reentrant per-thread.
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    main_loop(
        interp,
        &mut BufReader::new(stdin.lock()),
        &mut stdout.lock(),
    )
}

// ============================================================================
// Transport — newline-delimited JSON (MCP stdio) + JSON-RPC envelope
// ============================================================================

/// The result of pulling one line off the transport: a parsed message, a clean
/// EOF (peer hung up), or a non-blank line that didn't parse as JSON.
///
/// A parse failure is **not** fatal: JSON-RPC defines `-32700 Parse error` as a
/// per-message response, and the MCP stdio transport is one independent message
/// per line — so one garbled line (a truncated write, a stray log line on the
/// channel) must not tear down a long-lived session. `main_loop` answers with a
/// `-32700` envelope and keeps serving. (Earlier this surfaced as an
/// `io::ErrorKind::InvalidData` that propagated out of `main_loop` and killed
/// the connection — spec-incorrect and brittle for a daemon an agent keeps open
/// for an entire editing session.)
enum ReadOutcome {
    Message(Json),
    Eof,
    Parse(String),
}

/// Read one **newline-delimited** JSON message — the MCP stdio transport: one
/// JSON-RPC object per line, no framing headers. (This is *not* LSP, which uses
/// `Content-Length` headers; using that here is why a real MCP client — Claude
/// Code — could never complete the `initialize` handshake.) Returns
/// [`ReadOutcome::Eof`] at clean EOF (peer closed the channel — exit cleanly).
/// Blank lines are tolerated as separators; a non-empty line that doesn't parse
/// as JSON is [`ReadOutcome::Parse`] (the caller replies `-32700` and keeps
/// serving). A genuine *I/O* error still propagates (the channel itself is gone).
fn read_message<R: BufRead>(r: &mut R) -> std::io::Result<ReadOutcome> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Ok(ReadOutcome::Eof); // EOF between messages — peer hung up
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue; // tolerate stray blank lines between messages
        }
        return Ok(match serde_json::from_str::<Json>(trimmed) {
            Ok(msg) => ReadOutcome::Message(msg),
            Err(e) => ReadOutcome::Parse(e.to_string()),
        });
    }
}

/// The JSON-RPC `-32700 Parse error` response for an unparseable line. Per the
/// spec the `id` is `null` (the request couldn't be parsed, so its id is
/// unknown). `data` carries the parser's message so an operator can see *what*
/// failed to parse.
fn parse_error_response(detail: &str) -> Json {
    json!({
        "jsonrpc": "2.0",
        "id": Json::Null,
        "error": {
            "code": -32700,
            "message": "Parse error",
            "data": detail,
        },
    })
}

/// Write one **newline-delimited** JSON message: the compact body followed by a
/// single `\n` (the MCP stdio transport). The body must contain no embedded
/// newlines, which `serde_json`'s compact serialization guarantees.
fn write_message<W: Write>(w: &mut W, msg: &Json) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    w.write_all(&body)?;
    w.write_all(b"\n")?;
    w.flush()
}

// ============================================================================
// Main loop
// ============================================================================

/// What to do with each incoming message: send a reply, ignore (a
/// notification), or stop the loop (`exit`).
enum Outcome {
    Reply(Json),
    NoReply,
    Exit,
}

/// Detects, per request, whether the running binary has been **rebuilt since this
/// server started** — a long-lived `nest mcp` otherwise silently serves the
/// *pre-rebuild* runtime. A stale server pinned to a pre-fix binary was the cause of
/// the 2026-05-31 GC `flush_oob` report (`docs/gc-flush-panic-2026-05-31.md`),
/// so we warn loudly (once) and tell the operator to restart.
///
/// Best-effort: if the executable path or its mtime can't be read, the guard simply
/// never fires (no false alarms). [`check`](Self::check) returns the *decision* and
/// latches; `main_loop` owns the stderr message (so the logic stays unit-testable
/// without capturing stderr, and stdout stays a clean JSON-RPC stream).
struct StalenessGuard {
    started: std::time::SystemTime,
    exe: Option<std::path::PathBuf>,
    warned: bool,
}

impl StalenessGuard {
    fn new() -> Self {
        StalenessGuard {
            started: std::time::SystemTime::now(),
            exe: std::env::current_exe().ok(),
            warned: false,
        }
    }

    /// `true` exactly once — the first time the executable's mtime is observed to be
    /// newer than the server's start time (i.e. it was rebuilt under us). Latches, so
    /// the caller warns at most once.
    fn check(&mut self) -> bool {
        if self.warned {
            return false;
        }
        let Some(exe) = self.exe.as_deref() else {
            return false;
        };
        let Ok(mtime) = std::fs::metadata(exe).and_then(|m| m.modified()) else {
            return false;
        };
        if mtime > self.started {
            self.warned = true;
            return true;
        }
        false
    }
}

/// The human-facing staleness message. The agent never sees the server's
/// stderr, so this also rides back **in-band** (see [`attach_staleness_warning`])
/// — stderr alone is why the 2026-05-31/06-02 stale-server crashes went unnoticed.
fn staleness_message(exe: Option<&str>) -> String {
    format!(
        "⚠ nest mcp is serving a STALE runtime: {} was rebuilt after this server \
         started, so it is still running the old, pre-rebuild code. Restart the \
         `nest mcp` server to pick up the new build — a stale server on a pre-fix \
         binary caused the GC flush_oob crashes (docs/gc-flush-panic-2026-05-31.md). \
         Results from this session may reflect the old runtime.",
        exe.unwrap_or("the nest binary"),
    )
}

/// Append a one-shot staleness notice as an extra `text` content block on a
/// `tools/call` reply, so the **agent** sees it (stderr doesn't reach an MCP
/// client). Returns `true` if it attached — only succeeds on a successful
/// `tools/call` reply (one with a `result.content` array); other replies
/// (`initialize`, errors, notifications) leave the warning pending for the next
/// content-bearing reply, so it is never silently dropped. `content[0]` (the
/// handler's return value) is left untouched — the notice is appended.
fn attach_staleness_warning(resp: &mut Json, warning: &str) -> bool {
    let Some(blocks) = resp
        .get_mut("result")
        .and_then(|r| r.get_mut("content"))
        .and_then(Json::as_array_mut)
    else {
        return false;
    };
    blocks.push(json!({ "type": "text", "text": warning }));
    true
}

/// The synchronous request loop. Pulled out of [`run`] so tests can drive it
/// with in-memory `Cursor` / `Vec<u8>` channels.
fn main_loop<R: BufRead, W: Write>(
    interp: &mut Interp,
    r: &mut R,
    w: &mut W,
) -> Result<(), Box<dyn Error>> {
    let mut staleness = StalenessGuard::new();
    // Set once the rebuild is detected; cleared once the notice has ridden back
    // to the client on a content-bearing reply. Survives across non-tool replies
    // so the agent always sees it.
    let mut pending_warning: Option<String> = None;
    loop {
        let msg = match read_message(r)? {
            ReadOutcome::Message(msg) => msg,
            ReadOutcome::Eof => return Ok(()),
            // An unparseable line is recoverable: answer -32700 and keep the
            // session alive (the JSON-RPC contract for a parse failure).
            ReadOutcome::Parse(detail) => {
                write_message(w, &parse_error_response(&detail))?;
                continue;
            }
        };
        // A rebuild mid-session means we're now serving stale code — warn once,
        // on stderr (for a human at the terminal) and in-band (for the agent).
        if staleness.check() {
            let exe = staleness.exe.as_deref().map(|p| p.display().to_string());
            let warning = staleness_message(exe.as_deref());
            eprintln!("{warning}");
            pending_warning = Some(warning);
        }
        match dispatch(interp, &msg) {
            Outcome::Reply(mut resp) => {
                if let Some(warning) = &pending_warning {
                    if attach_staleness_warning(&mut resp, warning) {
                        pending_warning = None;
                    }
                }
                write_message(w, &resp)?;
            }
            Outcome::NoReply => {}
            Outcome::Exit => return Ok(()),
        }
    }
}

/// Route one message to its handler. A `method` we don't know:
/// - **with `id`** (a request) → reply `MethodNotFound`, per JSON-RPC.
/// - **without `id`** (a notification) → drop silently, per JSON-RPC.
fn dispatch(interp: &mut Interp, msg: &Json) -> Outcome {
    let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Json::Null);

    // Notifications carry no id; the only one we currently *act on* is `exit`
    // (which stops the loop). Every other notification — `initialized`
    // included — falls through to the generic no-reply drop below, which is the
    // correct JSON-RPC handling for a notification (no response is ever sent).
    if id.is_none() {
        if method == "exit" {
            return Outcome::Exit;
        }
        return Outcome::NoReply;
    }

    let result: Result<Json, RpcError> = match method {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(json!({ "tools": list_tools(interp) })),
        "tools/call" => call_tool(interp, &params),
        "resources/list" => Ok(json!({ "resources": list_resources() })),
        "resources/read" => read_resource(&params),
        "prompts/list" => Ok(json!({ "prompts": list_prompts() })),
        "prompts/get" => get_prompt(&params),
        "ping" => Ok(json!({})),
        "shutdown" => Ok(Json::Null),
        other => Err(RpcError::method_not_found(other)),
    };

    Outcome::Reply(envelope(id.unwrap(), result))
}

/// Wrap a per-handler result in the JSON-RPC response envelope. `data` (the
/// structured shape from `lisp_error_to_json`) rides on the error object when
/// present so the agent can branch on `error.data.kind` rather than parsing
/// `error.message`.
fn envelope(id: Json, result: Result<Json, RpcError>) -> Json {
    match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(e) => {
            let mut err_obj = JsonMap::new();
            err_obj.insert("code".into(), json!(e.code));
            err_obj.insert("message".into(), Json::String(e.message));
            if let Some(data) = e.data {
                err_obj.insert("data".into(), data);
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": Json::Object(err_obj) })
        }
    }
}

// ============================================================================
// `initialize`
// ============================================================================

/// MCP `initialize` response. The protocol version string ("2024-11-05") is
/// the one Claude Code currently announces; the spec calls these strings
/// dated and forwards-compatible — clients that don't recognise ours fall
/// back to feature negotiation.
fn initialize_result() -> Json {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {},
            "resources": {},
            "prompts": {},
        },
        "serverInfo": {
            "name": "nest-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

// ============================================================================
// `tools/list` + `tools/call`
// ============================================================================

/// Project the Brood-side tool catalogue (`(mcp/tools)`, in `std/tool/mcp.blsp`,
/// plus any project-side extensions a project's own `mcp.blsp` conses on) to the
/// JSON shape `tools/list` requires. Any error building the catalogue collapses to
/// an empty list — the server stays useful, just with no tools.
fn list_tools(interp: &mut Interp) -> Vec<Json> {
    let cp = interp.heap.checkpoint();
    let roots_base = interp.heap.roots_len();

    // Building the catalogue shouldn't print, but a project `mcp.blsp` loaded by
    // the `(require-one 'mcp)` below could — divert it off the JSON-RPC channel and
    // discard it (a `tools/list` reply has no place to surface stray output).
    brood::builtins::begin_stdout_capture();

    // Best-effort require — silently ignore "no such module" so the server still
    // boots even if a project hasn't defined its own MCP extensions.
    let _ = interp.eval_str("(require-one 'mcp)");

    let tools = match interp.eval_str("(mcp/tools)") {
        Ok(v) => {
            interp.heap.push_root(v);
            project_tool_catalogue(&interp.heap, v).unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };

    let _ = brood::builtins::take_captured_stdout();
    interp.heap.truncate_roots(roots_base);
    interp.heap.reset_local_to(cp);
    tools
}

/// Lift a Brood list of `{:name :description :schema :handler}` maps into an
/// MCP-shaped tools array. A single malformed entry doesn't poison the rest —
/// drop it and keep going (the server is more useful with some tools than
/// none).
fn project_tool_catalogue(heap: &Heap, tools: Value) -> Result<Vec<Json>, String> {
    let items = heap.seq_items(tools).map_err(|e| e.to_string())?;
    Ok(items
        .into_iter()
        .filter_map(|item| tool_entry_to_json(heap, item))
        .collect())
}

/// Convert one Brood map of tool metadata to the MCP shape `tools/list`
/// returns. Missing `:name` or `:schema` fails the entry; missing
/// `:description` is fine (omitted in the JSON).
fn tool_entry_to_json(heap: &Heap, entry: Value) -> Option<Json> {
    let map_id = match entry {
        Value::Map(id) => id,
        _ => return None,
    };
    let name = map_get_kw(heap, map_id, "name").and_then(|v| match v {
        Value::Str(id) => Some(heap.string(id).to_string()),
        _ => None,
    })?;
    let schema = map_get_kw(heap, map_id, "schema")?;
    let schema_json = value_to_json(heap, schema).ok()?;
    let mut obj = JsonMap::new();
    obj.insert("name".into(), Json::String(name));
    obj.insert("inputSchema".into(), schema_json);
    if let Some(Value::Str(id)) = map_get_kw(heap, map_id, "description") {
        obj.insert(
            "description".into(),
            Json::String(heap.string(id).to_string()),
        );
    }
    Some(Json::Object(obj))
}

/// Look up a keyword-keyed entry in a Brood map: `(get m :kw)` in Rust. The
/// keyword name has to intern, so callers pass a `&str`. Goes through the
/// CHAMP-backed `map_get` (ADR-040) — O(log N) probe instead of the old
/// linear scan over an entries slice.
fn map_get_kw(heap: &Heap, map_id: MapId, kw: &str) -> Option<Value> {
    let target = value::intern(kw);
    heap.map_get(map_id, Value::Keyword(target))
}

/// Find a tool by `name` in the catalogue and apply its handler to the JSON
/// arguments. Wraps the Brood return value in MCP's `content` envelope.
fn call_tool(interp: &mut Interp, params: &Json) -> Result<Json, RpcError> {
    let name = params
        .get("name")
        .and_then(Json::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing 'name'"))?
        .to_string();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // MCP progress (the streaming tier): if the request carried a
    // `_meta.progressToken`, arm a sink so the handler's `(progress …)`
    // calls stream `notifications/progress` messages to the client on the same
    // stdout channel — *during* this synchronous call. The stdout lock is
    // reentrant, so writing from here is safe even though `main_loop` holds it.
    let progress_token = params
        .get("_meta")
        .and_then(|m| m.get("progressToken"))
        .cloned();
    let progress_armed = if let Some(token) = progress_token {
        brood::builtins::arm_mcp_progress(Box::new(move |progress, total, message| {
            emit_progress(&progress_notification(
                &token,
                progress,
                total,
                message.as_deref(),
            ));
        }));
        true
    } else {
        false
    };

    let cp = interp.heap.checkpoint();
    let roots_base = interp.heap.roots_len();

    // Divert any `(print …)` a handler runs into an in-memory buffer for the
    // duration of the call: writing to the real stdout here would corrupt the
    // JSON-RPC stream this server speaks over stdio. The captured text rides
    // back in the result envelope (see `wrap_as_mcp_content`), so `print`-based
    // debugging is safe rather than a channel-breaking footgun.
    brood::builtins::begin_stdout_capture();

    // Run the whole handler inside `catch_unwind` so a Rust panic in *any*
    // Brood-callable path (eval / apply / a builtin / a `defn` body) is
    // contained at the MCP boundary: it surfaces as a structured RpcError
    // (`from_panic`) and the server keeps serving the next call instead of
    // tearing down the whole stdio channel.
    //
    // `AssertUnwindSafe` is sound here because the MCP server is
    // single-threaded (a synchronous `main_loop` over stdio) and the heap
    // reset just below restores the LOCAL arena to its pre-call checkpoint,
    // discarding any partial allocations a panicking handler left behind.
    // That gives us the same recovery the no-panic path has, just triggered
    // by an unwind instead of an early return.
    // Watchdog: whether this tool's *handler* runs under a 30s deadline. Only
    // `eval`/`load` run arbitrary, possibly-runaway code; other tools (fast, or
    // legitimately long like `run-tests`) run unbounded. The deadline is armed
    // *inside* the closure — right before the handler `apply` — so the
    // dispatcher's own overhead (the `(require-one 'mcp)` / catalogue rebuild below)
    // doesn't eat the handler's budget. Checked inline in eval's loop (scheduler
    // deadline, ADR-063), so it surfaces as an ordinary error and leaves the
    // existing error / panic / output-capture handling intact.
    let watchdog = name == "eval" || name == "load";
    let inner = std::panic::AssertUnwindSafe(|| -> Result<Json, RpcError> {
        // Re-fetch the catalogue per call so a `def` in a previous `eval`
        // call (hot reload) reshapes the tool surface immediately. This runs
        // *before* the deadline is armed, so a slow catalogue rebuild doesn't
        // count against the handler's 30s.
        let _ = interp.eval_str("(require-one 'mcp)");
        let tools = interp
            .eval_str("(mcp/tools)")
            .map_err(|_| RpcError::invalid_params(format!("no such tool: {name}")))?;
        interp.heap.push_root(tools);

        let handler = find_handler(&interp.heap, tools, &name)
            .ok_or_else(|| RpcError::invalid_params(format!("no such tool: {name}")))?;
        // Closures from `defn` are RUNTIME (so stable across LOCAL resets),
        // but `apply` may itself fire GC at its outermost safepoint — push
        // anything we hold across it.
        interp.heap.push_root(handler);

        let args_value =
            json_to_value(&mut interp.heap, &arguments).map_err(RpcError::invalid_params)?;
        interp.heap.push_root(args_value);

        // Arm the deadline only now — it wraps just the handler evaluation, not
        // the dispatcher overhead above. Cleared unconditionally after
        // `catch_unwind` below (a no-op when it was never armed).
        if watchdog {
            brood::process::set_deadline(Some(
                std::time::Instant::now() + std::time::Duration::from_secs(30),
            ));
        }
        let result_value =
            brood::eval::apply(&mut interp.heap, handler, &[args_value], interp.root)
                .map_err(|e| RpcError::from_lisp(&mut interp.heap, &e))?;

        let content = value_to_json(&interp.heap, result_value).map_err(RpcError::internal)?;
        Ok(content)
    });
    let outcome = match std::panic::catch_unwind(inner) {
        Ok(result) => result,
        Err(payload) => Err(RpcError::from_panic(payload)),
    };
    brood::process::set_deadline(None);
    if progress_armed {
        brood::builtins::disarm_mcp_progress();
    }

    // Always drain the capture buffer (even on error / panic) so it never leaks
    // into the next call; attach it to a successful reply's content envelope.
    let captured = brood::builtins::take_captured_stdout().unwrap_or_default();
    let outcome = outcome.map(|content| wrap_as_mcp_content(content, &captured));

    // Reset regardless of how the call ended — early-return error, normal
    // success, or a caught panic. This drops every LOCAL allocation the
    // handler made (including any half-formed state the panic left behind),
    // so subsequent tool calls start from the same heap shape the failing
    // one did.
    interp.heap.truncate_roots(roots_base);
    interp.heap.reset_local_to(cp);
    outcome
}

thread_local! {
    /// A test-only redirect for progress notifications. When set, `emit_progress`
    /// writes there instead of the process stdout — so an end-to-end `main_loop`
    /// test can observe the `notifications/progress` stream (the real path uses
    /// the reentrant stdout lock, which a `Vec` test writer can't see).
    static PROGRESS_TEST_OUT: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<Vec<u8>>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Write one progress notification to the client — the process stdout (a
/// reentrant lock, safe while `main_loop` holds it), or a test redirect.
fn emit_progress(note: &Json) {
    PROGRESS_TEST_OUT.with(|c| {
        if let Some(buf) = c.borrow().as_ref() {
            let mut b = buf.borrow_mut();
            let _ = write_message(&mut *b, note);
        } else {
            let mut out = std::io::stdout().lock();
            if write_message(&mut out, note).is_ok() {
                let _ = out.flush();
            }
        }
    });
}

/// Build an MCP `notifications/progress` message for `token`: `progress` is the
/// value so far, `total` the math/denominator (if known), `message` a human label.
/// Per the MCP spec, `progress` MUST increase; the token echoes the request's
/// `_meta.progressToken` (a string or a number — passed through as-is).
fn progress_notification(
    token: &Json,
    progress: i64,
    total: Option<i64>,
    message: Option<&str>,
) -> Json {
    let mut params = json!({ "progressToken": token, "progress": progress });
    if let Some(t) = total {
        params["total"] = json!(t);
    }
    if let Some(m) = message {
        params["message"] = json!(m);
    }
    json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": params })
}

/// Walk the tool list looking for the entry whose `:name` matches; return its
/// `:handler` value (a `Fn` or `Native`).
fn find_handler(heap: &Heap, tools: Value, name: &str) -> Option<Value> {
    for item in heap.seq_items(tools).ok()? {
        let map_id = match item {
            Value::Map(id) => id,
            _ => continue,
        };
        let item_name = match map_get_kw(heap, map_id, "name") {
            Some(Value::Str(id)) => heap.string(id),
            _ => continue,
        };
        if item_name == name {
            return map_get_kw(heap, map_id, "handler");
        }
    }
    None
}

/// MCP `tools/call` returns `{ content: [{type: "text", text: "..."}] }`.
/// Plain strings pass through; structured values are pretty-printed JSON.
/// (`structuredContent` is a recent MCP addition; sticking to `text` for v0
/// maximises client compatibility, ADR-011.)
///
/// `content[0]` is always the handler's return value (the stable contract an
/// agent parses). If the handler `(print …)`d anything, that captured stdout
/// rides along as a second, clearly-labelled text block — so `print`-based
/// debugging surfaces in the reply instead of corrupting the JSON-RPC channel.
///
/// A handler signals a *soft* failure (the `docs/mcp.md` convention) by returning
/// a map with a non-null `:error` key. Those get MCP's `isError: true`, so a client
/// can distinguish a failed call from a successful one without parsing the payload.
fn wrap_as_mcp_content(content: Json, captured_stdout: &str) -> Json {
    let is_error =
        matches!(&content, Json::Object(m) if m.get("error").is_some_and(|v| !v.is_null()));
    let text = match &content {
        Json::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    let mut blocks = vec![json!({ "type": "text", "text": text })];
    if !captured_stdout.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": format!("[captured stdout]\n{captured_stdout}"),
        }));
    }
    let mut result = json!({ "content": blocks });
    if is_error {
        if let Some(object) = result.as_object_mut() {
            object.insert("isError".to_string(), Json::Bool(true));
        }
    }
    result
}

// ============================================================================
// `resources/list` + `resources/read`
// ============================================================================

/// Static resources served by URI. The doc set is baked in at compile time —
/// the agent gets the canonical Brood references over MCP without needing
/// filesystem access. (Project-specific state — the manifest, source — is reachable
/// through the `eval` tool, e.g. `(slurp "project.blsp")`, so there's no dynamic
/// resource here yet; add one if a read-only project URI proves worth the plumbing.)
const RESOURCES: &[(&str, &str, &str)] = &[
    (
        "brood://docs/brood-for-claude",
        "Brood for Claude (pocket reference)",
        include_str!("../../../docs/brood-for-claude.md"),
    ),
    // The self-improving findings index — entries grow with each non-trivial
    // session (see `docs/llm-native.md` §3). The next agent reads this *after*
    // `brood-for-claude.md` to see what already bit prior agents.
    (
        "brood://docs/incarnations",
        "Incarnations — what tripped up prior agents",
        include_str!("../../../docs/incarnations.md"),
    ),
    (
        "brood://docs/llm-native",
        "Making Brood LLM-native (forward-looking plan)",
        include_str!("../../../docs/llm-native.md"),
    ),
    // First incarnation entry — full writeup. Subsequent entries land alongside
    // and join `RESOURCES` here.
    (
        "brood://docs/claude-demo-findings",
        "Claude Opus 4.7 — concurrent Mandelbrot findings (2026-05-28)",
        include_str!("../../../docs/claude-demo-findings.md"),
    ),
    (
        "brood://docs/language",
        "Brood language reference",
        include_str!("../../../docs/language.md"),
    ),
    (
        "brood://docs/decisions",
        "Architecture decision records",
        include_str!("../../../docs/decisions.md"),
    ),
    (
        "brood://docs/types",
        "Type system contract",
        include_str!("../../../docs/types.md"),
    ),
    (
        "brood://docs/error-codes",
        "Stable error codes (`E0010`, `E0030`, …) and the catch shape",
        include_str!("../../../docs/error-codes.md"),
    ),
    ("brood://prelude", "Brood prelude source", brood::PRELUDE),
];

fn list_resources() -> Vec<Json> {
    RESOURCES
        .iter()
        .map(|(uri, name, _)| {
            json!({
                "uri": uri,
                "name": name,
                "mimeType": "text/markdown",
            })
        })
        .collect()
}

fn read_resource(params: &Json) -> Result<Json, RpcError> {
    let uri = params
        .get("uri")
        .and_then(Json::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing 'uri'"))?;
    let (_, _, text) = RESOURCES
        .iter()
        .find(|(u, _, _)| *u == uri)
        .ok_or_else(|| RpcError::invalid_params(format!("no such resource: {uri}")))?;
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "text/markdown",
            "text": text,
        }],
    }))
}

// ============================================================================
// `prompts/list` + `prompts/get`
// ============================================================================

/// The orientation prompt every Brood-aware agent should fetch first. Short
/// on purpose — depth lives in the `brood://docs/brood-for-claude` resource;
/// this is the "what should I do *right now*?" pointer. Sourced from
/// `docs/prompts/brood-task.md` so the maintainer can edit it without
/// recompiling, *and* other agent harnesses (Cursor, Aider, Continue per
/// `docs/llm-native.md` §14) can drop the same file into their system
/// prompts. Step 5a (ADR-036).
const BROOD_TASK_PROMPT: &str = include_str!("../../../docs/prompts/brood-task.md");

fn list_prompts() -> Vec<Json> {
    vec![json!({
        "name": "brood-task",
        "description": "Orient an agent for editing this Brood project: language quirks, MCP tool list, and project conventions pointer.",
    })]
}

fn get_prompt(params: &Json) -> Result<Json, RpcError> {
    let name = params
        .get("name")
        .and_then(Json::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing 'name'"))?;
    if name != "brood-task" {
        return Err(RpcError::invalid_params(format!("no such prompt: {name}")));
    }
    Ok(json!({
        "description": "Orient an agent for editing this Brood project",
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": BROOD_TASK_PROMPT },
        }],
    }))
}

// ============================================================================
// Brood ↔ JSON conversion
// ============================================================================

/// Project a Brood value into JSON. The mapping is the obvious one
/// (nil→null, bool→bool, int/float→number, string→string, list/vector→array,
/// map→object); symbols and keywords collapse to strings (keywords without
/// the leading colon — the canonical interchange form). Closures, refs,
/// pids, etc. have no JSON shape and surface as errors so a tool returning
/// one fails loudly instead of silently dropping data.
pub fn value_to_json(heap: &Heap, v: Value) -> Result<Json, String> {
    match v {
        Value::Nil => Ok(Json::Null),
        Value::Bool(b) => Ok(Json::Bool(b)),
        Value::Int(n) => Ok(json!(n)),
        // A bignum is outside i64 and JSON's `Number` can't carry it without
        // precision loss, so emit it as its decimal string (loud, lossless)
        // rather than a rounded float.
        Value::BigInt(id) => Ok(Json::String(heap.bigint(id).to_string())),
        // A decimal is exact base-10; JSON's float `Number` can't carry it without
        // precision loss, so emit its canonical decimal string (loud, lossless).
        Value::Decimal(id) => Ok(Json::String(heap.decimal(id).to_string())),
        Value::Ratio(id) => Ok(Json::String(heap.ratio(id).to_string())),
        Value::Float(f) => {
            // serde_json::Number can't carry NaN or infinity; rather than
            // emit `null` and silently lose data, fail.
            if f.is_finite() {
                Ok(json!(f))
            } else {
                Err(format!("non-finite float {f} can't be represented in JSON"))
            }
        }
        Value::Str(id) => Ok(Json::String(heap.string(id).to_string())),
        Value::Sym(s) | Value::Keyword(s) => Ok(Json::String(value::symbol_name(s))),
        // A range projects as the array it stands in for (same as print/send).
        Value::Pair(_) | Value::Vector(_) | Value::Range(_) => {
            let items = heap.seq_items(v).map_err(|e| e.to_string())?;
            items.into_iter().map(|x| value_to_json(heap, x)).collect()
        }
        Value::Map(id) => {
            let mut obj = JsonMap::new();
            for (k, val) in heap.map_entries(id) {
                let key = match k {
                    Value::Str(id) => heap.string(id).to_string(),
                    Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
                    other => {
                        return Err(format!(
                            "map key must be string/keyword/symbol for JSON, got {:?}",
                            value::tag(other)
                        ))
                    }
                };
                // String/keyword/symbol keys all collapse to the same JSON
                // string, so `:foo`, `"foo"`, and `'foo` would silently clobber
                // each other (last wins). That's data loss — fail loudly, the
                // same fail-loud contract this function holds for non-finite
                // floats and bignums.
                let json_val = value_to_json(heap, val)?;
                if obj.insert(key.clone(), json_val).is_some() {
                    return Err(format!(
                        "map has colliding JSON key {key:?} (string/keyword/symbol keys \
                         share one JSON key — last would silently win)"
                    ));
                }
            }
            Ok(Json::Object(obj))
        }
        // A set projects as a JSON array of its elements (JSON has no set type;
        // same array shape as a vector/list — the distinct set-ness is a Brood
        // concept the JSON boundary doesn't carry).
        Value::Set(id) => heap
            .set_elems(id)
            .into_iter()
            .map(|x| value_to_json(heap, x))
            .collect(),
        // Pids and refs round-trip as tagged objects so a tool returning
        // `(list-processes)` (or any pid-bearing value) doesn't lose data.
        // `{"$type": "pid", "node": "name", "id": 42}` and `{"$type": "ref",
        // "id": 7}` — the `$type` tag distinguishes them from plain maps so
        // an agent can spot them programmatically. `json_to_value` does
        // *not* reverse this (a JSON object stays a Brood map keyed by
        // keywords); constructing a fresh pid/ref from JSON would be
        // unsound (pids name a live mailbox; refs are unique).
        Value::Pid { node, id } => Ok(json!({
            "$type": "pid",
            "node": value::symbol_name(node),
            "id": id,
        })),
        Value::Ref(id) => Ok(json!({ "$type": "ref", "id": id })),
        // A rope is editor-internal buffer text with no JSON shape; a tool that
        // wants its content should return `(rope->string r)` explicitly. A socket
        // is a live OS resource — likewise no JSON shape.
        // A table is a live shared resource — no JSON shape; a tool that wants its
        // contents should return `(table-snapshot t)` (a map) explicitly.
        // A lazy seq-view has no JSON shape until realised, and this read-only
        // projection has no evaluator to run its transducer — a tool that wants
        // its items should realise it first (e.g. `(vec (map …))`).
        Value::Fn(_)
        | Value::Macro(_)
        | Value::Native(_)
        | Value::Rope(_)
        | Value::Socket(_)
        | Value::Subprocess(_)
        | Value::Table(_)
        | Value::Bytes(_)
        | Value::SeqView(_) => Err(format!(
            "value of kind {:?} has no JSON representation",
            value::tag(v)
        )),
    }
}

/// Build a Brood value from JSON. Arrays become **lists** (the pattern-match
/// friendly default in Brood — `(first xs)`/`(rest xs)` style); objects
/// become **maps** keyed by keywords (so `(get args :source)` is the
/// idiomatic access pattern in a handler). Strings become Brood strings;
/// numbers preserve integer-ness where possible.
pub fn json_to_value(heap: &mut Heap, j: &Json) -> Result<Value, String> {
    match j {
        Json::Null => Ok(Value::Nil),
        Json::Bool(b) => Ok(Value::Bool(*b)),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float(f))
            } else {
                Err(format!("number {n} outside i64/f64 range"))
            }
        }
        Json::String(s) => Ok(heap.alloc_string(s)),
        Json::Array(arr) => {
            let items: Result<Vec<Value>, String> =
                arr.iter().map(|x| json_to_value(heap, x)).collect();
            Ok(heap.list(items?))
        }
        Json::Object(obj) => {
            let mut entries = Vec::with_capacity(obj.len());
            for (k, v) in obj.iter() {
                let key = Value::Keyword(value::intern(k));
                let val = json_to_value(heap, v)?;
                entries.push((key, val));
            }
            Ok(heap.map_from_pairs(entries))
        }
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Minimal JSON-RPC error. Codes follow the spec —
/// <https://www.jsonrpc.org/specification#error_object>. `data` carries the
/// structured fields from a [`LispError`] (kind / Brood code / file / line /
/// col / hint) so an agent that hits a tool-dispatch failure can branch on
/// `error.data.kind` instead of parsing `error.message` (see
/// `docs/llm-native.md` §4).
struct RpcError {
    code: i32,
    message: String,
    data: Option<Json>,
}

impl RpcError {
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            data: None,
        }
    }
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            data: None,
        }
    }
    /// Project a `LispError` into a JSON-RPC `Internal` error carrying the
    /// structured fields in `data`. Used when a Brood-side operation
    /// (`eval_str`, `apply`) errors and we want the agent to see the kind /
    /// code / location rather than only the rendered message. The `data` shape
    /// is *derived* from `LispError::to_value_map` (the canonical Brood-map
    /// shape `try`/`catch` exposes), so the JSON an agent reads off
    /// `error.data` and the map a handler reads off `(catch …)` can't drift —
    /// see [`lisp_error_to_json`]. Allocates a transient map into LOCAL; the
    /// caller's `reset_local_to` reclaims it.
    fn from_lisp(heap: &mut Heap, e: &brood::error::LispError) -> Self {
        Self {
            code: -32603,
            message: e.to_string(),
            data: Some(lisp_error_to_json(heap, e)),
        }
    }
    /// Project a Rust *panic* (caught at the MCP tool-call boundary by
    /// `panic::catch_unwind`) into a structured error. Without this the
    /// unwind would tear through `main_loop` and kill the whole server —
    /// every `mcp__brood__*` tool would drop for the rest of the session.
    /// Here we keep serving: the agent gets an error response, the panic
    /// message and the kind-tag `"panic"` on `error.data`, and the next
    /// tool call works.
    ///
    /// The panic payload is `Box<dyn Any + Send>` — usually a `&'static str`
    /// (from `panic!("…")`) or a `String` (from `panic!("{}", x)`). Anything
    /// else falls back to a generic message; the caller still sees that
    /// *something* panicked.
    fn from_panic(payload: Box<dyn std::any::Any + Send>) -> Self {
        let message = if let Some(s) = payload.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "Rust panic in tool handler (no message)".to_string()
        };
        let mut data = JsonMap::new();
        data.insert("kind".into(), Json::String("panic".into()));
        data.insert("message".into(), Json::String(message.clone()));
        data.insert(
            "hint".into(),
            Json::String(
                "interpreter bug — the tool handler triggered a Rust panic. \
                 Subsequent calls on this session continue to work."
                    .into(),
            ),
        );
        Self {
            code: -32603,
            message: format!("panic in tool handler: {message}"),
            data: Some(Json::Object(data)),
        }
    }
}

/// Convert a [`LispError`]'s structured fields to a JSON object, **derived** from
/// the canonical `LispError::to_value_map` (the Brood map shape `try`/`catch`
/// exposes) by projecting that map through [`value_to_json`]. Used for
/// `RpcError`'s `data` field. Deriving — rather than hand-rebuilding the same
/// `{kind, message, code?, file?, line?, col?, hint?}` shape here — is what
/// keeps an agent's `error.data.kind` and a handler's `(get e :kind)` identical
/// by construction: a field added to `to_value_map` shows up in both at once,
/// with no second site to keep in sync. (`value_to_json` renders keyword keys
/// as their bare name, so `:kind` → `"kind"`, matching the prior hand-built
/// shape exactly.) Falls back to a minimal object only if the projection
/// somehow fails (it can't for this map — every value is a string/int/keyword).
fn lisp_error_to_json(heap: &mut Heap, e: &brood::error::LispError) -> Json {
    let map = e.to_value_map(heap);
    value_to_json(heap, map)
        .unwrap_or_else(|_| json!({ "kind": e.kind.tag_name(), "message": e.message.clone() }))
}

// ============================================================================
// Tests — drive `main_loop` with in-memory I/O (the LSP's `Connection::memory`
// pattern, adapted to plain BufRead/Write).
// ============================================================================

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
