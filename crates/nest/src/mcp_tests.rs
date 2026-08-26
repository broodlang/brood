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
    let mut g2 = StalenessGuard {
        started: future,
        exe: Some(tmp2.clone()),
        warned: false,
    };
    assert!(
        !g2.check(),
        "a binary older than the start time is not stale"
    );
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
                    l (bit/or (bit/and (bit/shift-left b 1) (bit/xor col0 board)) (bit/shift-right (bit/and b high) wm1))
                    r (bit/or (bit/and (bit/shift-right b 1) (bit/xor high board)) (bit/shift-left (bit/and b col0) wm1))
                    up (fn (f) (bit/or (bit/and (bit/shift-left f w) board) (bit/shift-right f hm1w)))
                    dn (fn (f) (bit/or (bit/shift-right f w) (bit/shift-left (bit/and f mask) hm1w)))
                    ns [(up l) (up b) (up r) l r (dn l) (dn b) (dn r)]
                    planes (reduce (fn ([s0 s1 s2 s3] m)
                                     (let (c (bit/and s0 m) s0b (bit/xor s0 m) c2 (bit/and s1 c) s1b (bit/xor s1 c)
                                           c3 (bit/and s2 c2) s2b (bit/xor s2 c2) s3b (bit/or s3 c3))
                                       [s0b s1b s2b s3b]))
                             [0 0 0 0] ns)
                    s0 (vector-ref planes 0) s1 (vector-ref planes 1) s2 (vector-ref planes 2) s3 (vector-ref planes 3))
                (bit/and (bit/and s1 (bit/and (bit/xor s2 board) (bit/xor s3 board))) (bit/or s0 b)))))"#;
    // each call builds the wide masks as LOCAL lets, captured by the closures
    // passed to `ms` and `reduce` (exactly the prototype that crashed).
    let churn = r#"(let (w 200 h 120
                            mask (- (bit/shift-left 1 w) 1)
                            board (- (bit/shift-left 1 (* w h)) 1)
                            col0 (quot board mask)
                            high (bit/shift-left col0 (- w 1))
                            st (bit/and board (bit/shift-left (- (bit/shift-left 1 100) 1) 5000)))
                        (ms (fn () (bit/count (reduce (fn (b _) (wstep b w h mask board col0 high)) st (range 30))))))"#;
    let mut reqs = vec![
        req(1, "initialize", json!({})),
        req(
            2,
            "tools/call",
            json!({ "name": "eval", "arguments": { "source": setup } }),
        ),
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
    // Step 3 ships `std/tool/mcp.blsp` as a baked-in `EMBEDDED_MODULES` entry, so
    // `(require-one 'mcp) (mcp/tools)` succeeds in a fresh `Interp` and the
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
    // Pre-define an `mcp/tools` catalogue inline; mark `'mcp` as already
    // provided so the dispatcher's `(require-one 'mcp)` doesn't load the baked
    // `std/tool/mcp.blsp` and clobber our test catalogue. This is exactly the
    // override path a project's own `mcp.blsp` will use (step 5): provide
    // the feature themselves, then bind their own `mcp/tools`.
    interp
        .eval_str(
            r#"
                (provide 'mcp)
                (defn mcp/tools ()
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
    // claim the feature so the dispatcher's `(require-one 'mcp)` is a no-op
    // and our inline catalogue is what `(mcp/tools)` returns.
    interp
        .eval_str(
            r#"
                (provide 'mcp)
                (defn mcp/tools ()
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
                (defn mcp/tools ()
                  (list
                    {:name "chatty"
                     :schema {:type "object" :properties {}}
                     :handler (fn (_) (io/write "debug line") 42)}))
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
    // term/draw writes terminal escapes via crossterm straight to fd 1 — which,
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
                        "source": "(term/draw [[:clear] [:text 0 0 \"ab\"]])" } }),
            ),
            notif("exit", json!(null)),
        ],
    );
    assert!(
        resp[0].get("result").is_some(),
        "term/draw must return a clean result envelope, got {:?}",
        resp[0]
    );
    let content = resp[0]["result"]["content"].as_array().unwrap();
    let joined: String = content.iter().filter_map(|c| c["text"].as_str()).collect();
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
    let err =
        value_to_json(&interp.heap, collide).expect_err("colliding JSON keys must be a loud error");
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

// ---- step 3 — end-to-end against the baked std/tool/mcp.blsp catalogue --------
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
fn std_check_tool_returns_structured_diagnostics_for_the_served_project() {
    // `check` calls `(project/check-structured *project-root*)` and returns
    // `{:diagnostics [...]}`, or `{:error msg}` when there is no project. What *must
    // not* be present is the old "not yet wired" stub marker.
    //
    // Scoped to a ONE-FILE temp project, via the same `*project-root*` the server pins
    // its write sandbox to. This case used to invoke the tool with no root at all,
    // which fell through to `(cwd)` — so under `cargo test` it type-checked the whole
    // brood repository and took **87 s in CI against a 120 s hard kill** (KI-46), a
    // margin that only ever shrinks as the repo grows. Naming the root is not a
    // shortcut around the work: the tool, the JSON-RPC round trip and the real
    // `check-structured` all still run, over a project whose size this test
    // controls instead of one it inherits from wherever it happens to be run.
    let tmp = std::env::temp_dir().join(format!(
        "nest-mcp-check-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(tmp.join("src")).unwrap();
    std::fs::write(
        tmp.join("project.blsp"),
        b"(project :name \"mcpcheck\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        tmp.join("src/main.blsp"),
        b"(defmodule main \"d\")\n\n(defn main () nil)\n",
    )
    .unwrap();
    // One deliberate warning, so the assertions below can prove WHICH project was
    // checked rather than merely that something came back. An unbound global is the
    // cheapest lint that certainly fires, and it is a check-time warning only — the
    // file still loads, since nothing calls `go`.
    std::fs::write(
        tmp.join("src/oops.blsp"),
        b"(defmodule oops \"d\")\n\n(defn go () (definitely-not-a-real-global 1))\n",
    )
    .unwrap();

    let mut interp = Interp::new();
    interp
        .eval_str(&format!(
            "(require-one 'project) (def *project-root* \"{}\")",
            brood::introspect::escape_brood_string(&tmp.to_string_lossy())
        ))
        .expect("pin the served project root");

    let (_, body) = invoke_tool(&mut interp, "check", json!({}));
    let body = body.unwrap();
    if let Some(err) = body["error"].as_str() {
        panic!("check reported an error instead of diagnostics for the temp project: {err:?}");
    }
    let diagnostics = body["diagnostics"]
        .as_array()
        .unwrap_or_else(|| panic!("no :diagnostics array: {body:?}"));

    // The planted warning must be here, with the documented fields populated. The old
    // cwd-dependent version accepted an empty array OR an error, so it never once
    // proved the diagnostics path produces an entry at all.
    let planted = diagnostics
        .iter()
        .find(|d| {
            d["message"]
                .as_str()
                .is_some_and(|m| m.contains("definitely-not-a-real-global"))
        })
        .unwrap_or_else(|| {
            panic!("the planted unbound-symbol warning is missing: {diagnostics:?}")
        });
    assert!(
        planted["file"]
            .as_str()
            .is_some_and(|f| f.ends_with("oops.blsp")),
        "diagnostic does not name the file it came from: {planted:?}"
    );
    assert!(planted["line"].is_number(), "no :line: {planted:?}");

    // And nothing from OUTSIDE the temp project. This is the assertion that pins the
    // scoping: if `check` ever stops honouring `*project-root*` and falls back to
    // `(cwd)`, it checks the brood repo instead — where the planted warning does not
    // exist, so the `find` above fails outright. Without this pair the regression would
    // show up only as the test getting slow again, which is precisely how KI-46 hid.
    let root = tmp.to_string_lossy().to_string();
    for d in diagnostics {
        let file = d["file"].as_str().unwrap_or_default();
        assert!(
            file.starts_with(&root),
            "checked a file outside the served project root ({root}): {d:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
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
/// rebind it — a later `(require-one 'mcp)` is idempotent and won't reset it.
fn interp_with_project_root(tag: &str) -> (Interp, std::path::PathBuf) {
    let mut interp = Interp::new();
    let _ = invoke_tool(&mut interp, "eval", json!({ "source": "1" }));
    let root = std::env::temp_dir().join(format!("brood-mcp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    interp
        .eval_str(&format!(
            "(def *project-root* {:?})",
            root.to_str().unwrap()
        ))
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
    for bad in [
        "../escape.blsp",
        "/etc/passwd",
        "~/secret",
        "a/../../b.blsp",
    ] {
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
                (defn mcp/tools ()
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
                (defn mcp/tools ()
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
                req(1, "tools/call", json!({ "name": "boom", "arguments": {} })),
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

// ---- MCP progress notifications (the streaming tier) ---------------------

#[test]
fn progress_notification_builds_the_right_shape() {
    let n = progress_notification(&json!("tok-1"), 3, Some(10), Some("halfway"));
    assert_eq!(n["jsonrpc"], "2.0");
    assert_eq!(n["method"], "notifications/progress");
    assert_eq!(n["params"]["progressToken"], "tok-1");
    assert_eq!(n["params"]["progress"], 3);
    assert_eq!(n["params"]["total"], 10);
    assert_eq!(n["params"]["message"], "halfway");
    // A numeric token passes through; total/message are optional.
    let n2 = progress_notification(&json!(42), 1, None, None);
    assert_eq!(n2["params"]["progressToken"], 42);
    assert!(n2["params"].get("total").is_none());
    assert!(n2["params"].get("message").is_none());
}

/// Capture the progress stream emitted while `f` runs (the real path writes
/// to the reentrant stdout lock, which a `Vec` test writer can't see).
fn with_progress_capture(f: impl FnOnce()) -> Vec<Json> {
    let buf = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    super::PROGRESS_TEST_OUT.with(|c| *c.borrow_mut() = Some(buf.clone()));
    f();
    super::PROGRESS_TEST_OUT.with(|c| *c.borrow_mut() = None);
    let bytes = buf.borrow().clone();
    unframe(&bytes)
}

#[test]
fn a_tools_call_with_a_progress_token_streams_notifications() {
    let mut interp = Interp::new();
    let notes = with_progress_capture(|| {
        round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    json!({
                        "name": "eval",
                        "arguments": { "source": "(mcp/progress 1 3 \"step-one\")" },
                        "_meta": { "progressToken": "tok-42" }
                    }),
                ),
                notif("exit", json!(null)),
            ],
        );
    });
    assert_eq!(notes.len(), 1, "expected one progress notification");
    assert_eq!(notes[0]["method"], "notifications/progress");
    assert_eq!(notes[0]["params"]["progressToken"], "tok-42");
    assert_eq!(notes[0]["params"]["progress"], 1);
    assert_eq!(notes[0]["params"]["total"], 3);
    assert_eq!(notes[0]["params"]["message"], "step-one");
}

#[test]
fn without_a_progress_token_no_notifications_are_sent() {
    let mut interp = Interp::new();
    let notes = with_progress_capture(|| {
        let resp = round_trip(
            &mut interp,
            &[
                req(
                    1,
                    "tools/call",
                    // Same call, no _meta.progressToken.
                    json!({
                        "name": "eval",
                        "arguments": { "source": "(mcp/progress 1 3 \"step-one\")" }
                    }),
                ),
                notif("exit", json!(null)),
            ],
        );
        // The handler still succeeds — mcp/progress is a no-op returning
        // false (the eval tool wraps it as {:value "false"}).
        let text = resp[0]["result"]["content"][0]["text"].as_str().unwrap();
        let body: Json = serde_json::from_str(text).unwrap();
        assert_eq!(body["value"], "false");
    });
    assert!(
        notes.is_empty(),
        "no token → no progress notifications: {notes:?}"
    );
}
