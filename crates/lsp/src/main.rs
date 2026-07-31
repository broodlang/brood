//! `brood-lsp` — the Brood language server. A separate binary that speaks LSP
//! over stdio so any editor gets Brood's language knowledge without
//! re-implementing it. See `docs/lsp.md` for the design and ADR-025.
//!
//! Tier 0: lifecycle, incremental document sync, and **syntactic diagnostics** read off
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
        // Incremental sync: the client sends only the changed range(s) on each
        // keystroke, which we splice into the stored buffer (byte offsets via the
        // UTF-16 `LineIndex`). The *parse* stays whole-document (the reader/CST is
        // cheap) — incremental sync only spares the transport re-sending the whole
        // file on every edit, which matters on a large buffer.
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            lsp_types::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                ..Default::default()
            },
        )),
        // We do UTF-16 column arithmetic in `LineIndex`; advertise it explicitly
        // rather than relying on the protocol default.
        position_encoding: Some(PositionEncodingKind::UTF16),
        // Completion offers locals + special forms + globals; `resolve_provider`
        // lets us fill each item's signature/docstring lazily on
        // `completionItem/resolve`. `:` triggers so the record-field candidates
        // (`completion::record_key_context`) pop up the moment a key is started —
        // an identifier-char default would only fire once a word char follows it.
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(vec![":".to_string()]),
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

    // The initialize/initialized handshake. We read one client capability:
    // whether it understands snippet syntax in completion items (`$0`, `${1:…}`),
    // so the `(impl …)` op-completion sends a fillable skeleton only when the
    // editor can navigate it — and a plain skeleton otherwise, never literal `$`.
    let init = connection.initialize(serde_json::to_value(capabilities)?)?;
    let snippet_support = serde_json::from_value::<lsp_types::InitializeParams>(init)
        .ok()
        .and_then(|params| {
            params
                .capabilities
                .text_document?
                .completion?
                .completion_item?
                .snippet_support
        })
        .unwrap_or(false);
    // Run the loop, then drop `connection` *before* the join: its `Sender` keeps
    // the stdout writer thread alive, so the thread only sees its channel close
    // (and exits, letting `io_threads.join()` return) once this drop happens.
    // Skipping the drop would deadlock the join.
    main_loop(&connection, snippet_support)?;
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

fn main_loop(
    connection: &Connection,
    snippet_support: bool,
) -> Result<(), Box<dyn Error + Sync + Send>> {
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
                let resp = handle_request(&docs, &mut interp, snippet_support, req);
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

/// Apply one incremental `didChange` content-change to `text`, in place.
///
/// A change with a `range` splices `change.text` over that span; a change with no
/// `range` replaces the whole document (a client may send that even under
/// incremental sync). The range is UTF-16 line/col, so it is resolved to byte
/// offsets through a fresh [`LineIndex`] over the *current* `text` — edits within
/// one batch compound, so each must see the text the prior edit produced (rebuild
/// is a single byte scan, cheap). `LineIndex::offset` already clamps a past-EOF
/// position; the extra `min(len)` + `start <= end` guard keeps the splice
/// panic-free on any degenerate range a client might send.
fn apply_content_change(text: &mut String, change: &lsp_types::TextDocumentContentChangeEvent) {
    match change.range {
        Some(range) => {
            let idx = LineIndex::new(text);
            let start = (idx.offset(text, range.start) as usize).min(text.len());
            let end = (idx.offset(text, range.end) as usize).min(text.len());
            if start <= end {
                text.replace_range(start..end, &change.text);
            }
        }
        None => text.clone_from(&change.text),
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
fn handle_request(
    docs: &Documents,
    interp: &mut Interp,
    snippet_support: bool,
    req: Request,
) -> Response {
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
                completion::completions(
                    interp,
                    &a.scope,
                    &a.cst,
                    &doc.text,
                    offset,
                    snippet_support,
                )
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
            // Incremental sync: start from the current buffer and apply each
            // content-change event in order (a ranged change splices its span; a
            // change with no range is a whole-document replace the client may still
            // send). Edits within one batch compound, so each resolves its range
            // against the text the prior edit produced. Re-parse ONCE after all
            // edits — incremental *sync* does not require incremental *parse*.
            let uri = p.text_document.uri;
            let version = p.text_document.version;
            let Some(doc) = docs.get(&uri) else {
                // A change for a document we never saw a didOpen for — ignore
                // (the protocol guarantees open before change).
                return Ok(());
            };
            let mut text = doc.text.clone();
            for change in &p.content_changes {
                apply_content_change(&mut text, change);
            }
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

mod uri;
pub(crate) use uri::{path_to_uri, uri_to_path};

#[cfg(test)]
#[path = "uri_tests.rs"]
mod uri_tests;

#[cfg(test)]
#[path = "diagnostic_tests.rs"]
mod diagnostic_tests;

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
#[path = "server_tests.rs"]
mod server_tests;
