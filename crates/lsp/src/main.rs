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
    // `main_loop` takes `connection` by value so it (and its `Sender`) is dropped
    // when the loop returns — only then does the stdout writer thread see its
    // channel close and exit, letting `io_threads.join()` complete. Borrowing
    // here instead would deadlock the join.
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
    match not.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let p = not.extract::<lsp_types::DidOpenTextDocumentParams>(DidOpenTextDocument::METHOD)?;
            let uri = p.text_document.uri;
            docs.insert(uri.clone(), p.text_document.text);
            publish(connection, docs, &uri)?;
        }
        DidChangeTextDocument::METHOD => {
            let p = not
                .extract::<lsp_types::DidChangeTextDocumentParams>(DidChangeTextDocument::METHOD)?;
            // Full sync: the last change event carries the entire new document.
            if let Some(change) = p.content_changes.into_iter().last() {
                let uri = p.text_document.uri;
                docs.insert(uri.clone(), change.text);
                publish(connection, docs, &uri)?;
            }
        }
        DidCloseTextDocument::METHOD => {
            let p = not
                .extract::<lsp_types::DidCloseTextDocumentParams>(DidCloseTextDocument::METHOD)?;
            let uri = p.text_document.uri;
            docs.remove(&uri);
            // Clear diagnostics for the closed document.
            send_diagnostics(connection, &uri, Vec::new())?;
        }
        _ => {} // initialized, didSave, cancellations, … — nothing to do yet
    }
    Ok(())
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
