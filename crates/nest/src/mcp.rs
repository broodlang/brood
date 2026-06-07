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
//! - `tools/list`            — call `(mcp/mcp-tools)` in the session's Brood image
//!                             and project the catalogue to MCP's
//!                             `{name, description, inputSchema}` shape.
//! - `tools/call`            — convert the JSON `arguments` to a Brood map,
//!                             [`brood::eval::apply`] the named handler, and
//!                             render the returned Brood value as JSON
//!                             wrapped in MCP's `content: [{type:"text"}]`.
//! - `resources/list`,
//!   `resources/read`        — static doc/source URIs baked in via
//!                             `include_str!` (see [`RESOURCES`]).
//! - `prompts/list`          — empty; Tier-1 (step 5) work.
//! - `ping`, `shutdown`,
//!   `exit`                  — the boring lifecycle pieces.
//!
//! ## State + hot reload
//!
//! One [`Interp`] for the connection's lifetime; the `def`s a `tools/call`
//! creates promote into RUNTIME and survive between calls (the hot-reload
//! contract, ADR-013). `(mcp/mcp-tools)` is re-evaluated on every `tools/list`
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
/// the 2026-05-31 GC `flush_oob` report (`docs/gc-flush-panic-mcp-2026-05-31.md`),
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
         binary caused the GC flush_oob crashes (docs/gc-flush-panic-mcp-2026-05-31.md). \
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

/// Project the Brood-side tool catalogue (`(mcp/mcp-tools)`, in `std/mcp.blsp` —
/// and any project-side extensions step 3 introduces) to the JSON shape
/// `tools/list` requires. A missing `std/mcp.blsp` (or any error) collapses
/// to an empty list — the server stays useful, just with no tools yet.
fn list_tools(interp: &mut Interp) -> Vec<Json> {
    let cp = interp.heap.checkpoint();
    let roots_base = interp.heap.roots_len();

    // Building the catalogue shouldn't print, but a project `mcp.blsp` loaded by
    // the `(require 'mcp)` below could — divert it off the JSON-RPC channel and
    // discard it (a `tools/list` reply has no place to surface stray output).
    brood::builtins::begin_stdout_capture();

    // Best-effort require — silently ignore "no such module" so the server
    // works the moment it boots, before `std/mcp.blsp` exists (step 3) and
    // even if a project hasn't defined its own MCP extensions yet.
    let _ = interp.eval_str("(require 'mcp)");

    let tools = match interp.eval_str("(mcp/mcp-tools)") {
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
    // dispatcher's own overhead (the `(require 'mcp)` / catalogue rebuild below)
    // doesn't eat the handler's budget. Checked inline in eval's loop (scheduler
    // deadline, ADR-063), so it surfaces as an ordinary error and leaves the
    // existing error / panic / output-capture handling intact.
    let watchdog = name == "eval" || name == "load";
    let inner = std::panic::AssertUnwindSafe(|| -> Result<Json, RpcError> {
        // Re-fetch the catalogue per call so a `def` in a previous `eval`
        // call (hot reload) reshapes the tool surface immediately. This runs
        // *before* the deadline is armed, so a slow catalogue rebuild doesn't
        // count against the handler's 30s.
        let _ = interp.eval_str("(require 'mcp)");
        let tools = interp
            .eval_str("(mcp/mcp-tools)")
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
fn wrap_as_mcp_content(content: Json, captured_stdout: &str) -> Json {
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
    json!({ "content": blocks })
}

// ============================================================================
// `resources/list` + `resources/read`
// ============================================================================

/// Static resources served by URI. The doc set is baked in at compile time —
/// the agent gets the canonical Brood references over MCP without needing
/// filesystem access. Step 3 will add a dynamic `brood://project` URI that
/// reads `project.blsp` from the bootstrapped project root.
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
    (
        "brood://prelude",
        "Brood prelude source",
        include_str!("../../../std/prelude.blsp"),
    ),
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
        Value::Fn(_)
        | Value::Macro(_)
        | Value::Native(_)
        | Value::Rope(_)
        | Value::Socket(_)
        | Value::Transient(_) => Err(format!(
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
    value_to_json(heap, map).unwrap_or_else(|_| {
        json!({ "kind": e.kind.tag_name(), "message": e.message.clone() })
    })
}

// ============================================================================
// Tests — drive `main_loop` with in-memory I/O (the LSP's `Connection::memory`
// pattern, adapted to plain BufRead/Write).
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn staleness_guard_fires_once_when_binary_is_newer_than_start() {
        // Binary present, mtime (now) > started (epoch) → stale: fires once, latches.
        let tmp = std::env::temp_dir().join(format!("nest-mcp-stale-{}", std::process::id()));
        std::fs::write(&tmp, b"x").unwrap();
        let mut g = StalenessGuard {
            started: std::time::UNIX_EPOCH,
            exe: Some(tmp.clone()),
            warned: false,
        };
        assert!(g.check(), "a binary newer than the start time is stale");
        assert!(!g.check(), "the warning latches — fires at most once");
        let _ = std::fs::remove_file(&tmp);

        // Binary older than the start time → not stale.
        let tmp2 = std::env::temp_dir().join(format!("nest-mcp-fresh-{}", std::process::id()));
        std::fs::write(&tmp2, b"x").unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        let mut g2 = StalenessGuard { started: future, exe: Some(tmp2.clone()), warned: false };
        assert!(!g2.check(), "a binary older than the start time is not stale");
        let _ = std::fs::remove_file(&tmp2);

        // Unresolvable executable → best-effort no-op (never a false alarm).
        let mut g3 = StalenessGuard {
            started: std::time::UNIX_EPOCH,
            exe: Some(std::path::PathBuf::from("/no/such/nest-binary-xyz")),
            warned: false,
        };
        assert!(!g3.check(), "a missing binary must not fire");
    }

    #[test]
    fn staleness_warning_rides_back_on_a_tool_reply_not_other_replies() {
        let warning = staleness_message(Some("/x/nest"));
        assert!(warning.contains("STALE"), "message names the condition");

        // A tools/call reply has a `result.content` array → the notice attaches
        // as an extra block, leaving content[0] (the handler's value) untouched.
        let mut tool_reply = json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "content": [{ "type": "text", "text": "42" }] }
        });
        assert!(attach_staleness_warning(&mut tool_reply, &warning));
        let blocks = tool_reply["result"]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2, "warning appended as a second block");
        assert_eq!(blocks[0]["text"], "42", "handler value is left first");
        assert!(blocks[1]["text"].as_str().unwrap().contains("STALE"));

        // A non-content reply (initialize, an error envelope) can't carry it →
        // the caller keeps the warning pending for the next content-bearing reply.
        let mut init_reply = json!({
            "jsonrpc": "2.0", "id": 1, "result": { "capabilities": {} }
        });
        assert!(!attach_staleness_warning(&mut init_reply, &warning));
        let mut err_reply = json!({
            "jsonrpc": "2.0", "id": 1, "error": { "code": -32601, "message": "x" }
        });
        assert!(!attach_staleness_warning(&mut err_reply, &warning));
    }

    /// Build a newline-delimited JSON buffer from a list of messages (the MCP
    /// stdio framing — one compact object per line).
    fn frame(messages: &[Json]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in messages {
            let body = serde_json::to_vec(m).unwrap();
            buf.extend_from_slice(&body);
            buf.push(b'\n');
        }
        buf
    }

    /// Parse a server's stream of newline-delimited JSON responses out of a `Vec<u8>`.
    fn unframe(output: &[u8]) -> Vec<Json> {
        let mut r = Cursor::new(output);
        let mut out = Vec::new();
        while let Ok(ReadOutcome::Message(m)) = read_message(&mut r) {
            out.push(m);
        }
        out
    }

    /// Run `main_loop` end-to-end against a sequence of requests. Returns the
    /// reply stream (notifications produce no replies and are absent).
    fn round_trip(interp: &mut Interp, requests: &[Json]) -> Vec<Json> {
        let input = frame(requests);
        let mut output = Vec::new();
        main_loop(interp, &mut Cursor::new(input), &mut output).unwrap();
        unframe(&output)
    }

    fn req(id: i64, method: &str, params: Json) -> Json {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    fn notif(method: &str, params: Json) -> Json {
        json!({ "jsonrpc": "2.0", "method": method, "params": params })
    }

    #[test]
    fn transport_is_newline_delimited_json_not_content_length() {
        // Regression: the MCP stdio transport is one JSON object per line. A real
        // client (Claude Code) frames this way — if we revert to LSP-style
        // `Content-Length` headers, `initialize` never completes. So assert the
        // raw bytes: a bare newline-delimited request parses, and a
        // `Content-Length:` header line is *not* valid JSON (it errors, proving
        // we no longer treat it as framing).
        let line = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut r = Cursor::new(&line[..]);
        let msg = match read_message(&mut r).unwrap() {
            ReadOutcome::Message(m) => m,
            other => panic!("expected a parsed message, got {}", outcome_label(&other)),
        };
        assert_eq!(msg["method"], "ping");

        // The output side emits compact body + a single trailing newline.
        let mut out = Vec::new();
        write_message(&mut out, &json!({"ok": true})).unwrap();
        assert_eq!(out, b"{\"ok\":true}\n");

        // A leftover `Content-Length:` header is just a non-JSON line → a
        // recoverable parse error (the caller answers -32700 and keeps serving),
        // *not* valid framing.
        let mut r = Cursor::new(&b"Content-Length: 17\r\n"[..]);
        assert!(
            matches!(read_message(&mut r).unwrap(), ReadOutcome::Parse(_)),
            "header must not be accepted as a message"
        );
    }

    /// A short label for a `ReadOutcome` in test panic messages.
    fn outcome_label(o: &ReadOutcome) -> &'static str {
        match o {
            ReadOutcome::Message(_) => "Message",
            ReadOutcome::Eof => "Eof",
            ReadOutcome::Parse(_) => "Parse",
        }
    }

    #[test]
    fn a_malformed_line_yields_a_parse_error_and_the_session_continues() {
        // JSON-RPC: a non-blank line that doesn't parse is answered with a
        // -32700 Parse error (id null) and the session keeps serving — one
        // garbled line must not tear down a long-lived daemon. We feed a junk
        // line between two valid requests and assert all three replies arrive.
        let mut interp = Interp::new();
        let mut input = Vec::new();
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        input.extend_from_slice(b"this is not json{{{\n");
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"exit\"}\n");
        let mut output = Vec::new();
        main_loop(&mut interp, &mut Cursor::new(input), &mut output).unwrap();
        let replies = unframe(&output);
        // ping(1), parse-error(null), ping(2) — exit produces no reply.
        assert_eq!(replies.len(), 3, "{replies:?}");
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[0]["result"], json!({}));
        // The middle reply is the -32700 with a null id.
        assert_eq!(replies[1]["error"]["code"], -32700);
        assert_eq!(replies[1]["id"], Json::Null);
        assert!(replies[1]["error"]["data"].is_string());
        // The session survived: the request *after* the junk still got served.
        assert_eq!(replies[2]["id"], 2);
        assert_eq!(replies[2]["result"], json!({}));
    }

    #[test]
    fn initialize_returns_server_info_and_capabilities() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[req(1, "initialize", json!({})), notif("exit", json!(null))],
        );
        assert_eq!(resp.len(), 1);
        let result = &resp[0]["result"];
        assert_eq!(result["serverInfo"]["name"], "nest-mcp");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
        assert!(result["capabilities"]["prompts"].is_object());
    }

    #[test]
    fn bignum_step_churn_via_mcp_does_not_corrupt_heap() {
        // Heap smoke test for the Life-demo workload that stresses the GC: many
        // wide-bignum whole-board `step` evals on ONE persistent interp, each
        // through the real `call_tool` checkpoint/promote/reset path, with wide
        // masks captured in relocated closure envs. Run under `BROOD_GC_VERIFY=1`
        // (debug) to assert the LOCAL graph stays sound across the churn. Guards
        // the checkpoint/promote/reset discipline this path depends on.
        let mut interp = Interp::new();
        // wstep takes the wide masks as ARGS (not globals), so the churn eval below
        // captures them in a LOCAL closure passed to `reduce` / a thunk — the shape
        // that actually crashed (wide bignums living in a relocated closure env).
        let setup = r#"(do
            (defn ms (f) (f))
            (defn wstep (b w h mask board col0 high)
              (let (wm1 (- w 1) hm1w (* (- h 1) w)
                    l (bit-or (bit-and (bit-shift-left b 1) (bit-xor col0 board)) (bit-shift-right (bit-and b high) wm1))
                    r (bit-or (bit-and (bit-shift-right b 1) (bit-xor high board)) (bit-shift-left (bit-and b col0) wm1))
                    up (fn (f) (bit-or (bit-and (bit-shift-left f w) board) (bit-shift-right f hm1w)))
                    dn (fn (f) (bit-or (bit-shift-right f w) (bit-shift-left (bit-and f mask) hm1w)))
                    ns [(up l) (up b) (up r) l r (dn l) (dn b) (dn r)]
                    planes (reduce (fn ([s0 s1 s2 s3] m)
                                     (let (c (bit-and s0 m) s0b (bit-xor s0 m) c2 (bit-and s1 c) s1b (bit-xor s1 c)
                                           c3 (bit-and s2 c2) s2b (bit-xor s2 c2) s3b (bit-or s3 c3))
                                       [s0b s1b s2b s3b]))
                             [0 0 0 0] ns)
                    s0 (vector-ref planes 0) s1 (vector-ref planes 1) s2 (vector-ref planes 2) s3 (vector-ref planes 3))
                (bit-and (bit-and s1 (bit-and (bit-xor s2 board) (bit-xor s3 board))) (bit-or s0 b)))))"#;
        // each call builds the wide masks as LOCAL lets, captured by the closures
        // passed to `ms` and `reduce` (exactly the prototype that crashed).
        let churn = r#"(let (w 200 h 120
                            mask (- (bit-shift-left 1 w) 1)
                            board (- (bit-shift-left 1 (* w h)) 1)
                            col0 (quot board mask)
                            high (bit-shift-left col0 (- w 1))
                            st (bit-and board (bit-shift-left (- (bit-shift-left 1 100) 1) 5000)))
                        (ms (fn () (bit-count (reduce (fn (b _) (wstep b w h mask board col0 high)) st (range 30))))))"#;
        let mut reqs = vec![
            req(1, "initialize", json!({})),
            req(2, "tools/call", json!({ "name": "eval", "arguments": { "source": setup } })),
        ];
        for i in 0..25 {
            reqs.push(req(
                10 + i,
                "tools/call",
                json!({ "name": "eval", "arguments": { "source": churn } }),
            ));
        }
        reqs.push(notif("exit", json!(null)));
        let resp = round_trip(&mut interp, &reqs);
        for r in &resp {
            assert!(
                r.get("error").is_none(),
                "an MCP call returned an error (heap corruption?): {r}"
            );
        }
    }

    #[test]
    fn tools_list_returns_the_baked_std_catalogue() {
        // Step 3 ships `std/mcp.blsp` as a baked-in `EMBEDDED_MODULES` entry, so
        // `(require 'mcp) (mcp/mcp-tools)` succeeds in a fresh `Interp` and the
        // dispatcher exposes the initial tool catalogue without any project setup.
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[req(1, "tools/list", json!({})), notif("exit", json!(null))],
        );
        let tools = resp[0]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        // The full v0 surface — six live, three documented stubs.
        for expected in &[
            "eval",
            "load",
            "write",
            "edit",
            "lookup",
            "macroexpand",
            "format",
            "check",
            "run-tests",
            "processes",
            "process-info",
            "node",
            "callers",
        ] {
            assert!(
                names.contains(expected),
                "missing {expected:?} in {names:?}"
            );
        }
        // Every entry must carry a JSON-Schema-shaped `inputSchema`.
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn tools_list_projects_a_brood_defined_catalogue() {
        let mut interp = Interp::new();
        // Pre-define an `mcp-tools` catalogue inline; mark `'mcp` as already
        // provided so the dispatcher's `(require 'mcp)` doesn't load the baked
        // `std/mcp.blsp` and clobber our test catalogue. This is exactly the
        // override path a project's own `mcp.blsp` will use (step 5): provide
        // the feature themselves, then bind their own `mcp-tools`.
        interp
            .eval_str(
                r#"
                (provide 'mcp)
                (defn mcp/mcp-tools ()
                  (list
                    {:name "echo"
                     :description "Echo the :msg argument back"
                     :schema {:type "object" :properties {:msg {:type "string"}}}
                     :handler (fn (args) (get args :msg))}))
                "#,
            )
            .unwrap();

        let resp = round_trip(
            &mut interp,
            &[req(1, "tools/list", json!({})), notif("exit", json!(null))],
        );
        let tools = resp[0]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
        assert_eq!(tools[0]["description"], "Echo the :msg argument back");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn tools_call_dispatches_to_a_brood_handler() {
        let mut interp = Interp::new();
        // Same pattern as `tools_list_projects_a_brood_defined_catalogue`:
        // claim the feature so the dispatcher's `(require 'mcp)` is a no-op
        // and our inline catalogue is what `(mcp/mcp-tools)` returns.
        interp
            .eval_str(
                r#"
                (provide 'mcp)
                (defn mcp/mcp-tools ()
                  (list
                    {:name "double"
                     :schema {:type "object" :properties {:n {:type "integer"}}}
                     :handler (fn (args) (* (get args :n) 2))}))
                "#,
            )
            .unwrap();

        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": "double", "arguments": { "n": 21 } }),
                ),
                notif("exit", json!(null)),
            ],
        );
        let content = &resp[0]["result"]["content"][0];
        assert_eq!(content["type"], "text");
        assert_eq!(content["text"], "42");
    }

    #[test]
    fn handler_print_is_captured_not_leaked_onto_the_channel() {
        // A handler that `(print …)`s must not corrupt the JSON-RPC stdio stream:
        // the printed text is diverted into a buffer and rides back as a second
        // content block, while `content[0]` stays the handler's return value.
        // `round_trip` reads the reply as newline-delimited JSON — if the print
        // had leaked to stdout it would not parse here, so a clean round-trip is
        // itself proof the channel stayed pure.
        let mut interp = Interp::new();
        interp
            .eval_str(
                r#"
                (provide 'mcp)
                (defn mcp/mcp-tools ()
                  (list
                    {:name "chatty"
                     :schema {:type "object" :properties {}}
                     :handler (fn (_) (print "debug line") 42)}))
                "#,
            )
            .unwrap();
        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": "chatty", "arguments": {} }),
                ),
                notif("exit", json!(null)),
            ],
        );
        let content = resp[0]["result"]["content"].as_array().unwrap();
        // content[0] is the unchanged return value.
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "42");
        // content[1] carries the captured stdout, clearly labelled.
        assert_eq!(
            content.len(),
            2,
            "expected a captured-stdout block: {content:?}"
        );
        let captured = content[1]["text"].as_str().unwrap();
        assert!(captured.contains("debug line"), "{captured:?}");
        assert!(captured.contains("captured stdout"), "{captured:?}");
    }

    #[test]
    fn capture_does_not_leak_between_calls() {
        // The buffer is drained after every call (even when the handler prints
        // nothing), so a silent handler reports no captured-stdout block.
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": "eval", "arguments": { "source": "(+ 1 1)" } }),
                ),
                notif("exit", json!(null)),
            ],
        );
        let content = resp[0]["result"]["content"].as_array().unwrap();
        assert_eq!(
            content.len(),
            1,
            "a non-printing handler should add no block: {content:?}"
        );
    }

    #[test]
    fn term_draw_under_mcp_diverts_escapes_instead_of_corrupting_the_stream() {
        // term-draw writes terminal escapes via crossterm straight to fd 1 — which,
        // under `nest mcp`, is the JSON-RPC channel. Without the capture-divert
        // (`write_term_bytes`), those bytes corrupt the stream and wedge the client.
        // With it: the call returns a clean result envelope and the rendered escapes
        // ride back inside the captured-stdout content block.
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": "eval", "arguments": {
                        "source": "(term-draw [[:clear] [:text 0 0 \"ab\"]])" } }),
                ),
                notif("exit", json!(null)),
            ],
        );
        assert!(
            resp[0].get("result").is_some(),
            "term-draw must return a clean result envelope, got {:?}",
            resp[0]
        );
        let content = resp[0]["result"]["content"].as_array().unwrap();
        let joined: String = content
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect();
        assert!(
            joined.contains("[2J"),
            "rendered escapes should be diverted into the result content (not the raw \
             channel): {joined:?}"
        );
    }

    #[test]
    fn eval_deadline_aborts_a_runaway_inline() {
        // The MCP watchdog: a runaway eval (here an infinite tail loop) is aborted by
        // the inline deadline (scheduler `DEADLINE`, ADR-063) and surfaces as an
        // ordinary error — not a hang — so the server keeps serving. Inline, so it
        // doesn't disturb the dispatcher's error/panic/output handling. A short
        // deadline stands in for the dispatcher's 30s.
        let mut interp = Interp::new();
        interp.eval_str("(defn ginf () (ginf))").unwrap();
        brood::process::set_deadline(Some(
            std::time::Instant::now() + std::time::Duration::from_millis(300),
        ));
        let r = interp.eval_str("(ginf)");
        brood::process::set_deadline(None);
        let err = r.expect_err("a runaway must be aborted by the deadline, not hang");
        let msg = format!("{err}");
        assert!(
            msg.contains("time limit"),
            "expected a time-limit error, got: {msg}"
        );
    }

    #[test]
    fn tools_call_returns_an_error_for_an_unknown_tool() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "tools/call", json!({ "name": "nope", "arguments": {} })),
                notif("exit", json!(null)),
            ],
        );
        assert_eq!(resp[0]["error"]["code"], -32602);
        assert!(resp[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no such tool"));
    }

    #[test]
    fn resources_list_includes_the_baked_doc_resources() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "resources/list", json!({})),
                notif("exit", json!(null)),
            ],
        );
        let resources = resp[0]["result"]["resources"].as_array().unwrap();
        let uris: Vec<&str> = resources
            .iter()
            .map(|r| r["uri"].as_str().unwrap())
            .collect();
        assert!(uris.contains(&"brood://docs/brood-for-claude"));
        assert!(uris.contains(&"brood://prelude"));
        // The incarnations index + its companion docs (added in the
        // llm-native bundle); the agent's orientation funnel relies on
        // these being discoverable.
        assert!(uris.contains(&"brood://docs/incarnations"));
        assert!(uris.contains(&"brood://docs/llm-native"));
        assert!(uris.contains(&"brood://docs/claude-demo-findings"));
        // Stable error-code reference (structured errors, §4).
        assert!(uris.contains(&"brood://docs/error-codes"));
    }

    #[test]
    fn resources_read_returns_the_baked_text() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "resources/read", json!({ "uri": "brood://prelude" })),
                notif("exit", json!(null)),
            ],
        );
        let contents = &resp[0]["result"]["contents"][0];
        assert_eq!(contents["uri"], "brood://prelude");
        // The prelude opens with a defining form — just check we got real
        // content rather than an empty payload.
        assert!(contents["text"].as_str().unwrap().len() > 100);
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "no/such/method", json!({})),
                notif("exit", json!(null)),
            ],
        );
        assert_eq!(resp[0]["error"]["code"], -32601);
    }

    #[test]
    fn unknown_notification_is_dropped_silently() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                notif("bogus/notification", json!({"x": 1})),
                req(1, "ping", json!({})),
                notif("exit", json!(null)),
            ],
        );
        // The bogus notification produced no reply; `ping` did.
        assert_eq!(resp.len(), 1);
        assert_eq!(resp[0]["result"], json!({}));
    }

    #[test]
    fn ping_returns_an_empty_result() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[req(1, "ping", json!({})), notif("exit", json!(null))],
        );
        assert_eq!(resp[0]["result"], json!({}));
    }

    #[test]
    fn shutdown_then_exit_terminates_the_loop() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[req(1, "shutdown", json!({})), notif("exit", json!(null))],
        );
        // `shutdown` replies with `null`; `exit` produces no reply.
        assert_eq!(resp.len(), 1);
        assert_eq!(resp[0]["result"], Json::Null);
    }

    // ---- Brood ↔ JSON converters ---------------------------------------------

    #[test]
    fn json_round_trips_through_brood_for_data_kinds() {
        // Build a JSON value, project into Brood, project back, expect
        // structural equivalence (array→list→array, object→map→object).
        let mut interp = Interp::new();
        let input = json!({
            "n": 42,
            "f": 1.5,
            "s": "hello",
            "items": [1, 2, 3],
            "nested": { "k": "v" },
            "flag": true,
            "absent": null,
        });
        let v = json_to_value(&mut interp.heap, &input).unwrap();
        let back = value_to_json(&interp.heap, v).unwrap();
        assert_eq!(input, back);
    }

    #[test]
    fn value_to_json_rejects_colliding_keys() {
        // `:foo` (keyword) and `"foo"` (string) both render to the JSON key
        // "foo" — so a map carrying both would silently lose one. That's data
        // loss, so `value_to_json` must error rather than pick a winner.
        let mut interp = Interp::new();
        let collide = interp.eval_str(r#"{:foo 1 "foo" 2}"#).unwrap();
        let err = value_to_json(&interp.heap, collide)
            .expect_err("colliding JSON keys must be a loud error");
        assert!(err.contains("colliding"), "{err}");
        // A map with genuinely distinct JSON keys still converts fine.
        let ok = interp.eval_str(r#"{:foo 1 :bar 2}"#).unwrap();
        assert!(value_to_json(&interp.heap, ok).is_ok());
    }

    #[test]
    fn value_to_json_rejects_unrepresentable_kinds() {
        // A closure can't be JSON. The tool catalogue holds these — but
        // `value_to_json` won't ever see them at the top level (`tool_entry_to_json`
        // pulls `:schema` and discards `:handler`), so a tool that *returns* a
        // closure surfaces this honest failure rather than silently dropping it.
        let mut interp = Interp::new();
        let cl = interp.eval_str("(fn (x) x)").unwrap();
        assert!(value_to_json(&interp.heap, cl).is_err());
    }

    // ---- step 3 — end-to-end against the baked std/mcp.blsp catalogue --------
    //
    // Each test fires a real `tools/call` for one of the six live tools and
    // asserts on the parsed JSON in the `content[0].text` payload (the Brood
    // result's `pretty_print`ed JSON). The remaining two — `check` and
    // `run-tests` — ship as documented stubs; we pin their `:error` message
    // here so a future un-stub doesn't silently regress the contract.

    /// Send one `tools/call`, parse the dispatcher's `content[0].text` back
    /// into JSON, and hand it to the assertion closure. Returns the *raw*
    /// response too so tests can read `error`-shaped replies as well.
    fn invoke_tool(interp: &mut Interp, name: &str, arguments: Json) -> (Json, Option<Json>) {
        let resp = round_trip(
            interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": name, "arguments": arguments }),
                ),
                notif("exit", json!(null)),
            ],
        );
        let parsed = resp[0]["result"]["content"][0]["text"]
            .as_str()
            .map(|s| serde_json::from_str::<Json>(s).expect("payload was not JSON"));
        (resp[0].clone(), parsed)
    }

    #[test]
    fn std_eval_tool_returns_the_printed_value() {
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(&mut interp, "eval", json!({ "source": "(+ 1 2)" }));
        assert_eq!(body.unwrap()["value"], "3");
    }

    #[test]
    fn std_eval_tool_captures_a_runtime_error_as_a_structured_map() {
        // After structured errors (`docs/llm-native.md` §4), a caught built-in
        // error is a map with `:kind` / `:code` / `:message` — the agent can
        // branch on `:kind` without parsing strings. `(no-such-fn 1)` raises
        // unbound; we pin both the kind and the stable code.
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(&mut interp, "eval", json!({ "source": "(no-such-fn 1)" }));
        let body = body.unwrap();
        assert!(body.get("value").is_none(), "{body:?}");
        let err = &body["error"];
        assert!(
            err.is_object(),
            "expected :error to be a structured map, got {err}"
        );
        assert_eq!(err["kind"], "unbound");
        assert_eq!(err["code"], "E0010");
        assert!(!err["message"].as_str().unwrap().is_empty());
    }

    #[test]
    fn std_eval_tool_state_persists_across_calls() {
        // The hot-reload promise: a `def` in one tool call is visible to the next.
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": "eval", "arguments": { "source": "(def mcp-test-x 7)" } }),
                ),
                req(
                    2,
                    "tools/call",
                    json!({ "name": "eval", "arguments": { "source": "(* mcp-test-x 6)" } }),
                ),
                notif("exit", json!(null)),
            ],
        );
        let second = resp[1]["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Json = serde_json::from_str(second).unwrap();
        assert_eq!(parsed["value"], "42");
    }

    #[test]
    fn std_lookup_tool_describes_a_prelude_fn() {
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(&mut interp, "lookup", json!({ "name": "map" }));
        let body = body.unwrap();
        assert_eq!(body["name"], "map");
        // `arglist` for the prelude `map` is a non-empty list. JSON-shape: array.
        assert!(body["arglist"].is_array());
        assert!(!body["arglist"].as_array().unwrap().is_empty());
        // Prelude defs now *do* carry a source location — the prelude build
        // materialises a copy to `$XDG_CACHE_HOME/brood/prelude.blsp` and
        // reads it positioned, so `M-.` can land inside the standard library
        // (ADR-031 step 4). The lookup returns the cache path + line + col.
        let loc = &body["source-location"];
        assert!(loc.is_array(), "expected source-location array: {loc}");
        let arr = loc.as_array().unwrap();
        assert_eq!(arr.len(), 3, "expected [path line col]");
        let path = arr[0].as_str().unwrap_or("");
        assert!(
            path.ends_with("prelude.blsp"),
            "expected prelude cache path, got {path:?}"
        );
    }

    #[test]
    fn std_lookup_tool_handles_unbound_names_softly() {
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(
            &mut interp,
            "lookup",
            json!({ "name": "no-such-name-xyzzy" }),
        );
        let body = body.unwrap();
        // Unbound is a soft failure surfaced as :error, not a thrown exception
        // (the dispatcher would render that as a JSON-RPC error). After
        // structured errors (§4), the :error field is the kernel-shaped map —
        // the agent branches on `:kind` / `:code` rather than parsing a string.
        assert_eq!(body["name"], "no-such-name-xyzzy");
        let err = &body["error"];
        assert!(err.is_object(), "expected :error to be a map: {err}");
        assert_eq!(err["kind"], "unbound");
        assert_eq!(err["code"], "E0010");
    }

    #[test]
    fn std_macroexpand_tool_steps_a_when() {
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(
            &mut interp,
            "macroexpand",
            json!({ "form": "(when x 1)", "mode": "1" }),
        );
        let expanded = body.unwrap()["expanded"].as_str().unwrap().to_string();
        // `(when c e)` lowers to an `if`-shaped form; we don't pin the exact
        // expansion (let `docs/macros` evolve it) — only that the conditional
        // shape is there.
        assert!(expanded.contains("if"), "got {expanded:?}");
    }

    #[test]
    fn std_format_tool_reformats_messy_source() {
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(
            &mut interp,
            "format",
            json!({ "source": "(  +  1   2  )\n\n\n" }),
        );
        let formatted = body.unwrap()["formatted"].as_str().unwrap().to_string();
        assert!(!formatted.is_empty());
        // Idempotent: feeding the formatted source back is a fixed point.
        let (_, body2) = invoke_tool(
            &mut interp,
            "format",
            json!({ "source": formatted.clone() }),
        );
        assert_eq!(body2.unwrap()["formatted"].as_str().unwrap(), formatted);
    }

    #[test]
    fn run_tests_structured_returns_a_structured_summary() {
        // Drive the underlying `(test/run-tests-structured)` directly — invoking
        // the `run-tests` MCP tool would discover and run the workspace's
        // entire in-language suite (cwd-dependent), which is slow and
        // potentially recursive in CI. Register two inline tests and verify
        // the result map carries the documented keys.
        let mut interp = Interp::new();
        interp
            .eval_str(
                r#"
                (require 'test)
                (test/test "always-ok" (test/assert= 1 1))
                "#,
            )
            .unwrap();
        let result = interp.eval_str("(test/run-tests-structured)").unwrap();
        let printed = interp.print(result);
        // Pin the contract keys without counting (the test framework can
        // auto-register tests of its own across versions).
        for key in &[":total", ":passed", ":failed", ":ms", ":results"] {
            assert!(printed.contains(key), "missing {key}: {printed}");
        }
    }

    #[test]
    fn std_check_tool_returns_structured_diagnostics_or_an_error() {
        // After step 1c-a, `check` calls `(project/check-project-structured)` and
        // returns either `{:diagnostics [...]}` (when invoked from inside a
        // Brood project — the workspace root in `cargo test`'s cwd usually
        // is one) or `{:error msg}` (when it isn't). Either shape passes;
        // what *must not* be present is the old "not yet wired" stub marker.
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(&mut interp, "check", json!({}));
        let body = body.unwrap();
        let has_diag = body["diagnostics"].is_array();
        let has_err = body["error"].is_string();
        assert!(
            has_diag || has_err,
            "neither :diagnostics nor :error: {body:?}"
        );
        if let Some(err) = body["error"].as_str() {
            assert!(!err.contains("not yet wired"), "still a stub: {err:?}");
        }
    }

    #[test]
    fn std_processes_tool_returns_process_info_maps() {
        // `processes` maps `(process-info pid)` over `(list-processes)`, so each
        // entry is the full per-process stat map the observer reads — not a bare
        // pid. There's always at least *some* registered mailbox by the time a
        // tool call executes (the dispatcher's eval runs in a registered
        // process), so the list is non-empty. The map's `:pid` field is itself a
        // tagged `{$type: "pid"}` object (see `value_to_json`).
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(&mut interp, "processes", json!({}));
        let body = body.unwrap();
        let procs = body["processes"]
            .as_array()
            .expect("expected :processes to be an array");
        assert!(!procs.is_empty(), "no live processes?");
        for p in procs {
            assert!(p["id"].is_number(), "{p:?}");
            assert!(p["mailbox"].is_number(), "{p:?}");
            assert!(p["reductions"].is_number(), "{p:?}");
            assert!(p["node"].is_string(), "{p:?}");
            assert_eq!(p["pid"]["$type"], "pid", "{p:?}");
        }
    }

    #[test]
    fn std_node_tool_returns_runtime_stats() {
        let mut interp = Interp::new();
        let (_, body) = invoke_tool(&mut interp, "node", json!({}));
        let body = body.unwrap();
        assert!(body["node"].is_string(), "{body:?}");
        assert!(body["workers"].is_number(), "{body:?}");
        assert!(body["process-count"].is_number(), "{body:?}");
        assert!(body["mem-bytes"].is_number(), "{body:?}");
        assert!(body["peers"].is_array(), "{body:?}");
    }

    #[test]
    fn std_process_info_tool_looks_up_by_id() {
        let mut interp = Interp::new();
        // Grab a live id from `processes`, then look it up by that integer id.
        let (_, listing) = invoke_tool(&mut interp, "processes", json!({}));
        let listing = listing.unwrap();
        let id = listing["processes"][0]["id"].as_i64().expect("a live id");
        let (_, body) = invoke_tool(&mut interp, "process-info", json!({ "id": id }));
        let body = body.unwrap();
        assert_eq!(body["id"], id, "{body:?}");
        assert!(body["reductions"].is_number(), "{body:?}");
        // A bogus id yields a soft error map, not a thrown tool error.
        let (_, miss) = invoke_tool(&mut interp, "process-info", json!({ "id": 9_999_999 }));
        assert!(miss.unwrap()["error"].is_string());
    }

    /// A fresh interp with `*project-root*` pinned to a unique temp dir, so the
    /// sandboxed `write`/`edit` tools have a project to write into. Returns the
    /// root path. The first `eval` call triggers the dispatcher's `(require
    /// 'mcp)` (which loads `project`, defining `*project-root*` as nil); we then
    /// rebind it — a later `(require 'mcp)` is idempotent and won't reset it.
    fn interp_with_project_root(tag: &str) -> (Interp, std::path::PathBuf) {
        let mut interp = Interp::new();
        let _ = invoke_tool(&mut interp, "eval", json!({ "source": "1" }));
        let root = std::env::temp_dir().join(format!("brood-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        interp
            .eval_str(&format!("(def *project-root* {:?})", root.to_str().unwrap()))
            .unwrap();
        (interp, root)
    }

    #[test]
    fn std_write_tool_writes_a_blsp_file_into_the_project_and_loads_it() {
        let (mut interp, root) = interp_with_project_root("write");
        let (_, body) = invoke_tool(
            &mut interp,
            "write",
            json!({ "path": "src/gen.blsp", "content": "(defn gen-answer () 42)" }),
        );
        let body = body.unwrap();
        assert_eq!(body["ok"], true, "{body:?}");
        assert_eq!(body["path"], "src/gen.blsp");
        // The file landed on disk under the project root...
        let on_disk = std::fs::read_to_string(root.join("src/gen.blsp")).unwrap();
        assert_eq!(on_disk, "(defn gen-answer () 42)");
        // ...and `.blsp` content was loaded into the live image (def is callable).
        let (_, called) = invoke_tool(&mut interp, "eval", json!({ "source": "(gen-answer)" }));
        assert_eq!(called.unwrap()["value"], "42");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn std_write_tool_refuses_to_escape_the_project_root() {
        let (mut interp, root) = interp_with_project_root("escape");
        for bad in ["../escape.blsp", "/etc/passwd", "~/secret", "a/../../b.blsp"] {
            let (_, body) = invoke_tool(
                &mut interp,
                "write",
                json!({ "path": bad, "content": "nope" }),
            );
            let body = body.unwrap();
            assert_eq!(body["ok"], false, "should reject {bad:?}: {body:?}");
        }
        // None of those wrote anything under (or above) the root.
        assert!(!root.exists(), "sandbox-violating write created files");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn std_edit_tool_replaces_unique_text_and_rejects_ambiguity() {
        let (mut interp, root) = interp_with_project_root("edit");
        invoke_tool(
            &mut interp,
            "write",
            json!({ "path": "notes.txt", "content": "alpha beta beta gamma" }),
        );
        // Ambiguous: "beta" occurs twice → soft error, file untouched.
        let (_, dup) = invoke_tool(
            &mut interp,
            "edit",
            json!({ "path": "notes.txt", "old": "beta", "new": "X" }),
        );
        assert_eq!(dup.unwrap()["ok"], false);
        // Unique: "alpha" once → replaced.
        let (_, ok) = invoke_tool(
            &mut interp,
            "edit",
            json!({ "path": "notes.txt", "old": "alpha", "new": "ALPHA" }),
        );
        assert_eq!(ok.unwrap()["ok"], true);
        assert_eq!(
            std::fs::read_to_string(root.join("notes.txt")).unwrap(),
            "ALPHA beta beta gamma"
        );
        // Missing file → soft error.
        let (_, miss) = invoke_tool(
            &mut interp,
            "edit",
            json!({ "path": "nope.txt", "old": "x", "new": "y" }),
        );
        assert_eq!(miss.unwrap()["ok"], false);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prompts_list_includes_brood_task() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "prompts/list", json!({})),
                notif("exit", json!(null)),
            ],
        );
        let prompts = resp[0]["result"]["prompts"].as_array().unwrap();
        let names: Vec<&str> = prompts
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"brood-task"), "{names:?}");
    }

    #[test]
    fn prompts_get_returns_the_orientation_message() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "prompts/get", json!({ "name": "brood-task" })),
                notif("exit", json!(null)),
            ],
        );
        let messages = resp[0]["result"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        let text = messages[0]["content"]["text"].as_str().unwrap();
        // Pin the orientation pointers — the prompt is a *contract*, the
        // agent reads it once at session start, so we don't want it to
        // silently drift to something un-useful.
        assert!(text.contains("brood://docs/brood-for-claude"), "{text}");
        assert!(text.contains("immutable"), "{text}");
        assert!(text.contains("MCP tools"), "{text}");
    }

    #[test]
    fn prompts_get_returns_an_error_for_unknown_names() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "prompts/get", json!({ "name": "no-such-prompt" })),
                notif("exit", json!(null)),
            ],
        );
        assert_eq!(resp[0]["error"]["code"], -32602);
    }

    #[test]
    fn value_to_json_renders_pids_as_tagged_objects() {
        // The tagged-object shape pids round-trip through is part of the MCP
        // contract — `(list-processes)` and any handler returning a pid relies
        // on it. Pin both fields.
        let mut interp = Interp::new();
        let pid = interp.eval_str("(self)").unwrap();
        let json = value_to_json(&interp.heap, pid).unwrap();
        assert_eq!(json["$type"], "pid");
        assert!(json["id"].is_number());
        assert!(json["node"].is_string());
    }

    #[test]
    fn argument_validation_throws_a_protocol_error() {
        // The handlers `throw` when `:source` / `:file` / `:name` is missing or
        // wrong-typed; the dispatcher converts the throw into a JSON-RPC error
        // (so a misshapen `arguments` from the agent never looks like a
        // *value*, it looks like a *protocol failure*). Since structured errors
        // landed (§4), the JSON-RPC `error.data` carries the kind/code/file/etc.
        // so the agent can branch on it programmatically — the human-readable
        // `error.message` stays alongside.
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": "eval", "arguments": { "source": 42 } }),
                ),
                notif("exit", json!(null)),
            ],
        );
        assert!(resp[0]["error"].is_object(), "{:?}", resp[0]);
        assert!(resp[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains(":source"));
        // A `(throw "...")` from Brood lands as a `:user` kind in the
        // structured data — `(throw v)` keeps `v` opaque to the kernel, so no
        // `:code` (those are for kernel-raised errors).
        let data = &resp[0]["error"]["data"];
        assert!(data.is_object(), "expected error.data: {:?}", resp[0]);
        assert_eq!(data["kind"], "user");
    }

    #[test]
    fn uncaught_handler_throw_projects_structured_data() {
        // A project's own tool whose handler doesn't try/catch surfaces the
        // kernel error through the JSON-RPC `error.data` field. Build a
        // catalogue inline (the override path — `(provide 'mcp)` so the
        // std catalogue doesn't clobber ours) where the handler triggers
        // a built-in error (`(/ 1 0)` → runtime).
        let mut interp = Interp::new();
        interp
            .eval_str(
                r#"
                (provide 'mcp)
                (defn mcp/mcp-tools ()
                  (list
                    {:name "blow-up"
                     :schema {:type "object" :properties {}}
                     :handler (fn (_) (/ 1 0))}))
                "#,
            )
            .unwrap();
        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({ "name": "blow-up", "arguments": {} }),
                ),
                notif("exit", json!(null)),
            ],
        );
        let err = &resp[0]["error"];
        assert_eq!(err["code"], -32603, "{err}"); // JSON-RPC internal
        let data = &err["data"];
        assert_eq!(data["kind"], "runtime");
        // `(/ 1 0)` carries the specific `E0040` code (div-by-zero); the
        // generic `E0099` is the runtime catch-all for raises that haven't
        // been tagged with a specific code yet (see `docs/error-codes.md`).
        assert_eq!(data["code"], "E0040");
        assert!(data["message"]
            .as_str()
            .unwrap()
            .contains("division by zero"));
    }

    #[test]
    #[cfg(debug_assertions)]
    fn handler_panic_is_caught_and_server_keeps_serving() {
        // Regression for the MCP-host panic-isolation behaviour
        // (`docs/deferred.md` §3): a *Rust panic* inside a tool handler must
        // surface as a structured JSON-RPC error and NOT tear down the server.
        // Before the `catch_unwind` wrap in `call_tool`, any panic propagated
        // through `main_loop` and dropped every `mcp__brood__*` tool for the
        // rest of the session.
        //
        // We trigger the panic via `%force-panic` — a debug-only kernel
        // primitive whose only job is to `panic!()`, giving this test a
        // reliable trigger without putting an "intentionally crash" knob in
        // the release surface (`#[cfg(debug_assertions)]`-gated in
        // `builtins.rs`).
        //
        // Without the panic hook silenced, the panic backtrace is also
        // printed to stderr. That's a side effect of `panic::catch_unwind`'s
        // contract — useful for debugging server-side, doesn't corrupt the
        // stdio JSON-RPC channel (stderr is separate).
        let mut interp = Interp::new();
        interp
            .eval_str(
                r#"
                (provide 'mcp)
                (defn mcp/mcp-tools ()
                  (list
                    {:name "boom"
                     :schema {:type "object" :properties {}}
                     :handler (fn (_) (%force-panic "stunt panic for test"))}
                    {:name "echo"
                     :schema {:type "object" :properties {:n {:type "integer"}}}
                     :handler (fn (args) (get args :n))}))
                "#,
            )
            .unwrap();

        // Silence the default panic hook for the duration of this test only,
        // so cargo's test output stays clean. We restore it on exit. The hook
        // is process-wide, so other concurrent tests would see this — but the
        // test binary defaults to single-threaded-per-test for unit tests in
        // the same module under `cargo test --no-fail-fast`, and crucially
        // the next assertion (subsequent tool call succeeds) is the proof,
        // not stderr.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            round_trip(
                &mut interp,
                &[
                    // First call panics inside the handler.
                    req(
                        1,
                        "tools/call",
                        json!({ "name": "boom", "arguments": {} }),
                    ),
                    // Second call must still work — proves the server didn't
                    // die and `Interp` is in a usable state.
                    req(
                        2,
                        "tools/call",
                        json!({ "name": "echo", "arguments": { "n": 7 } }),
                    ),
                    notif("exit", json!(null)),
                ],
            )
        }));
        std::panic::set_hook(prev);
        let resp = result.expect("the MCP server itself must not unwind");

        // First reply: structured panic error.
        let err = &resp[0]["error"];
        assert_eq!(err["code"], -32603, "{err}"); // JSON-RPC internal
        assert!(
            err["message"]
                .as_str()
                .unwrap()
                .contains("panic in tool handler"),
            "message should mark this as a panic: {err}"
        );
        let data = &err["data"];
        assert_eq!(data["kind"], "panic");
        assert!(
            data["message"]
                .as_str()
                .unwrap()
                .contains("stunt panic for test"),
            "the original panic message must round-trip: {data}"
        );
        assert!(
            data["hint"].as_str().unwrap().contains("interpreter bug"),
            "the hint should call this an interpreter bug: {data}"
        );

        // Second reply: the server is still alive and the next tool call works.
        let content = &resp[1]["result"]["content"][0];
        assert_eq!(content["type"], "text");
        assert_eq!(content["text"], "7");
    }
}
