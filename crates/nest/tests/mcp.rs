//! End-to-end tests for `nest mcp`: the real binary, in a temp project, driven over stdio
//! the way an MCP client drives it — newline-delimited JSON-RPC in, one line per reply out.
//!
//! The dispatcher's own branches (error mapping, deadlines, progress, the JSON projection)
//! are covered in-image by `tests/mcp_test.blsp`; these pin what only the process boundary
//! shows: the transport framing, that nothing but JSON reaches stdout, the embedded
//! documents, the project bootstrap, and a clean exit.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value as Json, json};

mod support;
use support::{TempDir, temp_dir};

/// A one-module project, plus `extra` files (`(relative path, contents)`).
fn project(tag: &str, extra: &[(&str, &str)]) -> TempDir {
    let directory = temp_dir(&format!("mcp-{tag}"));
    let root = &directory.path;
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"mcpdemo\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.blsp"),
        "(defmodule main \"d\")\n\n(defn main () nil)\n",
    )
    .unwrap();
    for (relative, contents) in extra {
        std::fs::write(root.join(relative), contents).unwrap();
    }
    directory
}

struct Session {
    replies: Vec<Json>,
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

/// Run `nest mcp` in `dir`, feed it `input` (raw lines) and close stdin; every stdout line
/// must be JSON, which is the channel-purity assertion every case gets for free.
fn serve_raw(dir: &Path, input: &str) -> Session {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("mcp")
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn nest mcp");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("wait for nest mcp");
    let stdout = String::from_utf8_lossy_owned(output.stdout);
    let replies = stdout
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("non-JSON on the channel ({e}): {line:?}"))
        })
        .collect();
    Session {
        replies,
        stdout,
        stderr: String::from_utf8_lossy_owned(output.stderr),
        code: output.status.code(),
    }
}

fn serve(dir: &Path, messages: &[Json]) -> Session {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    serve_raw(dir, &input)
}

fn request(id: i64, method: &str, params: Json) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn notification(method: &str) -> Json {
    json!({ "jsonrpc": "2.0", "method": method })
}

fn call(id: i64, tool: &str, arguments: Json) -> Json {
    request(
        id,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
}

/// The reply carrying `id`.
fn reply(session: &Session, id: i64) -> &Json {
    session
        .replies
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no reply for id {id}: {:?}", session.replies))
}

/// A tool reply's first content block, parsed back from its JSON text.
fn tool_body(session: &Session, id: i64) -> Json {
    let text = reply(session, id)["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content for id {id}: {}", reply(session, id)));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("payload is not JSON ({e}): {text}"))
}

#[test]
fn a_session_over_stdio_speaks_newline_delimited_json_rpc() {
    let directory = project("session", &[]);
    let lines = [
        request(1, "initialize", json!({})).to_string(),
        notification("notifications/initialized").to_string(),
        String::new(),
        // A leftover LSP framing header is not a message: answered -32700, not obeyed.
        "Content-Length: 17\r".to_string(),
        "this is not json{{{".to_string(),
        request(2, "ping", json!({})).to_string(),
        request(3, "no/such/method", json!({})).to_string(),
        request(4, "shutdown", json!({})).to_string(),
        notification("exit").to_string(),
        request(5, "ping", json!({})).to_string(),
    ];
    let session = serve_raw(&directory.path, &(lines.join("\n") + "\n"));
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);

    let initialize = &reply(&session, 1)["result"];
    assert_eq!(initialize["serverInfo"]["name"], "nest-mcp");
    assert_eq!(initialize["protocolVersion"], "2024-11-05");
    for capability in ["tools", "resources", "prompts"] {
        assert!(initialize["capabilities"][capability].is_object());
    }

    let parse_errors: Vec<&Json> = session
        .replies
        .iter()
        .filter(|r| r["error"]["code"] == -32700)
        .collect();
    assert_eq!(parse_errors.len(), 2, "{:?}", session.replies);
    for error in parse_errors {
        assert_eq!(error["id"], Json::Null);
        assert!(error["error"]["data"].is_string());
    }
    assert_eq!(reply(&session, 2)["result"], json!({}));
    assert_eq!(reply(&session, 3)["error"]["code"], -32601);
    assert_eq!(reply(&session, 4)["result"], Json::Null);
    // initialize, two parse errors, ping, method-not-found, shutdown — nothing for the
    // notifications, and nothing after `exit`.
    assert_eq!(session.replies.len(), 6, "{:?}", session.replies);
    assert!(session.stdout.ends_with('\n'));
}

#[test]
fn end_of_input_ends_the_session_cleanly() {
    let directory = project("eof", &[]);
    let session = serve(&directory.path, &[request(1, "ping", json!({}))]);
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);
    assert_eq!(session.replies.len(), 1);
}

#[test]
fn outside_a_project_the_server_refuses_to_start() {
    let directory = temp_dir("mcp-noproject");
    let session = serve(&directory.path, &[request(1, "ping", json!({}))]);
    assert_eq!(session.code, Some(2));
    assert!(session.replies.is_empty());
    assert!(
        session.stderr.contains("nest mcp: no project.blsp"),
        "{}",
        session.stderr
    );
}

#[test]
fn tools_evaluate_in_one_live_image_and_keep_the_channel_pure() {
    let directory = project("eval", &[]);
    let session = serve(
        &directory.path,
        &[
            request(1, "tools/list", json!({})),
            call(2, "eval", json!({ "source": "(def mcp-test-x 7)" })),
            call(3, "eval", json!({ "source": "(* mcp-test-x 6)" })),
            call(
                4,
                "eval",
                json!({ "source": "(do (io/puts \"debug line\") 1)" }),
            ),
            call(
                5,
                "eval",
                json!({ "source": "(term/draw [[:clear] [:text 0 0 \"ab\"]])" }),
            ),
            call(6, "eval", json!({ "source": "(no-such-fn 1)" })),
            call(7, "eval", json!({ "source": 42 })),
            call(8, "nope", json!({})),
            request(
                9,
                "tools/call",
                json!({ "name": "eval",
                        "arguments": { "source": "(mcp/progress 1 3 \"step-one\")" },
                        "_meta": { "progressToken": "tok-42" } }),
            ),
            call(10, "lookup", json!({ "name": "map" })),
        ],
    );
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);

    let tools = reply(&session, 1)["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "eval",
        "load",
        "write",
        "edit",
        "lookup",
        "check",
        "run-tests",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }

    // The hot-reload promise: a `def` in one call is visible to the next.
    assert_eq!(tool_body(&session, 3)["value"], "42");
    // A silent handler adds no block; a printing one gets a labelled second block.
    assert_eq!(
        reply(&session, 3)["result"]["content"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let printed = reply(&session, 4)["result"]["content"][1]["text"]
        .as_str()
        .unwrap();
    assert_eq!(printed, "[captured stdout]\ndebug line\n");
    // Terminal escapes are captured too, rather than written over the JSON channel.
    let drawn = reply(&session, 5)["result"]["content"].to_string();
    assert!(drawn.contains("[2J"), "{drawn}");

    let error = &tool_body(&session, 6)["error"];
    assert_eq!(error["kind"], "unbound");
    assert_eq!(error["code"], "E0010");

    let thrown = &reply(&session, 7)["error"];
    assert_eq!(thrown["code"], -32603);
    assert!(thrown["message"].as_str().unwrap().contains(":source"));
    assert_eq!(thrown["data"]["kind"], "user");

    assert_eq!(reply(&session, 8)["error"]["code"], -32602);

    // The progress notification precedes the reply to the call that reported it.
    let notification_at = session
        .replies
        .iter()
        .position(|r| r["method"] == "notifications/progress")
        .expect("a progress notification");
    let reply_at = session.replies.iter().position(|r| r["id"] == 9).unwrap();
    assert!(notification_at < reply_at);
    let progress = &session.replies[notification_at]["params"];
    assert_eq!(
        progress,
        &json!({ "progressToken": "tok-42", "progress": 1, "total": 3, "message": "step-one" })
    );
    assert_eq!(tool_body(&session, 9)["value"], "true");

    let lookup = tool_body(&session, 10);
    assert_eq!(lookup["name"], "map");
    assert!(!lookup["arglist"].as_array().unwrap().is_empty());
}

#[test]
fn the_embedded_documents_are_served_as_resources_and_the_prompt() {
    let directory = project("documents", &[]);
    let session = serve(
        &directory.path,
        &[
            request(1, "resources/list", json!({})),
            request(2, "resources/read", json!({ "uri": "brood://prelude" })),
            request(
                3,
                "resources/read",
                json!({ "uri": "brood://docs/error-codes" }),
            ),
            request(4, "resources/read", json!({ "uri": "brood://nope" })),
            request(5, "prompts/list", json!({})),
            request(6, "prompts/get", json!({ "name": "brood-task" })),
            request(7, "prompts/get", json!({ "name": "nope" })),
        ],
    );
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);
    let uris: Vec<&str> = reply(&session, 1)["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    for expected in [
        "brood://docs/brood-for-claude",
        "brood://docs/error-codes",
        "brood://prelude",
    ] {
        assert!(uris.contains(&expected), "missing {expected}: {uris:?}");
    }
    // The six reference docs and nothing else: notes about developing Brood itself are not
    // served to a project's agent.
    assert_eq!(uris.len(), 6, "{uris:?}");
    let prelude = &reply(&session, 2)["result"]["contents"][0];
    assert_eq!(prelude["uri"], "brood://prelude");
    assert_eq!(prelude["text"].as_str().unwrap(), brood::boot::PRELUDE);
    let error_codes = reply(&session, 3)["result"]["contents"][0]["text"]
        .as_str()
        .unwrap();
    assert!(error_codes.contains("E0010"));
    assert_eq!(reply(&session, 4)["error"]["code"], -32602);

    let prompts = reply(&session, 5)["result"]["prompts"].as_array().unwrap();
    assert_eq!(prompts[0]["name"], "brood-task");
    let text = reply(&session, 6)["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("brood://docs/brood-for-claude"), "{text}");
    assert!(text.contains("immutable"), "{text}");
    assert!(text.contains("MCP tools"), "{text}");
    assert_eq!(reply(&session, 7)["error"]["code"], -32602);
}

#[test]
fn the_project_tools_act_on_the_served_project() {
    // One deliberate warning, so `check` proves WHICH project it checked.
    let directory = project(
        "project",
        &[(
            "src/oops.blsp",
            "(defmodule oops \"d\")\n\n(defn go () (definitely-not-a-real-global 1))\n",
        )],
    );
    let root = directory.path.canonicalize().unwrap();
    let session = serve(
        &directory.path,
        &[
            call(
                1,
                "write",
                json!({ "path": "src/gen.blsp", "content": "(defn gen-answer () 42)" }),
            ),
            call(2, "eval", json!({ "source": "(gen-answer)" })),
            call(
                3,
                "write",
                json!({ "path": "../escape.blsp", "content": "nope" }),
            ),
            call(
                4,
                "write",
                json!({ "path": "/etc/passwd", "content": "nope" }),
            ),
            call(
                5,
                "write",
                json!({ "path": "notes.txt", "content": "alpha beta beta gamma" }),
            ),
            call(
                6,
                "edit",
                json!({ "path": "notes.txt", "old": "beta", "new": "X" }),
            ),
            call(
                7,
                "edit",
                json!({ "path": "notes.txt", "old": "alpha", "new": "ALPHA" }),
            ),
            call(8, "check", json!({})),
            call(9, "processes", json!({})),
            call(10, "node", json!({})),
        ],
    );
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);

    assert_eq!(tool_body(&session, 1)["ok"], true);
    assert_eq!(
        std::fs::read_to_string(root.join("src/gen.blsp")).unwrap(),
        "(defn gen-answer () 42)"
    );
    assert_eq!(tool_body(&session, 2)["value"], "42");
    assert_eq!(tool_body(&session, 3)["ok"], false);
    assert_eq!(tool_body(&session, 4)["ok"], false);
    assert!(!root.parent().unwrap().join("escape.blsp").exists());
    assert_eq!(tool_body(&session, 6)["ok"], false, "an ambiguous :old");
    assert_eq!(tool_body(&session, 7)["ok"], true);
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "ALPHA beta beta gamma"
    );

    let diagnostics = tool_body(&session, 8)["diagnostics"].clone();
    let diagnostics = diagnostics.as_array().expect("a :diagnostics array");
    let planted = diagnostics
        .iter()
        .find(|d| {
            d["message"]
                .as_str()
                .is_some_and(|m| m.contains("definitely-not-a-real-global"))
        })
        .unwrap_or_else(|| panic!("the planted warning is missing: {diagnostics:?}"));
    assert!(
        planted["file"]
            .as_str()
            .is_some_and(|f| f.ends_with("oops.blsp"))
    );

    let processes = tool_body(&session, 9)["processes"].clone();
    let processes = processes.as_array().unwrap();
    assert!(!processes.is_empty());
    for process in processes {
        assert!(process["id"].is_number(), "{process}");
        assert_eq!(process["pid"]["$type"], "pid", "{process}");
    }
    let node = tool_body(&session, 10);
    assert!(node["workers"].is_number(), "{node}");
    assert!(node["peers"].is_array(), "{node}");
}

#[test]
fn a_project_mcp_blsp_extends_the_catalogue() {
    let directory = project(
        "extension",
        &[(
            "mcp.blsp",
            "(io/puts \"noise a load must not put on the channel\")\n\
             (defn mcp-tools ()\n  (cons {:name \"echo\" :schema {:type \"object\"} \
             :handler (fn (args) (get args :msg))}\n    (mcp/tools)))\n",
        )],
    );
    let session = serve(
        &directory.path,
        &[
            request(1, "tools/list", json!({})),
            call(2, "echo", json!({ "msg": "hello" })),
        ],
    );
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);
    let names: Vec<&str> = reply(&session, 1)["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"echo") && names.contains(&"eval"),
        "{names:?}"
    );
    assert_eq!(reply(&session, 2)["result"]["content"][0]["text"], "hello");
}

#[test]
fn a_thrown_value_reports_where_the_handler_raised_it() {
    let directory = project(
        "thrown-position",
        &[(
            "mcp.blsp",
            "(defn reject (x)\n  (+ 1 (throw (str \"rejected: \" x))))\n\n\
             (defn mcp-tools ()\n  (cons {:name \"strict\" :schema {:type \"object\"} \
             :handler (fn (args) (+ 1 (reject (get args :msg))))}\n    (mcp/tools)))\n",
        )],
    );
    let session = serve(
        &directory.path,
        &[call(1, "strict", json!({ "msg": "nope" }))],
    );
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);
    let error = &reply(&session, 1)["error"];
    assert_eq!(error["code"], -32603);
    let data = &error["data"];
    assert_eq!(data["kind"], "user", "{data}");
    assert_eq!(data["message"], "rejected: nope", "{data}");
    assert!(
        data["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("mcp.blsp")),
        "{data}"
    );
    assert_eq!(data["line"], 2, "{data}");
    assert!(data["col"].is_number(), "{data}");
    let through_reject = data["trace"].as_array().is_some_and(|frames| {
        frames.iter().any(|frame| {
            frame["fn"]
                .as_str()
                .is_some_and(|name| name.ends_with("reject"))
        })
    });
    assert!(through_reject, "{data}");
}

#[test]
#[cfg(debug_assertions)]
fn a_panicking_handler_is_an_error_reply_and_the_session_survives() {
    let directory = project("panic", &[]);
    let session = serve(
        &directory.path,
        &[
            call(
                1,
                "eval",
                json!({ "source": "(%force-panic \"stunt panic\")" }),
            ),
            call(2, "eval", json!({ "source": "(+ 3 4)" })),
        ],
    );
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);
    let error = &reply(&session, 1)["error"];
    assert_eq!(error["code"], -32603);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("panic in tool handler")
    );
    assert_eq!(error["data"]["kind"], "panic");
    assert!(session.stderr.contains("stunt panic"), "{}", session.stderr);
    assert_eq!(tool_body(&session, 2)["value"], "7");
}

#[test]
fn bignum_churn_through_many_calls_leaves_the_heap_sound() {
    // Many wide-bignum evals on one long-lived image, with the masks captured in closure
    // environments — the Life-demo workload that once corrupted the heap.
    let directory = project("bignum", &[]);
    let setup = "(defn wstep (b w h mask board col0 high)
        (let (wm1 (- w 1) hm1w (* (- h 1) w)
              l (bit/or (bit/and (bit/shift-left b 1) (bit/xor col0 board)) (bit/shift-right (bit/and b high) wm1))
              r (bit/or (bit/and (bit/shift-right b 1) (bit/xor high board)) (bit/shift-left (bit/and b col0) wm1))
              up (fn (f) (bit/or (bit/and (bit/shift-left f w) board) (bit/shift-right f hm1w)))
              dn (fn (f) (bit/or (bit/shift-right f w) (bit/shift-left (bit/and f mask) hm1w))))
          (reduce [(up l) (up b) (up r) l r (dn l) (dn b) (dn r)] 0 (fn (acc m) (bit/xor acc m)))))";
    let churn = "(let (w 200 h 120 mask (- (bit/shift-left 1 w) 1)
                      board (- (bit/shift-left 1 (* w h)) 1)
                      col0 (math/quot board mask) high (bit/shift-left col0 (- w 1))
                      st (bit/and board (bit/shift-left (- (bit/shift-left 1 100) 1) 5000)))
                  (bit/count (reduce (range 30) st (fn (b _) (wstep b w h mask board col0 high)))))";
    let mut messages = vec![call(1, "eval", json!({ "source": setup }))];
    for id in 2..20 {
        messages.push(call(id, "eval", json!({ "source": churn })));
    }
    let session = serve(&directory.path, &messages);
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);
    assert_eq!(session.replies.len(), 19);
    for id in 2..20 {
        let body = tool_body(&session, id);
        assert!(body["value"].is_string(), "call {id}: {body}");
        assert!(body.get("error").is_none(), "call {id}: {body}");
    }
}

/// The orientation an agent gets — the `brood-task` prompt and the `writing-brood` skill
/// `nest new` drops into every project — names every tool the server lists. Both are prose
/// written by hand, so a tool added or renamed without them is invisible to the agent (they
/// once named nine of twenty-four, and called `eval` `reflect/eval`).
#[test]
fn the_prompt_and_the_skill_name_every_listed_tool() {
    let directory = project("tool-docs", &[]);
    let session = serve(
        &directory.path,
        &[
            request(1, "tools/list", json!({})),
            request(2, "prompts/get", json!({ "name": "brood-task" })),
        ],
    );
    assert_eq!(session.code, Some(0), "stderr: {}", session.stderr);
    let names: Vec<String> = reply(&session, 1)["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.len() > 20, "{names:?}");
    let prompt = reply(&session, 2)["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .to_string();
    let skill = include_str!("../../../docs/writing-brood-skill.md");
    for (document, text) in [("brood-task prompt", prompt.as_str()), ("skill", skill)] {
        for name in &names {
            assert!(
                text.contains(&format!("`{name}`")),
                "the {document} does not name the `{name}` tool"
            );
        }
        assert!(
            !text.contains("`reflect/eval`") && !text.contains("`reflect/load`"),
            "the {document} names a tool by a function's name"
        );
    }
}
