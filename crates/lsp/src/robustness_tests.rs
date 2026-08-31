//! Robustness regressions for the server message loop — the failures that are
//! *silent* or *fatal* rather than merely wrong, driven end-to-end over an
//! in-process `Connection::memory()` pair like [`server_tests`](crate::server_tests).
//!
//! Each case here reproduced a real defect found by driving the shipped binary
//! with hostile JSON-RPC (see `docs/lsp.md`); every one fails without its fix.

use super::*;
use lsp_server::{Request, RequestId};
use lsp_types::{DidOpenTextDocumentParams, Position, TextDocumentItem, TextEdit, WorkspaceEdit};
use std::thread;

fn uri_for(name: &str) -> Uri {
    format!("file:///t_{name}.blsp").parse().unwrap()
}

fn note<P: serde::Serialize>(method: &str, params: P) -> Message {
    Message::Notification(ServerNotification::new(method.to_string(), params))
}

fn open(u: &Uri, text: &str) -> Message {
    note(
        DidOpenTextDocument::METHOD,
        DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: u.clone(),
                language_id: "brood".into(),
                version: 1,
                text: text.into(),
            },
        },
    )
}

fn shutdown(client: &Connection) {
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(9999),
            "shutdown".to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();
    client
        .sender
        .send(note("exit", serde_json::Value::Null))
        .unwrap();
}

fn request(client: &Connection, id: i32, method: &str, params: serde_json::Value) -> Response {
    client
        .sender
        .send(Message::Request(Request::new(
            RequestId::from(id),
            method.to_string(),
            params,
        )))
        .unwrap();
    loop {
        match client
            .receiver
            .recv()
            .expect("server closed before responding")
        {
            Message::Response(r) if r.id == RequestId::from(id) => return r,
            _ => continue,
        }
    }
}

fn diagnostics(client: &Connection) -> Vec<Diagnostic> {
    loop {
        match client.receiver.recv().expect("server closed") {
            Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                let p: PublishDiagnosticsParams = serde_json::from_value(n.params).unwrap();
                return p.diagnostics;
            }
            _ => continue,
        }
    }
}

/// **The server must survive a request that panics.** Before containment, an
/// unwind out of a handler propagated through `main_loop` and killed the
/// process — over stdio that is the editor's entire language support gone, with
/// no response to the request that triggered it.
///
/// The trigger used here is the real one that was found: `foldingRange` over an
/// unclosed container whose span therefore runs to EOF, on a buffer ending in a
/// multibyte character (`(é` — mid-`é` slice). `LineIndex::position` is total
/// now, so this no longer panics at all; the case stays as the end-to-end proof
/// that the request is *answered*, whichever way the fix holds.
#[test]
fn a_half_typed_multibyte_buffer_does_not_kill_the_server() {
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || main_loop(&server, ClientCaps::default()));

    for (i, text) in ["(é", "(😀", "[中", "(defn f (x)\n  (g é"]
        .iter()
        .enumerate()
    {
        let u = uri_for(&format!("mb{i}"));
        client.sender.send(open(&u, text)).unwrap();
        let _ = diagnostics(&client);
        let r = request(
            &client,
            100 + i as i32,
            FoldingRangeRequest::METHOD,
            serde_json::json!({ "textDocument": { "uri": u } }),
        );
        assert!(
            r.response_result.is_ok(),
            "folding on {text:?} errored: {r:?}"
        );
    }

    // Still serving afterwards.
    let u = uri_for("mb0");
    let r = request(
        &client,
        200,
        DocumentSymbolRequest::METHOD,
        serde_json::json!({ "textDocument": { "uri": u } }),
    );
    assert!(r.response_result.is_ok());

    shutdown(&client);
    handle.join().unwrap().unwrap();
}

/// **A checker finding must be located in UTF-16, not in characters.** The
/// checker reports a reader `Pos` whose column counts *characters*; publishing
/// that as `Position.character` drifts by one per astral char earlier on the
/// line. The squiggle then covers the wrong text — and, worse, `codeAction`
/// anchors its quick-fix edit to that range, so accepting "did you mean?"
/// rewrote a span in the *previous* form and left the typo in place.
#[test]
fn a_diagnostic_after_an_emoji_lands_on_the_offending_token() {
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || main_loop(&server, ClientCaps::default()));

    // `(def s "😀😀") ` is 14 characters but 16 UTF-16 code units.
    let text = "(def s \"😀😀\") (reduc + 1)\n";
    let u = uri_for("emoji");
    client.sender.send(open(&u, text)).unwrap();
    let diags = diagnostics(&client);
    let d = diags
        .iter()
        .find(|d| d.message.contains("unbound symbol: reduc"))
        .unwrap_or_else(|| panic!("expected an unbound-symbol finding, got {diags:?}"));

    // The token `reduc` occupies UTF-16 columns 16..21 on line 0.
    assert_eq!(d.range.start, Position::new(0, 16), "start of `reduc`");
    assert_eq!(d.range.end, Position::new(0, 21), "end of `reduc`");

    // And the quick-fix built from it edits exactly that span — applying it must
    // produce `(reduce + 1)`, not splice into the string literal before it.
    let r = request(
        &client,
        1,
        CodeActionRequest::METHOD,
        serde_json::json!({
            "textDocument": { "uri": u },
            "range": d.range,
            "context": { "diagnostics": [d] },
        }),
    );
    let actions: Vec<lsp_types::CodeActionOrCommand> =
        serde_json::from_value(r.response_result.expect("code actions")).unwrap();
    let fix = actions
        .iter()
        .find_map(|a| match a {
            lsp_types::CodeActionOrCommand::CodeAction(ca) if ca.title.contains("reduce") => {
                Some(ca)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a did-you-mean fix, got {actions:?}"));
    let edits = fix.edit.as_ref().unwrap().changes.as_ref().unwrap();
    let edit = &edits[&u][0];
    assert_eq!(edit.range, d.range);
    assert_eq!(edit.new_text, "reduce");

    shutdown(&client);
    handle.join().unwrap().unwrap();
}

/// **A rename must not hand the client overlapping or duplicate edits.** The
/// spec leaves overlapping edits for one document undefined and clients differ
/// (duplicate the text, drop one, reject all); the cross-file rename unions
/// several independent scans, so this is sanitized centrally rather than trusted.
#[test]
fn overlapping_and_duplicate_edits_are_dropped() {
    let r = |sl, sc, el, ec, t: &str| TextEdit {
        range: Range::new(Position::new(sl, sc), Position::new(el, ec)),
        new_text: t.to_string(),
    };
    let kept = non_overlapping(vec![
        r(0, 10, 0, 15, "b"), // out of order on purpose
        r(0, 0, 0, 5, "a"),
        r(0, 0, 0, 5, "a"),   // exact duplicate
        r(0, 3, 0, 8, "c"),   // overlaps the first kept edit
        r(0, 15, 0, 20, "d"), // touches end-to-start — legal, kept
        r(1, 5, 1, 1, "e"),   // inverted — never emitted
    ]);
    let texts: Vec<&str> = kept.iter().map(|e| e.new_text.as_str()).collect();
    assert_eq!(texts, vec!["a", "b", "d"], "kept: {kept:?}");
}

/// **A rename must carry the document version when the client can use it.** A
/// bare `changes` map is applied blind, so a rename computed against version N
/// lands on a buffer the user has since edited. With `documentChanges` the
/// client rejects the stale edit instead.
#[test]
fn rename_is_version_stamped_for_a_capable_client() {
    let (server, client) = Connection::memory();
    let caps = ClientCaps {
        snippet: false,
        document_changes: true,
    };
    let handle = thread::spawn(move || main_loop(&server, caps));

    let u = uri_for("ver");
    client
        .sender
        .send(open(&u, "(defn f (x) x)\n(f 1)\n"))
        .unwrap();
    let _ = diagnostics(&client);

    let r = request(
        &client,
        1,
        Rename::METHOD,
        serde_json::json!({
            "textDocument": { "uri": u },
            "position": { "line": 0, "character": 6 },
            "newName": "g",
        }),
    );
    let edit: WorkspaceEdit = serde_json::from_value(r.response_result.expect("a rename")).unwrap();
    assert!(
        edit.changes.is_none(),
        "a documentChanges-capable client must not get the unversioned map"
    );
    let Some(DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected versioned documentChanges, got {edit:?}");
    };
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0].text_document.version,
        Some(1),
        "the open document's version must be stamped on its edits"
    );
    assert!(edits[0].edits.len() >= 2, "def + call site");

    shutdown(&client);
    handle.join().unwrap().unwrap();
}

/// A client that declared nothing still gets the plain `changes` map — we must
/// not send a shape it can't read.
#[test]
fn rename_stays_unversioned_for_a_plain_client() {
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || main_loop(&server, ClientCaps::default()));

    let u = uri_for("plain");
    client
        .sender
        .send(open(&u, "(defn f (x) x)\n(f 1)\n"))
        .unwrap();
    let _ = diagnostics(&client);
    let r = request(
        &client,
        1,
        Rename::METHOD,
        serde_json::json!({
            "textDocument": { "uri": u },
            "position": { "line": 0, "character": 6 },
            "newName": "g",
        }),
    );
    let edit: WorkspaceEdit = serde_json::from_value(r.response_result.expect("a rename")).unwrap();
    assert!(edit.document_changes.is_none());
    assert!(edit.changes.is_some());

    shutdown(&client);
    handle.join().unwrap().unwrap();
}

/// **An inverted `didChange` range must still move the mirror.** Dropping the
/// edit (the old behaviour) leaves the server's copy silently out of step with
/// the editor's, and every position computed afterwards is against text the user
/// isn't looking at.
#[test]
fn an_inverted_change_range_still_applies() {
    let mut text = String::from("(defn f (x) x)");
    apply_content_change(
        &mut text,
        &lsp_types::TextDocumentContentChangeEvent {
            // end (char 1) before start (char 14) — the reversed span 1..14.
            range: Some(Range::new(Position::new(0, 14), Position::new(0, 1))),
            range_length: None,
            text: "ok".into(),
        },
    );
    assert_eq!(text, "(ok");
}

/// **`exit` with no preceding `shutdown` is an abnormal end.** The loop must
/// report it so `main` can exit nonzero — reporting success for both makes a
/// forced stop indistinguishable from an orderly one to a supervising editor.
#[test]
fn a_bare_exit_is_reported_as_an_abnormal_end() {
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || main_loop(&server, ClientCaps::default()));
    client
        .sender
        .send(note("exit", serde_json::Value::Null))
        .unwrap();
    assert!(
        !handle.join().unwrap().unwrap(),
        "a bare exit must not report a clean shutdown"
    );

    // …and the shutdown/exit pair still does report clean.
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || main_loop(&server, ClientCaps::default()));
    shutdown(&client);
    assert!(handle.join().unwrap().unwrap());
}

/// Hostile-but-legal traffic: unknown methods, missing/misshaped params, and a
/// position far past EOF must each get an answer and leave the server serving.
#[test]
fn hostile_requests_are_answered_and_not_fatal() {
    let (server, client) = Connection::memory();
    let handle = thread::spawn(move || main_loop(&server, ClientCaps::default()));

    let u = uri_for("hostile");
    client.sender.send(open(&u, "(defn f (x) x)\n")).unwrap();
    let _ = diagnostics(&client);

    // Unknown method → MethodNotFound, not silence.
    let r = request(&client, 1, "textDocument/nonsense", serde_json::json!({}));
    assert_eq!(
        r.response_result.expect_err("an error").code,
        ErrorCode::MethodNotFound as i32
    );

    // Misshaped params → InvalidParams, not a panic.
    let r = request(&client, 2, HoverRequest::METHOD, serde_json::json!({}));
    assert_eq!(
        r.response_result.expect_err("an error").code,
        ErrorCode::InvalidParams as i32
    );

    // A position past the end of the document → a null result.
    let r = request(
        &client,
        3,
        HoverRequest::METHOD,
        serde_json::json!({
            "textDocument": { "uri": u },
            "position": { "line": 4294967295u32, "character": 4294967295u32 },
        }),
    );
    assert!(r.response_result.is_ok(), "{r:?}");

    // A request for a document we never opened → a result, never a hang.
    let r = request(
        &client,
        4,
        DocumentSymbolRequest::METHOD,
        serde_json::json!({ "textDocument": { "uri": uri_for("never") } }),
    );
    assert!(r.response_result.is_ok(), "{r:?}");

    shutdown(&client);
    handle.join().unwrap().unwrap();
}
