    use super::*;
    use lsp_server::{Request, RequestId};
    use lsp_types::{
        DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
        VersionedTextDocumentIdentifier,
    };
    use std::thread;

    fn uri() -> Uri {
        "file:///t.blsp".parse().unwrap()
    }

    fn note<P: serde::Serialize>(method: &str, params: P) -> Message {
        Message::Notification(ServerNotification::new(method.to_string(), params))
    }

    fn did_open(text: &str) -> Message {
        note(
            DidOpenTextDocument::METHOD,
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri(),
                    language_id: "brood".into(),
                    version: 1,
                    text: text.into(),
                },
            },
        )
    }

    /// Read client messages until the next `publishDiagnostics`, returning its
    /// messages. Panics if the server closes the channel first.
    fn next_diagnostics(client: &Connection) -> Vec<String> {
        loop {
            match client
                .receiver
                .recv()
                .expect("server closed before diagnostics")
            {
                Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                    let p: PublishDiagnosticsParams = serde_json::from_value(n.params).unwrap();
                    return p.diagnostics.into_iter().map(|d| d.message).collect();
                }
                _ => continue,
            }
        }
    }

    /// Send `shutdown` + `exit` so `main_loop` returns and the thread can join.
    fn shutdown(client: &Connection) {
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "shutdown".to_string(),
                serde_json::Value::Null,
            )))
            .unwrap();
        client
            .sender
            .send(note("exit", serde_json::Value::Null))
            .unwrap();
    }

    #[test]
    fn open_then_change_publishes_then_clears_diagnostics() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        client.sender.send(did_open("(foo")).unwrap(); // unclosed list
        let diags = next_diagnostics(&client);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].contains("unclosed delimiter"), "{diags:?}");

        // Edit to well-formed source → diagnostics cleared.
        client
            .sender
            .send(note(
                DidChangeTextDocument::METHOD,
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier {
                        uri: uri(),
                        version: 2,
                    },
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        // A well-formed form with no unbound names — so the
                        // type-check tier produces no warnings either, and the
                        // diagnostics list is genuinely empty.
                        text: "nil".into(),
                    }],
                },
            ))
            .unwrap();
        assert!(next_diagnostics(&client).is_empty());

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }

    /// A ranged `didChange` (`range: Some(..)`, incremental sync) splices ONLY the
    /// given span. Proves the range→byte-offset math lands on the right line: a
    /// wrong offset would corrupt a different line and not yield exactly this error.
    #[test]
    fn incremental_ranged_change_splices_at_the_right_offset() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        // Three clean literal lines → no diagnostics.
        client.sender.send(did_open("nil\nnil\nnil")).unwrap();
        assert!(next_diagnostics(&client).is_empty());

        let ranged = |version, sl, sc, el, ec, text: &str| {
            note(
                DidChangeTextDocument::METHOD,
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier {
                        uri: uri(),
                        version,
                    },
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: Some(lsp_types::Range {
                            start: lsp_types::Position {
                                line: sl,
                                character: sc,
                            },
                            end: lsp_types::Position {
                                line: el,
                                character: ec,
                            },
                        }),
                        range_length: None,
                        text: text.into(),
                    }],
                },
            )
        };

        // Replace ONLY the middle line's `nil` (line 1, chars 0..3) with `(` — an
        // unclosed delimiter. Text becomes "nil\n(\nnil".
        client.sender.send(ranged(2, 1, 0, 1, 3, "(")).unwrap();
        let diags = next_diagnostics(&client);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].contains("unclosed delimiter"), "{diags:?}");

        // Ranged edit back to well-formed: replace line 1 char 0..1 (the `(`) with
        // `nil`. The offset must resolve against the *prior edit's* text.
        client.sender.send(ranged(3, 1, 0, 1, 1, "nil")).unwrap();
        assert!(next_diagnostics(&client).is_empty());

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }

    /// Two edits in ONE `didChange` batch compound: the second resolves its range
    /// against the text the first produced. `[` at (0,0) then ` nil]` at (0,4) —
    /// the second offset (4) only exists once the first grew the line to `[nil`,
    /// so a correct result `[nil nil]` (clean) proves the per-edit index rebuild.
    #[test]
    fn incremental_multi_edit_batch_compounds() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        client.sender.send(did_open("nil")).unwrap();
        assert!(next_diagnostics(&client).is_empty());

        let edit = |sl, sc, text: &str| TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position {
                    line: sl,
                    character: sc,
                },
                end: lsp_types::Position {
                    line: sl,
                    character: sc,
                },
            }),
            range_length: None,
            text: text.into(),
        };
        client
            .sender
            .send(note(
                DidChangeTextDocument::METHOD,
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier {
                        uri: uri(),
                        version: 2,
                    },
                    content_changes: vec![edit(0, 0, "["), edit(0, 4, " nil]")],
                },
            ))
            .unwrap();
        // "[nil nil]" is a clean vector literal — no diagnostics. A stale index on
        // the second edit would misplace `]` and produce a parse error instead.
        assert!(next_diagnostics(&client).is_empty());

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn close_clears_diagnostics() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        client.sender.send(did_open("(")).unwrap();
        assert!(!next_diagnostics(&client).is_empty());

        client
            .sender
            .send(note(
                DidCloseTextDocument::METHOD,
                DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier { uri: uri() },
                },
            ))
            .unwrap();
        assert!(next_diagnostics(&client).is_empty());

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn malformed_notification_does_not_kill_the_server() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        // Bogus params for didOpen: must be logged and ignored, not fatal.
        client
            .sender
            .send(note(
                DidOpenTextDocument::METHOD,
                serde_json::json!({ "bogus": true }),
            ))
            .unwrap();
        // A subsequent valid didOpen still gets served → the server survived.
        client.sender.send(did_open(")")).unwrap();
        assert_eq!(next_diagnostics(&client), vec!["unmatched `)`".to_string()]);

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }

    /// Send a request and read client messages until its `Response` arrives
    /// (skipping any diagnostics the open/change emitted in between).
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
                .expect("server closed before response")
            {
                Message::Response(r) if r.id == RequestId::from(id) => return r,
                _ => continue,
            }
        }
    }

    fn position_params(line: u32, character: u32) -> serde_json::Value {
        serde_json::json!({
            "textDocument": { "uri": uri() },
            "position": { "line": line, "character": character },
        })
    }

    #[test]
    fn serves_tier1_requests_end_to_end() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        // `f` defined, then called; `map` is a prelude global.
        client
            .sender
            .send(did_open("(defn f (x) \"doubles\" (+ x x))\n(f (map g xs))"))
            .unwrap();

        // documentSymbol → one symbol, `f`.
        let r = request(
            &client,
            1,
            DocumentSymbolRequest::METHOD,
            serde_json::json!({ "textDocument": { "uri": uri() } }),
        );
        let syms: Vec<lsp_types::DocumentSymbol> =
            serde_json::from_value(r.result.unwrap()).unwrap();
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "f");

        // hover on the `f` call site (line 1, char 1) → its signature + docstring.
        let r = request(&client, 2, HoverRequest::METHOD, position_params(1, 1));
        let h: lsp_types::Hover = serde_json::from_value(r.result.unwrap()).unwrap();
        let lsp_types::HoverContents::Markup(m) = h.contents else {
            panic!("expected markup");
        };
        assert!(m.value.contains("(f x)"), "{:?}", m.value);
        assert!(m.value.contains("doubles"), "{:?}", m.value);

        // goto-definition on the same `f` → its binder at line 0, char 6.
        let r = request(&client, 3, GotoDefinition::METHOD, position_params(1, 1));
        let loc: lsp_types::Location = serde_json::from_value(r.result.unwrap()).unwrap();
        assert_eq!(loc.range.start, lsp_types::Position::new(0, 6));

        // completion inside the defn body (line 0, at the `x` in `(+ x x)`) →
        // offers the local `x`, the doc def `f`, and the global `map`.
        let r = request(&client, 4, Completion::METHOD, position_params(0, 26));
        let items: Vec<lsp_types::CompletionItem> =
            serde_json::from_value(r.result.unwrap()).unwrap();
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"x"), "local x missing: {labels:?}");
        assert!(labels.contains(&"map"), "global map missing: {labels:?}");

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn serves_new_features_end_to_end() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        // A messy multi-line buffer with a typo'd call to a prelude global.
        client
            .sender
            .send(did_open("(defn f (x)\n  (reduc + x))"))
            .unwrap();

        // formatting → one whole-document edit that canonicalises the source.
        let r = request(
            &client,
            10,
            Formatting::METHOD,
            serde_json::json!({
                "textDocument": { "uri": uri() },
                "options": { "tabSize": 2, "insertSpaces": true },
            }),
        );
        let edits: Vec<lsp_types::TextEdit> = serde_json::from_value(r.result.unwrap()).unwrap();
        assert_eq!(edits.len(), 1, "one whole-document edit");
        assert!(edits[0].new_text.contains("(defn f (x)"), "{:?}", edits[0]);

        // workspace/symbol "f" → finds the top-level def `f`.
        let r = request(
            &client,
            11,
            WorkspaceSymbolRequest::METHOD,
            serde_json::json!({ "query": "f" }),
        );
        let syms: Vec<lsp_types::WorkspaceSymbol> =
            serde_json::from_value(r.result.unwrap()).unwrap();
        assert!(syms.iter().any(|s| s.name == "f"), "got: {syms:?}");

        // foldingRange → the multi-line defn folds (lines 0..1).
        let r = request(
            &client,
            12,
            FoldingRangeRequest::METHOD,
            serde_json::json!({ "textDocument": { "uri": uri() } }),
        );
        let folds: Vec<lsp_types::FoldingRange> =
            serde_json::from_value(r.result.unwrap()).unwrap();
        assert!(folds.iter().any(|f| f.start_line == 0), "got: {folds:?}");

        // inlayHint over the whole doc → labels the `(reduc + x)` args? `reduc`
        // is unbound (typo), so no hint there; but the outer call has none too.
        // Just assert the request succeeds and returns an array.
        let r = request(
            &client,
            13,
            InlayHintRequest::METHOD,
            serde_json::json!({
                "textDocument": { "uri": uri() },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 1, "character": 14 },
                },
            }),
        );
        let _hints: Vec<lsp_types::InlayHint> = serde_json::from_value(r.result.unwrap()).unwrap();

        // codeAction over the `reduc` token, passing the published unbound-symbol
        // diagnostic in context → a "did you mean `reduce`?" quick-fix.
        let r = request(
            &client,
            14,
            CodeActionRequest::METHOD,
            serde_json::json!({
                "textDocument": { "uri": uri() },
                "range": {
                    "start": { "line": 1, "character": 3 },
                    "end": { "line": 1, "character": 8 },
                },
                "context": {
                    "diagnostics": [{
                        "range": {
                            "start": { "line": 1, "character": 3 },
                            "end": { "line": 1, "character": 8 },
                        },
                        "message": "unbound symbol: reduc",
                        "severity": 2,
                        "source": "brood",
                    }],
                },
            }),
        );
        let actions: Vec<lsp_types::CodeActionOrCommand> =
            serde_json::from_value(r.result.unwrap()).unwrap();
        let titles: Vec<String> = actions
            .iter()
            .filter_map(|a| match a {
                lsp_types::CodeActionOrCommand::CodeAction(ca) => Some(ca.title.clone()),
                _ => None,
            })
            .collect();
        assert!(
            titles.iter().any(|t| t.contains("reduce")),
            "expected a 'did you mean reduce' fix, got: {titles:?}"
        );

        // selectionRange at the `+` head → a chain that expands to the enclosing
        // call and beyond (at least two nested levels).
        let r = request(
            &client,
            15,
            SelectionRangeRequest::METHOD,
            serde_json::json!({
                "textDocument": { "uri": uri() },
                "positions": [{ "line": 1, "character": 9 }],
            }),
        );
        let sel: Vec<lsp_types::SelectionRange> =
            serde_json::from_value(r.result.unwrap()).unwrap();
        assert_eq!(sel.len(), 1, "one chain per position");
        assert!(
            sel[0].parent.is_some(),
            "selection should expand to an enclosing form"
        );

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn unknown_request_gets_method_not_found() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(7),
                "textDocument/onTypeFormatting".to_string(), // not advertised
                serde_json::json!({}),
            )))
            .unwrap();
        match client.receiver.recv().unwrap() {
            Message::Response(r) => {
                assert_eq!(r.id, RequestId::from(7));
                let err = r.error.expect("an error response");
                assert_eq!(err.code, lsp_server::ErrorCode::MethodNotFound as i32);
            }
            other => panic!("expected an error Response, got {other:?}"),
        }

        shutdown(&client);
        handle.join().unwrap().unwrap();
    }
