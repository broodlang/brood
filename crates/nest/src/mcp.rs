//! `nest mcp` — the Model Context Protocol dispatcher for a Brood project.
//!
//! A synchronous JSON-RPC loop over stdio (Content-Length framing, the same
//! shape LSP uses) scoped strictly to a single project (ADR-036, ADR-028).
//! The caller in `main.rs` walks up to `project.blsp`, builds + bootstraps an
//! [`Interp`], and hands it here; `run` owns the protocol from that point on.
//!
//! ## Protocol surface (v0 / step 2)
//!
//! - `initialize`            — return server info + capabilities.
//! - `initialized` (notif)    — acknowledged, no reply.
//! - `tools/list`            — call `(mcp-tools)` in the session's Brood image
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
//! contract, ADR-013). `(mcp-tools)` is re-evaluated on every `tools/list`
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
use brood::core::value::{self, Value};
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
// Transport — Content-Length framing + JSON-RPC envelope
// ============================================================================

/// Read one framed JSON message. Returns `Ok(None)` at clean EOF (peer closed
/// the channel — exit cleanly). A header without a `Content-Length` or a body
/// that doesn't parse is `InvalidData` (the protocol is broken, not a soft
/// EOF, so propagate and let the caller fail loudly).
fn read_message<R: BufRead>(r: &mut R) -> std::io::Result<Option<Json>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            // EOF *between* messages is clean — that's the "peer hung up"
            // exit. EOF *mid-header* (header started but no blank line) is
            // not distinguishable here without more bookkeeping; treat any
            // top-of-loop EOF as clean and let downstream handle a truncated
            // header by failing the body read instead.
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().ok();
        }
        // Any other header (Content-Type, etc.) is ignored — MCP doesn't
        // require us to validate them.
    }
    let len = content_length.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing or malformed Content-Length",
        )
    })?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let msg: Json = serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(msg))
}

/// Write one framed JSON message. The framing is `Content-Length: N\r\n\r\n`
/// followed by `N` bytes of body — same as LSP.
fn write_message<W: Write>(w: &mut W, msg: &Json) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
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

/// The synchronous request loop. Pulled out of [`run`] so tests can drive it
/// with in-memory `Cursor` / `Vec<u8>` channels.
fn main_loop<R: BufRead, W: Write>(
    interp: &mut Interp,
    r: &mut R,
    w: &mut W,
) -> Result<(), Box<dyn Error>> {
    while let Some(msg) = read_message(r)? {
        match dispatch(interp, &msg) {
            Outcome::Reply(resp) => write_message(w, &resp)?,
            Outcome::NoReply => {}
            Outcome::Exit => return Ok(()),
        }
    }
    Ok(())
}

/// Route one message to its handler. A `method` we don't know:
/// - **with `id`** (a request) → reply `MethodNotFound`, per JSON-RPC.
/// - **without `id`** (a notification) → drop silently, per JSON-RPC.
fn dispatch(interp: &mut Interp, msg: &Json) -> Outcome {
    let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Json::Null);

    // Notifications carry no id; the only one we currently *act on* is `exit`
    // (which stops the loop) and `initialized` (acknowledged silently).
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
        "prompts/list" => Ok(json!({ "prompts": [] })),
        "ping" => Ok(json!({})),
        "shutdown" => Ok(Json::Null),
        other => Err(RpcError::method_not_found(other)),
    };

    Outcome::Reply(envelope(id.unwrap(), result))
}

/// Wrap a per-handler result in the JSON-RPC response envelope.
fn envelope(id: Json, result: Result<Json, RpcError>) -> Json {
    match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": e.code, "message": e.message },
        }),
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

/// Project the Brood-side tool catalogue (`(mcp-tools)`, in `std/mcp.blsp` —
/// and any project-side extensions step 3 introduces) to the JSON shape
/// `tools/list` requires. A missing `std/mcp.blsp` (or any error) collapses
/// to an empty list — the server stays useful, just with no tools yet.
fn list_tools(interp: &mut Interp) -> Vec<Json> {
    let cp = interp.heap.checkpoint();
    let roots_base = interp.heap.roots_len();

    // Best-effort require — silently ignore "no such module" so the server
    // works the moment it boots, before `std/mcp.blsp` exists (step 3) and
    // even if a project hasn't defined its own MCP extensions yet.
    let _ = interp.eval_str("(require 'mcp)");

    let tools = match interp.eval_str("(mcp-tools)") {
        Ok(v) => {
            interp.heap.push_root(v);
            project_tool_catalogue(&interp.heap, v).unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };

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
    Ok(items.into_iter().filter_map(|item| tool_entry_to_json(heap, item)).collect())
}

/// Convert one Brood map of tool metadata to the MCP shape `tools/list`
/// returns. Missing `:name` or `:schema` fails the entry; missing
/// `:description` is fine (omitted in the JSON).
fn tool_entry_to_json(heap: &Heap, entry: Value) -> Option<Json> {
    let map_id = match entry {
        Value::Map(id) => id,
        _ => return None,
    };
    let map = heap.map(map_id);
    let name = map_get_kw(map, "name").and_then(|v| match v {
        Value::Str(id) => Some(heap.string(id).to_string()),
        _ => None,
    })?;
    let schema = map_get_kw(map, "schema")?;
    let schema_json = value_to_json(heap, schema).ok()?;
    let mut obj = JsonMap::new();
    obj.insert("name".into(), Json::String(name));
    obj.insert("inputSchema".into(), schema_json);
    if let Some(Value::Str(id)) = map_get_kw(map, "description") {
        obj.insert("description".into(), Json::String(heap.string(id).to_string()));
    }
    Some(Json::Object(obj))
}

/// Look up a keyword-keyed entry in a Brood map: `(get m :kw)` in Rust. The
/// keyword name has to intern, so callers pass a `&str`.
fn map_get_kw(map: &[(Value, Value)], kw: &str) -> Option<Value> {
    let target = value::intern(kw);
    map.iter()
        .find(|(k, _)| matches!(k, Value::Keyword(s) if *s == target))
        .map(|(_, v)| *v)
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

    let outcome = (|| -> Result<Json, RpcError> {
        // Re-fetch the catalogue per call so a `def` in a previous `eval`
        // call (hot reload) reshapes the tool surface immediately.
        let _ = interp.eval_str("(require 'mcp)");
        let tools = interp
            .eval_str("(mcp-tools)")
            .map_err(|_| RpcError::invalid_params(format!("no such tool: {name}")))?;
        interp.heap.push_root(tools);

        let handler = find_handler(&interp.heap, tools, &name)
            .ok_or_else(|| RpcError::invalid_params(format!("no such tool: {name}")))?;
        // Closures from `defn` are RUNTIME (so stable across LOCAL resets),
        // but `apply` may itself fire GC at its outermost safepoint — push
        // anything we hold across it.
        interp.heap.push_root(handler);

        let args_value = json_to_value(&mut interp.heap, &arguments)
            .map_err(RpcError::invalid_params)?;
        interp.heap.push_root(args_value);

        let result_value =
            brood::eval::apply(&mut interp.heap, handler, &[args_value], interp.root)
                .map_err(|e| RpcError::internal(e.to_string()))?;

        let content = value_to_json(&interp.heap, result_value)
            .map_err(RpcError::internal)?;
        Ok(wrap_as_mcp_content(content))
    })();

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
        let map = heap.map(map_id);
        let item_name = match map_get_kw(map, "name") {
            Some(Value::Str(id)) => heap.string(id),
            _ => continue,
        };
        if item_name == name {
            return map_get_kw(map, "handler");
        }
    }
    None
}

/// MCP `tools/call` returns `{ content: [{type: "text", text: "..."}] }`.
/// Plain strings pass through; structured values are pretty-printed JSON.
/// (`structuredContent` is a recent MCP addition; sticking to `text` for v0
/// maximises client compatibility, ADR-011.)
fn wrap_as_mcp_content(content: Json) -> Json {
    let text = match &content {
        Json::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    json!({ "content": [{ "type": "text", "text": text }] })
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
        Value::Pair(_) | Value::Vector(_) => {
            let items = heap.seq_items(v).map_err(|e| e.to_string())?;
            items.into_iter().map(|x| value_to_json(heap, x)).collect()
        }
        Value::Map(id) => {
            let mut obj = JsonMap::new();
            for (k, val) in heap.map(id) {
                let key = match *k {
                    Value::Str(id) => heap.string(id).to_string(),
                    Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
                    other => {
                        return Err(format!(
                            "map key must be string/keyword/symbol for JSON, got {:?}",
                            value::tag(other)
                        ))
                    }
                };
                obj.insert(key, value_to_json(heap, *val)?);
            }
            Ok(Json::Object(obj))
        }
        Value::Fn(_) | Value::Macro(_) | Value::Native(_) | Value::Ref(_) | Value::Pid { .. } => {
            Err(format!(
                "value of kind {:?} has no JSON representation",
                value::tag(v)
            ))
        }
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
            Ok(heap.alloc_map(entries))
        }
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Minimal JSON-RPC error. Codes follow the spec —
/// <https://www.jsonrpc.org/specification#error_object>.
struct RpcError {
    code: i32,
    message: String,
}

impl RpcError {
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }
    fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
        }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
        }
    }
}

// ============================================================================
// Tests — drive `main_loop` with in-memory I/O (the LSP's `Connection::memory`
// pattern, adapted to plain BufRead/Write).
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a Content-Length-framed buffer from a list of JSON messages.
    fn frame(messages: &[Json]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in messages {
            let body = serde_json::to_vec(m).unwrap();
            write!(buf, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
            buf.extend_from_slice(&body);
        }
        buf
    }

    /// Parse a server's stream of framed JSON responses out of a `Vec<u8>`.
    fn unframe(output: &[u8]) -> Vec<Json> {
        let mut r = Cursor::new(output);
        let mut out = Vec::new();
        while let Ok(Some(m)) = read_message(&mut r) {
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
    fn initialize_returns_server_info_and_capabilities() {
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[
                req(1, "initialize", json!({})),
                notif("exit", json!(null)),
            ],
        );
        assert_eq!(resp.len(), 1);
        let result = &resp[0]["result"];
        assert_eq!(result["serverInfo"]["name"], "nest-mcp");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
        assert!(result["capabilities"]["prompts"].is_object());
    }

    #[test]
    fn tools_list_is_empty_when_no_catalogue_is_defined() {
        // No `std/mcp.blsp` exists yet (step 3 work); the dispatcher must
        // still serve an empty list rather than error.
        let mut interp = Interp::new();
        let resp = round_trip(
            &mut interp,
            &[req(1, "tools/list", json!({})), notif("exit", json!(null))],
        );
        assert_eq!(resp[0]["result"]["tools"], json!([]));
    }

    #[test]
    fn tools_list_projects_a_brood_defined_catalogue() {
        let mut interp = Interp::new();
        // Pre-define an `mcp-tools` catalogue inline — the same shape
        // `std/mcp.blsp` will use in step 3, so this test pins the contract.
        interp
            .eval_str(
                r#"
                (defn mcp-tools ()
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
        interp
            .eval_str(
                r#"
                (defn mcp-tools ()
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
                req(1, "tools/call", json!({ "name": "double", "arguments": { "n": 21 } })),
                notif("exit", json!(null)),
            ],
        );
        let content = &resp[0]["result"]["content"][0];
        assert_eq!(content["type"], "text");
        assert_eq!(content["text"], "42");
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
            &[req(1, "resources/list", json!({})), notif("exit", json!(null))],
        );
        let resources = resp[0]["result"]["resources"].as_array().unwrap();
        let uris: Vec<&str> = resources.iter().map(|r| r["uri"].as_str().unwrap()).collect();
        assert!(uris.contains(&"brood://docs/brood-for-claude"));
        assert!(uris.contains(&"brood://prelude"));
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
    fn value_to_json_rejects_unrepresentable_kinds() {
        // A closure can't be JSON. The tool catalogue holds these — but
        // `value_to_json` won't ever see them at the top level (`tool_entry_to_json`
        // pulls `:schema` and discards `:handler`), so a tool that *returns* a
        // closure surfaces this honest failure rather than silently dropping it.
        let mut interp = Interp::new();
        let cl = interp.eval_str("(fn (x) x)").unwrap();
        assert!(value_to_json(&interp.heap, cl).is_err());
    }
}
