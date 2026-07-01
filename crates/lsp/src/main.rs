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
// LSP handlers thread the document store + position + params (8 args); bundling
// them buys nothing. Accepted crate-wide like the lib (see its note).
#![allow(clippy::too_many_arguments)]

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::{Path, PathBuf};

use lsp_server::{
    Connection, ErrorCode, Message, Notification as ServerNotification, Request, RequestId,
    Response,
};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as NotificationTrait, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentLinkRequest,
    DocumentSymbolRequest, FoldingRangeRequest, Formatting, GotoDefinition, HoverRequest,
    InlayHintRequest, PrepareRenameRequest, References, Rename, Request as RequestTrait,
    ResolveCompletionItem, SelectionRangeRequest, SemanticTokensFullRequest, SignatureHelpRequest,
    WorkspaceSymbolRequest,
};
use lsp_types::{
    CodeActionParams, CodeActionProviderCapability, CompletionItem, CompletionOptions,
    CompletionParams, Diagnostic, DiagnosticSeverity, DocumentFormattingParams,
    DocumentHighlightParams, DocumentLinkOptions, DocumentLinkParams, DocumentSymbolParams,
    FoldingRangeParams, FoldingRangeProviderCapability, GotoDefinitionParams, HoverParams,
    HoverProviderCapability, InlayHintParams, OneOf, Position, PositionEncodingKind,
    PrepareRenameResponse, PublishDiagnosticsParams, Range, ReferenceParams, RenameOptions,
    RenameParams, SelectionRangeParams, SelectionRangeProviderCapability,
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensParams, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, SignatureHelpOptions,
    SignatureHelpParams, TextDocumentPositionParams, TextDocumentSyncCapability,
    TextDocumentSyncKind, Uri, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};

use brood::core::value::Value;
use brood::syntax::scope::{BindingKind, Resolution};
use brood::syntax::{cst, reader, scope};
use brood::types::check::check_file;
use brood::Interp;

mod code_actions;
mod completion;
mod definition;
mod defs;
mod diagnostics;
mod document_link;
mod folding;
mod formatting;
mod hover;
mod inlay_hints;
mod line_index;
mod module_ref;
mod references;
mod rename;
mod selection_range;
mod semantic_tokens;
mod signature;
mod symbols;
mod workspace;
mod workspace_symbols;

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
        // Completion offers locals + special forms + globals; `resolve_provider`
        // lets us fill each item's signature/docstring lazily on
        // `completionItem/resolve`. Trigger chars stay default (identifier chars).
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(true),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        // Project-wide symbol search over every file's top-level definitions.
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // Quick-fixes off published diagnostics (e.g. "did you mean?" for an
        // unbound symbol). Simple capability — no codeAction/resolve.
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        // Collapsible regions (multi-line forms, comment blocks) off the CST.
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        // Parameter-name hints at call sites, from `arglist`.
        inlay_hint_provider: Some(OneOf::Left(true)),
        // Whole-document formatting, delegated to the Brood formatter
        // (`std/format.blsp`) via `introspect::format_source`. Range/onType
        // formatting isn't offered — the formatter works on whole files.
        document_formatting_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        // Smart expand/shrink selection along the CST (symbol → form → outer
        // form → file) — especially natural for s-expressions.
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        // Clickable links over module names that resolve to a file — `(require
        // 'foo)` arguments and `(:use foo)`/`(:alias foo)` clauses. No resolve
        // step: each link carries its target URI up front.
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        // Rename, with `prepareRename` so the editor validates/highlights the
        // span before prompting.
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        // Semantic tokens (whole-document) — meaning-based highlighting off the
        // CST + scope tree. Range requests aren't offered (full is cheap enough).
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: semantic_tokens::legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(false),
                work_done_progress_options: Default::default(),
            },
        )),
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

/// Per-open-document state: the source text plus its cached [`Analysis`]. The
/// CST + scope tree + line index are derived once per document version (on
/// `did_open` / `did_change`) and reused for every request and the diagnostic
/// publish — pre-cache, hover / completion / signature / publish each parsed
/// the document afresh, so a single keystroke cost ~4 parses + 4 line-indexes.
type Documents = HashMap<Uri, Document>;

/// One open document — the text the editor sent plus its derived analysis.
/// Replace the whole `Document` on every `did_change` so cache and text stay
/// in sync without invalidation logic.
struct Document {
    text: String,
    analysis: Analysis,
    /// The editor's version for this text, echoed back on `publishDiagnostics`
    /// so the client can discard diagnostics for a stale version.
    version: i32,
}

/// All read-only views of a document version that every LSP request reuses:
/// the CST, the scope tree built from it, and the byte→line/col index.
/// Cheap to build once; ruinously expensive to build per keystroke on a big
/// buffer.
pub(crate) struct Analysis {
    pub(crate) cst: cst::Node,
    pub(crate) scope: scope::ScopeTree,
    pub(crate) line_index: LineIndex,
}

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

/// Build the analysis of a document — its CST, scope tree, and line index.
/// All three are derived from the source text; cached on the [`Document`] so
/// every request against the same document version reuses one parse.
fn analyze(text: &str) -> Analysis {
    let cst = cst::parse(text);
    let scope = scope::analyze(&cst, text);
    let line_index = LineIndex::new(text);
    Analysis {
        cst,
        scope,
        line_index,
    }
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
            let result = docs.get(&pos.text_document.uri).and_then(|doc| {
                let a = &doc.analysis;
                let offset = a.line_index.offset(&doc.text, pos.position);
                hover::hover(interp, &doc.text, &a.cst, &a.scope, &a.line_index, offset)
            });
            Response::new_ok(id, result)
        }
        Completion::METHOD => {
            let (id, p) = match extract::<CompletionParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position;
            let result = docs.get(&pos.text_document.uri).map(|doc| {
                let a = &doc.analysis;
                let offset = a.line_index.offset(&doc.text, pos.position);
                completion::completions(interp, &a.scope, &a.cst, &doc.text, offset)
            });
            Response::new_ok(id, result)
        }
        DocumentSymbolRequest::METHOD => {
            let (id, p) = match extract::<DocumentSymbolParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = docs.get(&p.text_document.uri).map(|doc| {
                let a = &doc.analysis;
                symbols::document_symbols(&a.cst, &doc.text, &a.line_index)
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
            // Not a closure: goto-definition needs `&mut interp` (for the
            // cross-file `source-location` fallback) alongside the immutable
            // `docs` borrow, so inline the lookup to keep both borrows separate.
            let result = match docs.get(&uri) {
                Some(doc) => {
                    let a = &doc.analysis;
                    let offset = a.line_index.offset(&doc.text, pos.position);
                    definition::definition(
                        interp,
                        &uri,
                        &doc.text,
                        &a.cst,
                        &a.scope,
                        &a.line_index,
                        offset,
                    )
                }
                None => None,
            };
            Response::new_ok(id, result)
        }
        SelectionRangeRequest::METHOD => {
            let (id, p) = match extract::<SelectionRangeParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = docs.get(&p.text_document.uri).map(|doc| {
                let a = &doc.analysis;
                selection_range::selection_ranges(&a.cst, &doc.text, &a.line_index, &p.positions)
            });
            Response::new_ok(id, result)
        }
        DocumentLinkRequest::METHOD => {
            let (id, p) = match extract::<DocumentLinkParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            // Like goto-definition, this needs `&mut interp` (module resolution runs
            // `require--find`) alongside the `docs` borrow — inline the lookup.
            let result = match docs.get(&p.text_document.uri) {
                Some(doc) => {
                    let a = &doc.analysis;
                    document_link::document_links(interp, &doc.text, &a.cst, &a.line_index)
                }
                None => Vec::new(),
            };
            Response::new_ok(id, result)
        }
        SignatureHelpRequest::METHOD => {
            let (id, p) = match extract::<SignatureHelpParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position_params;
            let result = docs.get(&pos.text_document.uri).and_then(|doc| {
                let a = &doc.analysis;
                let offset = a.line_index.offset(&doc.text, pos.position);
                signature::signature_help(interp, &doc.text, &a.cst, &a.scope, offset)
            });
            Response::new_ok(id, result)
        }
        References::METHOD => {
            let (id, p) = match extract::<ReferenceParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position;
            let uri = pos.text_document.uri;
            // A local → single-file (its own scope). A global / free name → the
            // whole project (flat module model, ADR-019), via `workspace`.
            let result = match docs.get(&uri) {
                Some(doc) => {
                    let a = &doc.analysis;
                    let offset = a.line_index.offset(&doc.text, pos.position);
                    Some(match a.scope.resolve_at(&a.cst, &doc.text, offset) {
                        Resolution::Defined {
                            kind: BindingKind::Local,
                            ..
                        } => references::references(
                            &uri,
                            &doc.text,
                            &a.cst,
                            &a.scope,
                            &a.line_index,
                            offset,
                        ),
                        Resolution::Defined { .. } | Resolution::Free => {
                            match workspace::symbol_at(&a.cst, &doc.text, offset) {
                                Some(name) => {
                                    let name = name.to_string();
                                    workspace::references(interp, docs, &uri, &name)
                                }
                                None => Vec::new(),
                            }
                        }
                        Resolution::NotASymbol => Vec::new(),
                    })
                }
                None => None,
            };
            Response::new_ok(id, result)
        }
        DocumentHighlightRequest::METHOD => {
            let (id, p) = match extract::<DocumentHighlightParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position_params;
            let result = docs.get(&pos.text_document.uri).map(|doc| {
                let a = &doc.analysis;
                let offset = a.line_index.offset(&doc.text, pos.position);
                references::document_highlights(&doc.text, &a.cst, &a.scope, &a.line_index, offset)
            });
            Response::new_ok(id, result)
        }
        PrepareRenameRequest::METHOD => {
            let (id, p) = match extract::<TextDocumentPositionParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = docs.get(&p.text_document.uri).and_then(|doc| {
                let a = &doc.analysis;
                let offset = a.line_index.offset(&doc.text, p.position);
                rename::prepare_rename(&doc.text, &a.cst, &a.scope, &a.line_index, offset)
                    .map(PrepareRenameResponse::Range)
            });
            Response::new_ok(id, result)
        }
        Rename::METHOD => {
            let (id, p) = match extract::<RenameParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let pos = p.text_document_position;
            let uri = pos.text_document.uri;
            // Local → single-file edit; global → a project-wide `WorkspaceEdit`.
            let result = match docs.get(&uri) {
                Some(doc) => {
                    let a = &doc.analysis;
                    let offset = a.line_index.offset(&doc.text, pos.position);
                    match a.scope.resolve_at(&a.cst, &doc.text, offset) {
                        Resolution::Defined {
                            kind: BindingKind::Local,
                            ..
                        } => rename::rename(
                            &uri,
                            &doc.text,
                            &a.cst,
                            &a.scope,
                            &a.line_index,
                            offset,
                            &p.new_name,
                        ),
                        Resolution::Defined { .. } | Resolution::Free => {
                            match workspace::symbol_at(&a.cst, &doc.text, offset) {
                                Some(name) => {
                                    let name = name.to_string();
                                    workspace::rename(interp, docs, &uri, &name, &p.new_name)
                                }
                                None => None,
                            }
                        }
                        Resolution::NotASymbol => None,
                    }
                }
                None => None,
            };
            Response::new_ok(id, result)
        }
        SemanticTokensFullRequest::METHOD => {
            let (id, p) = match extract::<SemanticTokensParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = docs.get(&p.text_document.uri).map(|doc| {
                let a = &doc.analysis;
                SemanticTokensResult::Tokens(semantic_tokens::semantic_tokens(
                    &doc.text,
                    &a.cst,
                    &a.scope,
                    &a.line_index,
                ))
            });
            Response::new_ok(id, result)
        }
        ResolveCompletionItem::METHOD => {
            let (id, item) = match extract::<CompletionItem>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            Response::new_ok(id, completion::resolve(interp, item))
        }
        Formatting::METHOD => {
            let (id, p) = match extract::<DocumentFormattingParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            // `&mut interp` (format-source evaluates the Brood formatter) plus the
            // immutable `docs` borrow — inline like goto-definition to keep both.
            let result = match docs.get(&p.text_document.uri) {
                Some(doc) => formatting::formatting(interp, &doc.text, &doc.analysis.line_index),
                None => None,
            };
            Response::new_ok(id, result)
        }
        WorkspaceSymbolRequest::METHOD => {
            let (id, p) = match extract::<WorkspaceSymbolParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let symbols = workspace_symbols::workspace_symbols(interp, docs, &p.query);
            Response::new_ok(id, WorkspaceSymbolResponse::Nested(symbols))
        }
        CodeActionRequest::METHOD => {
            let (id, p) = match extract::<CodeActionParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = match docs.get(&p.text_document.uri) {
                Some(doc) => {
                    let a = &doc.analysis;
                    // Resolve a diagnostic's range to the byte offset of its
                    // start — where the unbound name sits, for `names_in_scope`.
                    let offset_of = |r: Range| a.line_index.offset(&doc.text, r.start);
                    let mut acts = code_actions::code_actions(
                        interp,
                        &p.text_document.uri,
                        &a.cst,
                        &doc.text,
                        &a.scope,
                        &a.line_index,
                        offset_of,
                        &p.context.diagnostics,
                    );
                    // Structural fixes (not tied to a published diagnostic):
                    // offer to remove a seemingly-unused require under the cursor.
                    let req_start = a.line_index.offset(&doc.text, p.range.start);
                    let req_end = a.line_index.offset(&doc.text, p.range.end);
                    acts.extend(code_actions::unused_require_actions(
                        &p.text_document.uri,
                        &a.cst,
                        &doc.text,
                        &a.line_index,
                        req_start,
                        req_end,
                    ));
                    acts
                }
                None => Vec::new(),
            };
            Response::new_ok(id, result)
        }
        FoldingRangeRequest::METHOD => {
            let (id, p) = match extract::<FoldingRangeParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = docs.get(&p.text_document.uri).map(|doc| {
                let a = &doc.analysis;
                folding::folding_ranges(&a.cst, &doc.text, &a.line_index)
            });
            Response::new_ok(id, result)
        }
        InlayHintRequest::METHOD => {
            let (id, p) = match extract::<InlayHintParams>(req) {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            let result = match docs.get(&p.text_document.uri) {
                Some(doc) => {
                    let a = &doc.analysis;
                    let range = (
                        a.line_index.offset(&doc.text, p.range.start),
                        a.line_index.offset(&doc.text, p.range.end),
                    );
                    Some(inlay_hints::inlay_hints(
                        interp,
                        &a.cst,
                        &doc.text,
                        &a.scope,
                        &a.line_index,
                        range,
                    ))
                }
                None => None,
            };
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
            let text = p.text_document.text;
            let version = p.text_document.version;
            // Cache the analysis once per document version — every later
            // request against this URI reads from `doc.analysis` rather than
            // re-parsing the source.
            let analysis = analyze(&text);
            docs.insert(
                uri.clone(),
                Document {
                    text,
                    analysis,
                    version,
                },
            );
            publish(connection, docs, interp, bootstrapped, &uri)?;
        }
        DidChangeTextDocument::METHOD => {
            let Some(p) = params::<lsp_types::DidChangeTextDocumentParams>(not) else {
                return Ok(());
            };
            // Full sync: the last change event carries the entire new document.
            if let Some(change) = p.content_changes.into_iter().last() {
                let uri = p.text_document.uri;
                let text = change.text;
                let version = p.text_document.version;
                let analysis = analyze(&text);
                docs.insert(
                    uri.clone(),
                    Document {
                        text,
                        analysis,
                        version,
                    },
                );
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
            send_diagnostics(connection, &uri, Vec::new(), None)?;
        }
        DidSaveTextDocument::METHOD => {
            // A `project.blsp` save invalidates the cached project bootstrap:
            // the user just edited the project's manifest (modules, deps,
            // entry, …) and a hover / check from now on must see the new
            // state. Evicting the root from `bootstrapped` makes the next
            // `publish` re-run `bootstrap_project`, which re-evaluates the
            // project's source set into the live `Interp`. Per-source-file
            // saves don't need this — the buffer text already drives publish.
            let Some(p) = params::<lsp_types::DidSaveTextDocumentParams>(not) else {
                return Ok(());
            };
            let uri = p.text_document.uri;
            if let Some(path) = uri_to_path(&uri) {
                if path.file_name().and_then(|n| n.to_str()) == Some("project.blsp") {
                    if let Some(root) = path.parent() {
                        bootstrapped.remove(root);
                    }
                }
            }
            // Re-publish diagnostics against the (possibly re-bootstrapped)
            // image so the user sees the effect of their save right away.
            publish(connection, docs, interp, bootstrapped, &uri)?;
        }
        _ => {} // initialized, didChangeConfiguration, … — nothing to do yet
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

/// Extract the filesystem path from a `file://` URI. Percent-decodes the path
/// so an editor URI for `/home/Wilhelm Kirschbaum/proj/` (`%20`-escaped) maps
/// back to the real on-disk path — without this, `find_project_root` silently
/// failed for any path containing whitespace or non-ASCII bytes. A non-`file:`
/// URI returns `None` so callers skip project work.
///
/// Handles both `file:///abs/path` (empty authority — the common form) and
/// `file://host/abs/path` (some WSL / remote clients): the authority component
/// is dropped and the path taken from its leading `/`. Without this, a
/// host-bearing URI decoded to a *relative* path (`host/abs/path`) and project
/// bootstrap silently never fired.
fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // Empty authority → `rest` already starts at the path's `/`. A non-empty
    // authority (a host) precedes the first `/`; the path begins there.
    let path = if rest.starts_with('/') {
        rest
    } else {
        &rest[rest.find('/')?..]
    };
    Some(PathBuf::from(percent_decode(path)))
}

/// Build a `file://` URI from an absolute filesystem path — the inverse of
/// [`uri_to_path`], for the cross-file `Location`s goto-definition returns.
/// Percent-encodes every byte outside the URI "unreserved" set (plus `/`), so
/// spaces and non-ASCII path components round-trip. `None` if the result somehow
/// doesn't parse as a URI (it always should for an absolute path).
pub(crate) fn path_to_uri(path: &str) -> Option<Uri> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::from("file://");
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                s.push(b as char)
            }
            _ => {
                s.push('%');
                s.push(HEX[(b >> 4) as usize] as char);
                s.push(HEX[(b & 0xf) as usize] as char);
            }
        }
    }
    s.parse().ok()
}

#[cfg(test)]
mod uri_tests {
    use super::*;

    fn p(uri: &str) -> Option<String> {
        uri_to_path(&uri.parse::<Uri>().ok()?).map(|p| p.display().to_string())
    }

    #[test]
    fn uri_to_path_handles_empty_and_host_authorities() {
        // The common form: empty authority, leading `/` is the path.
        assert_eq!(
            p("file:///home/x/a.blsp").as_deref(),
            Some("/home/x/a.blsp")
        );
        // A host authority (WSL / remote) is dropped; the path stays absolute.
        assert_eq!(
            p("file://localhost/home/x/a.blsp").as_deref(),
            Some("/home/x/a.blsp")
        );
        // Percent-escapes in the path decode (a space here).
        assert_eq!(
            p("file:///home/a%20b/p.blsp").as_deref(),
            Some("/home/a b/p.blsp")
        );
    }

    #[test]
    fn path_to_uri_round_trips_through_uri_to_path() {
        for path in ["/home/x/a.blsp", "/home/a b/p.blsp", "/tmp/π/x.blsp"] {
            let uri = path_to_uri(path).expect("uri");
            assert_eq!(uri_to_path(&uri).unwrap().display().to_string(), path);
        }
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    /// The advisory type-check diagnostics the LSP would publish for `src` — the
    /// exact path `publish` takes, minus the wire send. `Interp::new()` loads the
    /// prelude, so prelude names (`cons`, `inc`, `map`, …) resolve.
    fn warnings(src: &str) -> Vec<Diagnostic> {
        let mut interp = brood::Interp::new();
        let a = analyze(src);
        typecheck_diagnostics(&mut interp, src, &a.cst, &a.line_index)
    }

    #[test]
    fn surfaces_the_callback_arity_warning_as_a_brood_warning() {
        // The Step-5+ arrow check (ADR-078) must reach the editor: `map` calls
        // its callback with one arg, but `cons` takes two.
        let diags = warnings("(def r (map cons (list 1 2 3)))");
        let hit = diags
            .iter()
            .find(|d| d.message.contains("callback") && d.message.contains("cons"))
            .expect("expected a callback-arity warning");
        assert_eq!(hit.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(hit.source.as_deref(), Some("brood"));
    }

    #[test]
    fn a_correct_arity_callback_produces_no_callback_warning() {
        let diags = warnings("(def r (map inc (list 1 2 3)))");
        assert!(
            diags.iter().all(|d| !d.message.contains("callback")),
            "a correct-arity callback must not warn: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}

/// Tiny `%`-decoder for the path portion of a `file://` URI — no allocation
/// unless the path actually contains a `%`. Invalid escapes (`%XY` with
/// non-hex digits, or a trailing `%`) pass through literally rather than
/// returning an error: the caller's failure mode (`exists()` returns false)
/// is already the right one for a path we can't make sense of.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    // `from_utf8_lossy` for the path-with-replacement-char fallback; the OS
    // won't accept a malformed-utf8 path anyway, and `String` is the public
    // shape `PathBuf::from` takes.
    String::from_utf8_lossy(&out).into_owned()
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
    let Some(file_path) = uri_to_path(uri) else {
        return;
    };
    let Some(root) = find_project_root(&file_path) else {
        return;
    };
    if bootstrapped.contains(&root) {
        return;
    }
    // Load the project image for tooling through the shared seam, so the LSP
    // and `nest mcp` can't drift on which frameworks a tooling image carries
    // (this used to inline `project-setup`/`load-sources`/`require 'test` and
    // omitted `'format`). Policy lives in Brood (`setup-tooling-image`).
    if let Err(e) = brood::introspect::load_tooling_image(interp, &root.display().to_string()) {
        eprintln!(
            "brood-lsp: project bootstrap failed for {}: {e}",
            root.display()
        );
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
    let Some(doc) = docs.get(uri) else {
        return Ok(());
    };
    let text = &doc.text;
    let cst_root = &doc.analysis.cst;
    let index = &doc.analysis.line_index;

    // Make project-local + test-framework names visible to the checker
    // (idempotent, cached per project root). No-op outside a project.
    bootstrap_project(interp, bootstrapped, uri);

    // (1) Syntactic diagnostics — Tier 0.
    let mut lsp_diags: Vec<Diagnostic> = diagnostics::collect(cst_root, text)
        .into_iter()
        .map(|d| {
            let range = index.range(text, d.span);
            let mut diag = Diagnostic::new_simple(range, d.message);
            diag.severity = Some(DiagnosticSeverity::ERROR);
            diag.source = Some("brood".to_string());
            diag
        })
        .collect();

    // (2) Type-check warnings — Tier 1, only when the parse succeeded enough to
    // read positioned forms. Skipped for the manifest: `project.blsp` is read as
    // *data* by the project loader (`project--apply`) and never evaluated as code,
    // so its `(project …)` head isn't a binding — running the checker on it would
    // emit a spurious `unbound symbol: project`. Tier-0 syntax errors still apply.
    if !is_manifest_uri(uri) {
        lsp_diags.extend(typecheck_diagnostics(interp, text, cst_root, index));
    }

    send_diagnostics(connection, uri, lsp_diags, Some(doc.version))
}

/// Whether `uri` names a project manifest (`project.blsp`). The manifest is data
/// consumed by the project loader, not evaluatable code, so the advisory
/// type-checker must not run on it (see [`publish`]).
fn is_manifest_uri(uri: &Uri) -> bool {
    uri_to_path(uri)
        .map(|p| p.file_name().and_then(|n| n.to_str()) == Some("project.blsp"))
        .unwrap_or(false)
}

/// The Tier-1 advisory type-check diagnostics for `text`: run [`check_file`] over
/// the positioned forms and turn each finding into a located `WARNING`. Pulled
/// out of [`publish`] so it can be unit-tested without a wire connection.
///
/// Wrapped in an arena checkpoint so the document's parsed forms (allocated in
/// LOCAL) are reclaimed after the check — the `Interp`'s heap doesn't grow per
/// keystroke. Project sources / `defn`s the bootstrap loaded promote to RUNTIME,
/// so they survive this reset.
fn typecheck_diagnostics(
    interp: &mut Interp,
    text: &str,
    cst_root: &cst::Node,
    index: &LineIndex,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let cp = interp.heap.checkpoint();
    if let Ok(positioned) = reader::read_all_positioned(&mut interp.heap, text) {
        let forms: Vec<Value> = positioned.into_iter().map(|(f, _)| f).collect();
        for (pos_opt, msg) in check_file(&mut interp.heap, &forms) {
            if let Some(pos) = pos_opt {
                // `Pos` is 1-based; LSP `Position` is 0-based. Refine the range
                // from the form start to the *offending token* where we can read
                // it off the CST (the named symbol in an "unbound symbol: X", or
                // a call's operator) — else fall back to a 1-char marker the
                // editor widens. `saturating_*` keeps the edges panic-free.
                let line = pos.line.saturating_sub(1);
                let col = pos.col.saturating_sub(1);
                let range = refine_diagnostic_range(cst_root, text, index, line, col, &msg)
                    .unwrap_or_else(|| {
                        let start = Position::new(line, col);
                        Range::new(start, Position::new(line, col.saturating_add(1)))
                    });
                let mut diag = Diagnostic::new_simple(range, msg);
                diag.severity = Some(DiagnosticSeverity::WARNING);
                diag.source = Some("brood".to_string());
                out.push(diag);
            }
        }
    }
    interp.heap.reset_local_to(cp);
    out
}

/// Tighten a checker finding's squiggle from the whole form to the token it's
/// really about. For `unbound symbol: NAME`, the first matching symbol token in
/// the form; otherwise the form's operator (arity / type-misuse are about the
/// call head). `None` if neither is found — the caller uses a 1-char marker.
fn refine_diagnostic_range(
    root: &cst::Node,
    text: &str,
    index: &LineIndex,
    line: u32,
    col: u32,
    msg: &str,
) -> Option<Range> {
    let off = index.offset(text, Position::new(line, col));
    let form = root.node_at(off)?;
    let span = if let Some(name) = msg.strip_prefix("unbound symbol: ") {
        find_symbol(form, text, name.trim())?
    } else {
        let head = form.forms().next()?;
        (head.kind == cst::NodeKind::Symbol).then_some(head.span)?
    };
    Some(index.range(text, span))
}

/// The span of the first `Symbol` token under `node` whose text is `name`.
fn find_symbol(node: &cst::Node, text: &str, name: &str) -> Option<brood::error::Span> {
    if node.kind == cst::NodeKind::Symbol && node.text(text) == name {
        return Some(node.span);
    }
    node.children
        .iter()
        .find_map(|c| find_symbol(c, text, name))
}

fn send_diagnostics(
    connection: &Connection,
    uri: &Uri,
    diagnostics: Vec<Diagnostic>,
    version: Option<i32>,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let params = PublishDiagnosticsParams::new(uri.clone(), diagnostics, version);
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
}
