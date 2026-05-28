//! `brood-lsp` — the Brood language server. A separate binary that speaks LSP
//! over stdio so any editor gets Brood's language knowledge without
//! re-implementing it. See `docs/lsp.md` for the design and ADR-025.
//!
//! Tier 0: lifecycle, full-document sync, and **syntactic diagnostics** read off
//! the tooling CST ([`brood::syntax::cst`]). Tier 1 (the [`completion`],
//! [`hover`], [`symbols`], and [`definition`] modules): name completion, hover
//! docs, the document outline, and goto-definition. The server never evaluates
//! document text — diagnostics and navigation come from parsing + the CST scope
//! walker ([`brood::syntax::scope`]), and the one [`Interp`] it owns answers only
//! introspection queries about the *language's* globals (never user code). A
//! half-typed buffer must stay safe and can't be run. It uses the synchronous
//! `lsp-server` stack (no async runtime): a single blocking request loop owns the
//! document store + the `Interp`, sidestepping the `!Sync` `Heap`.

// `lsp_types::Uri` trips clippy's `mutable_key_type` lint (it wraps a
// `fluent_uri` type clippy can't prove is immutable), but it's an interned,
// effectively-immutable URI — the canonical document-store key. False positive.
#![allow(clippy::mutable_key_type)]

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, ErrorCode, Message, Notification as ServerNotification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as NotificationTrait, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as RequestTrait,
    SignatureHelpRequest,
};
use lsp_types::{
    CompletionOptions, CompletionParams, Diagnostic, DiagnosticSeverity, DocumentSymbolParams,
    GotoDefinitionParams, HoverParams, HoverProviderCapability, OneOf, Position,
    PositionEncodingKind, PublishDiagnosticsParams, Range, ServerCapabilities, SignatureHelpOptions,
    SignatureHelpParams, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use brood::core::value::Value;
use brood::syntax::{cst, reader, scope};
use brood::types::check::check_file;
use brood::Interp;

mod completion;
mod definition;
mod defs;
mod diagnostics;
mod hover;
mod line_index;
mod signature;
mod symbols;

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
        // Tier 1 (docs/lsp.md): completion, hover, document symbols, and
        // goto-definition. The completion `trigger_characters` stay default —
        // the client requests on identifier chars, which is what we want.
        completion_provider: Some(CompletionOptions::default()),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        // Args are whitespace-separated in Lisp, so `(` opens signature help and
        // a space re-triggers it onto the next parameter.
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), " ".to_string()]),
            retrigger_characters: Some(vec![" ".to_string()]),
            work_done_progress_options: Default::default(),
        }),
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
    // One interpreter, loaded with the prelude + builtins, answers introspection
    // queries (completion candidates, hover signatures) and runs the advisory
    // type checker over each document. The first time a file under a project is
    // opened, its `project.blsp` + sources + the test framework are loaded once
    // into this Interp (see `bootstrap_project`), so cross-module names and
    // `describe`/`test`/`assert=`/`is` resolve. Project roots already bootstrapped
    // are tracked here so subsequent edits don't re-load. See `docs/lsp.md`.
    let mut interp = Interp::new();
    let mut bootstrapped: HashSet<PathBuf> = HashSet::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                // `handle_shutdown` performs the shutdown/exit handshake and
                // returns true when it was that request, at which point we stop.
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                let resp = handle_request(&docs, &mut interp, req);
                connection.sender.send(Message::Response(resp))?;
            }
            Message::Response(_) => {} // we issue no server→client requests yet
            Message::Notification(not) => {
                handle_notification(connection, &mut docs, &mut interp, &mut bootstrapped, not)?;
            }
        }
    }
    Ok(())
}

/// Build the per-request analysis of a document: its CST, scope tree, and line
/// index. All three are cheap to derive, so we rebuild them per request rather
/// than cache (full-document sync, ADR-011 — the simple shape until a need
/// justifies more). The `ScopeTree` owns its data (no borrow of `root`).
fn analyze(text: &str) -> (cst::Node, scope::ScopeTree, LineIndex) {
    let root = cst::parse(text);
    let tree = scope::analyze(&root, text);
    let index = LineIndex::new(text);
    (root, tree, index)
}

/// Deserialize a request's params, mapping a bad payload to an `InvalidParams`
/// error response (with the request's id) rather than a panic. The method has
/// already been matched, so the only failure is a params-shape mismatch.
fn extract<P: serde::de::DeserializeOwned>(req: Request) -> Result<(RequestId, P), Response> {
    let id = req.id.clone();
    let method = req.method.clone();
    req.extract::<P>(&method).map_err(|_| {
        Response::new_err(
            id,
            ErrorCode::InvalidParams as i32,
            format!("invalid params for {method}"),
        )
    })
}

/// Dispatch a client request to its Tier-1 feature handler, producing the
/// response to send. An unknown method gets `MethodNotFound`; a request for a
/// document we don't have gets a null result (the spec's "no information").
fn handle_request(docs: &Documents, interp: &mut Interp, req: Request) -> Response {
    match req.method.as_str() {
        HoverRequest::METHOD => {
            let (id, p) = match extract::<HoverParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position_params;
            let result = docs.get(&pos.text_document.uri).and_then(|text| {
                let (root, tree, index) = analyze(text);
                let offset = index.offset(text, pos.position);
                hover::hover(interp, text, &root, &tree, &index, offset)
            });
            Response::new_ok(id, result)
        }
        Completion::METHOD => {
            let (id, p) = match extract::<CompletionParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position;
            let result = docs.get(&pos.text_document.uri).map(|text| {
                let (_root, tree, index) = analyze(text);
                let offset = index.offset(text, pos.position);
                completion::completions(interp, &tree, offset)
            });
            Response::new_ok(id, result)
        }
        DocumentSymbolRequest::METHOD => {
            let (id, p) = match extract::<DocumentSymbolParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = docs.get(&p.text_document.uri).map(|text| {
                let (root, _tree, index) = analyze(text);
                symbols::document_symbols(&root, text, &index)
            });
            Response::new_ok(id, result)
        }
        GotoDefinition::METHOD => {
            let (id, p) = match extract::<GotoDefinitionParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position_params;
            let uri = pos.text_document.uri;
            let result = docs.get(&uri).and_then(|text| {
                let (root, tree, index) = analyze(text);
                let offset = index.offset(text, pos.position);
                definition::definition(&uri, text, &root, &tree, &index, offset)
            });
            Response::new_ok(id, result)
        }
        SignatureHelpRequest::METHOD => {
            let (id, p) = match extract::<SignatureHelpParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position_params;
            let result = docs.get(&pos.text_document.uri).and_then(|text| {
                let (root, tree, index) = analyze(text);
                let offset = index.offset(text, pos.position);
                signature::signature_help(interp, text, &root, &tree, offset)
            });
            Response::new_ok(id, result)
        }
        // Nothing else is advertised: reply method-not-found rather than leave
        // the client waiting on a response.
        _ => Response::new_err(
            req.id,
            ErrorCode::MethodNotFound as i32,
            format!("unsupported request: {}", req.method),
        ),
    }
}

fn handle_notification(
    connection: &Connection,
    docs: &mut Documents,
    interp: &mut Interp,
    bootstrapped: &mut HashSet<PathBuf>,
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
            publish(connection, docs, interp, bootstrapped, &uri)?;
        }
        DidChangeTextDocument::METHOD => {
            let Some(p) = params::<lsp_types::DidChangeTextDocumentParams>(not) else {
                return Ok(());
            };
            // Full sync: the last change event carries the entire new document.
            if let Some(change) = p.content_changes.into_iter().last() {
                let uri = p.text_document.uri;
                docs.insert(uri.clone(), change.text);
                publish(connection, docs, interp, bootstrapped, &uri)?;
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

/// Extract the filesystem path from a `file://` URI. Best-effort: typical Unix
/// paths have no percent-encoded characters, and a non-`file://` URI (or any
/// path we can't reason about) returns `None` so callers skip project work.
fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    uri.as_str().strip_prefix("file://").map(PathBuf::from)
}

/// Walk up from `file_path` looking for a directory containing `project.blsp`,
/// the project root marker. `None` if the file isn't inside a Brood project.
fn find_project_root(file_path: &Path) -> Option<PathBuf> {
    let mut dir = file_path.parent()?;
    loop {
        if dir.join("project.blsp").exists() {
            return Some(dir.to_path_buf());
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => return None,
        }
    }
}

/// Bootstrap the project rooted at the file in `uri` — once per project root
/// per server lifetime. Loads the manifest, puts source dirs on `*load-path*`,
/// loads every project source so cross-module names resolve, and `require`s
/// the test framework so `describe`/`test`/`assert=`/`is` are bound in test
/// files. Cached in `bootstrapped` so we don't re-load on every keystroke.
/// Best-effort: failures log and continue (the checker still runs with at
/// least the prelude). Files outside a project are a silent no-op.
fn bootstrap_project(interp: &mut Interp, bootstrapped: &mut HashSet<PathBuf>, uri: &Uri) {
    let Some(file_path) = uri_to_path(uri) else { return };
    let Some(root) = find_project_root(&file_path) else { return };
    if bootstrapped.contains(&root) {
        return;
    }
    // Escape backslashes and quotes for embedding into a Brood string literal.
    // Common Unix paths have neither, but be safe.
    let esc = root.display().to_string().replace('\\', "\\\\").replace('"', "\\\"");
    let cmd = format!(
        "(require 'project) (project-setup \"{e}\") (project-load-sources \"{e}\") (require 'test)",
        e = esc,
    );
    if let Err(e) = interp.eval_str(&cmd) {
        eprintln!("brood-lsp: project bootstrap failed for {}: {e}", root.display());
    }
    // Mark bootstrapped regardless of success — a partial load is consistent
    // (each top-level form's `eval_str` is checkpointed), and re-running on
    // every publish would re-load every source on every keystroke.
    bootstrapped.insert(root);
}

/// Parse the document and publish two tiers of diagnostics:
/// (1) **syntactic errors** — `Error` nodes in the tooling CST (parser failures,
///     always severity ERROR; the document doesn't parse).
/// (2) **advisory type-check warnings** — `check_file` over the positioned
///     forms (severity WARNING; the document parses but the checker spotted
///     something — unbound names, arity mismatch, type-misuse). Project sources
///     and the test framework are pre-loaded via `bootstrap_project`, so
///     cross-module references and test-framework macros resolve.
fn publish(
    connection: &Connection,
    docs: &Documents,
    interp: &mut Interp,
    bootstrapped: &mut HashSet<PathBuf>,
    uri: &Uri,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let Some(text) = docs.get(uri) else {
        return Ok(());
    };

    // Make project-local + test-framework names visible to the checker
    // (idempotent, cached per project root). No-op outside a project.
    bootstrap_project(interp, bootstrapped, uri);

    let cst_root = cst::parse(text);
    let index = LineIndex::new(text);

    // (1) Syntactic diagnostics — Tier 0.
    let mut lsp_diags: Vec<Diagnostic> = diagnostics::collect(&cst_root, text)
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

    // (2) Type-check warnings — Tier 1, only when the parse succeeded enough to
    // read positioned forms. Wrapped in an arena checkpoint so the document's
    // parsed forms (allocated in LOCAL) are reclaimed after the check — the
    // Interp's heap doesn't grow per keystroke. Project sources / `defn`s the
    // bootstrap loaded promote to RUNTIME, so they survive this reset.
    let cp = interp.heap.checkpoint();
    if let Ok(positioned) = reader::read_all_positioned(&mut interp.heap, text) {
        let forms: Vec<Value> = positioned.into_iter().map(|(f, _)| f).collect();
        for (pos_opt, msg) in check_file(&mut interp.heap, &forms) {
            if let Some(pos) = pos_opt {
                // `Pos` is 1-based; LSP `Position` is 0-based. A 1-character
                // marker is enough to anchor the squiggle — editors widen it
                // to the token under it. `saturating_*` handles the edge cases
                // (`pos.line == 0`, end-of-line columns) without panicking.
                let line = pos.line.saturating_sub(1);
                let col = pos.col.saturating_sub(1);
                let start = Position::new(line, col);
                let end = Position::new(line, col.saturating_add(1));
                let mut diag = Diagnostic::new_simple(Range::new(start, end), msg);
                diag.severity = Some(DiagnosticSeverity::WARNING);
                diag.source = Some("brood".to_string());
                lsp_diags.push(diag);
            }
        }
    }
    interp.heap.reset_local_to(cp);

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
            match client.receiver.recv().expect("server closed before response") {
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
        let syms: Vec<lsp_types::DocumentSymbol> = serde_json::from_value(r.result.unwrap()).unwrap();
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
    fn unknown_request_gets_method_not_found() {
        let (server, client) = Connection::memory();
        let handle = thread::spawn(move || main_loop(&server));

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(7),
                "textDocument/formatting".to_string(), // not advertised
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
