//! `brood-lsp` — the Brood language server. A separate binary that speaks LSP
//! over stdio so any editor gets Brood's language knowledge without
//! re-implementing it. See `docs/lsp.md` for the design and ADR-025.
//!
//! Tier 0 (this file): lifecycle, full-document sync, and **syntactic
//! diagnostics** read off the tooling CST ([`brood::syntax::cst`]). The server
//! never evaluates document text — diagnostics come from parsing, not running
//! (a half-typed buffer must stay safe and can't be run). It uses the
//! synchronous `lsp-server` stack (no async runtime): a single blocking request
//! loop owns the document store, sidestepping the `!Sync` `Interp`/`Heap`.

// `lsp_types::Uri` trips clippy's `mutable_key_type` lint (it wraps a
// `fluent_uri` type clippy can't prove is immutable), but it's an interned,
// effectively-immutable URI — the canonical document-store key. False positive.
#![allow(clippy::mutable_key_type)]

use std::collections::HashMap;
use std::error::Error;

use lsp_server::{Connection, Message, Notification as ServerNotification, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as NotificationTrait, PublishDiagnostics,
};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, PositionEncodingKind, PublishDiagnosticsParams, Range,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use brood::syntax::cst;

mod diagnostics;
mod line_index;

use line_index::LineIndex;

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    // stdio transport: the editor launches us and talks JSON-RPC over the pipe.
    let (connection, io_threads) = Connection::stdio();

    let capabilities = ServerCapabilities {
        // Full-document sync: re-parse the whole buffer on each change. The
        // reader/CST is fast enough that incremental sync is premature (ADR-011
        // — ship the simple shape until a need justifies the complex one).
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        // We do UTF-16 column arithmetic in `LineIndex`; advertise it explicitly
        // rather than relying on the protocol default.
        position_encoding: Some(PositionEncodingKind::UTF16),
        ..Default::default()
    };

    // The initialize/initialized handshake. We don't read the client's params
    // yet (no capability negotiation beyond the above).
    let _init = connection.initialize(serde_json::to_value(capabilities)?)?;
    // Run the loop, then drop `connection` *before* the join: its `Sender` keeps
    // the stdout writer thread alive, so the thread only sees its channel close
    // (and exits, letting `io_threads.join()` return) once this drop happens.
    // Skipping the drop would deadlock the join.
    main_loop(&connection)?;
    drop(connection);

    io_threads.join()?;
    Ok(())
}

/// Per-open-document state: just the source text. The CST and `LineIndex` are
/// cheap to rebuild, so we derive them on each change rather than cache them.
type Documents = HashMap<Uri, String>;

fn main_loop(connection: &Connection) -> Result<(), Box<dyn Error + Sync + Send>> {
    let mut docs: Documents = HashMap::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                // The only request Tier 0 serves is shutdown; `handle_shutdown`
                // performs the shutdown/exit handshake and returns true when it
                // was that request, at which point we stop.
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                // Nothing else is advertised, so reply method-not-found rather
                // than leave the client waiting on a response.
                let resp = Response::new_err(
                    req.id,
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("unsupported request: {}", req.method),
                );
                connection.sender.send(Message::Response(resp))?;
            }
            Message::Response(_) => {} // we issue no server→client requests yet
            Message::Notification(not) => {
                handle_notification(connection, &mut docs, not)?;
            }
        }
    }
    Ok(())
}

fn handle_notification(
    connection: &Connection,
    docs: &mut Documents,
    not: ServerNotification,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    // Bad params must not tear down the connection: a malformed (or
    // unexpectedly-shaped) notification is logged and dropped, never fatal.
    // Only `send` failures below propagate — those mean the client is gone.
    match not.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let Some(p) = params::<lsp_types::DidOpenTextDocumentParams>(not) else {
                return Ok(());
            };
            let uri = p.text_document.uri;
            docs.insert(uri.clone(), p.text_document.text);
            publish(connection, docs, &uri)?;
        }
        DidChangeTextDocument::METHOD => {
            let Some(p) = params::<lsp_types::DidChangeTextDocumentParams>(not) else {
                return Ok(());
            };
            // Full sync: the last change event carries the entire new document.
            if let Some(change) = p.content_changes.into_iter().last() {
                let uri = p.text_document.uri;
                docs.insert(uri.clone(), change.text);
                publish(connection, docs, &uri)?;
            }
        }
        DidCloseTextDocument::METHOD => {
            let Some(p) = params::<lsp_types::DidCloseTextDocumentParams>(not) else {
                return Ok(());
            };
            let uri = p.text_document.uri;
            docs.remove(&uri);
            // Clear diagnostics for the closed document.
            send_diagnostics(connection, &uri, Vec::new())?;
        }
        _ => {} // initialized, didSave, didChangeConfiguration, … — nothing to do yet
    }
    Ok(())
}

/// Deserialize a notification's params, logging and dropping it on failure.
/// The method has already been matched, so the only error is a params-shape
/// mismatch — which we tolerate rather than let kill the server.
fn params<P: serde::de::DeserializeOwned>(not: ServerNotification) -> Option<P> {
    let method = not.method.clone();
    match not.extract::<P>(&method) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("brood-lsp: ignoring malformed `{method}`: {e:?}");
            None
        }
    }
}

/// Parse the document, turn its `Error` nodes into LSP diagnostics, and publish.
fn publish(
    connection: &Connection,
    docs: &Documents,
    uri: &Uri,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let Some(text) = docs.get(uri) else {
        return Ok(());
    };
    let root = cst::parse(text);
    let index = LineIndex::new(text);
    let lsp_diags = diagnostics::collect(&root, text)
        .into_iter()
        .map(|d| {
            let range = Range::new(
                index.position(text, d.span.start),
                index.position(text, d.span.end),
            );
            let mut diag = Diagnostic::new_simple(range, d.message);
            diag.severity = Some(DiagnosticSeverity::ERROR);
            diag.source = Some("brood".to_string());
            diag
        })
        .collect();
    send_diagnostics(connection, uri, lsp_diags)
}

fn send_diagnostics(
    connection: &Connection,
    uri: &Uri,
    diagnostics: Vec<Diagnostic>,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let params = PublishDiagnosticsParams::new(uri.clone(), diagnostics, None);
    let not = ServerNotification::new(PublishDiagnostics::METHOD.to_string(), params);
    connection.sender.send(Message::Notification(not))?;
    Ok(())
}

/// Integration tests for the server message loop, driven over an in-process
/// `Connection::memory()` pair (the rust-analyzer test pattern): a thread runs
/// `main_loop` on the server end while the test plays the client. `initialize`
/// is consumed in `main` before `main_loop`, so these drive the loop directly
/// with document notifications and a `shutdown`/`exit` to end it.
#[cfg(test)]
mod server_tests {
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
                        text: "(foo)".into(),
                    }],
                },
            ))
            .unwrap();
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

    #[test]
    fn unknown_request_gets_method_not_found() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(7),
                "textDocument/hover".to_string(),
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
}
