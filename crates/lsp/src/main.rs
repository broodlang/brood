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
    ResolveCompletionItem, SelectionRangeRequest, SemanticTokensFullRequest,
    SemanticTokensRangeRequest, SignatureHelpRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CodeActionParams, CodeActionProviderCapability, CompletionItem, CompletionOptions,
    CompletionParams, Diagnostic, DiagnosticSeverity, DiagnosticTag, DocumentChanges,
    DocumentFormattingParams, DocumentHighlightParams, DocumentLinkOptions, DocumentLinkParams,
    DocumentSymbolParams, FoldingRangeParams, FoldingRangeProviderCapability, GotoDefinitionParams,
    HoverParams, HoverProviderCapability, InlayHintParams, OneOf,
    OptionalVersionedTextDocumentIdentifier, PositionEncodingKind, PrepareRenameResponse,
    PublishDiagnosticsParams, Range, ReferenceParams, RenameOptions, RenameParams,
    SelectionRangeParams, SelectionRangeProviderCapability, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult, SemanticTokensServerCapabilities,
    ServerCapabilities, SignatureHelpOptions, SignatureHelpParams, TextDocumentEdit,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
    WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};

use brood::Interp;
use brood::core::value::Value;
use brood::syntax::scope::{BindingKind, Resolution};
use brood::syntax::{cst, reader, scope};
use brood::types::check::check_file;

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
    // stdio transport: the editor launches us and talks JSON-RPC over the pipe. Our own
    // framing rather than `Connection::stdio()`: that reader allocates whatever
    // `Content-Length` a client sends, so one huge header would abort the server with an
    // allocation failure. Ours refuses a frame past
    // `MAX_FRAME_BYTES` as a protocol error.
    let (connection, io_threads) = stdio_capped();

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
        // Clickable links over module names that resolve to a file —
        // `(:use foo)`/`(:alias foo)` clauses. No resolve step: each link carries
        // its target URI up front.
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
                // Range requests: an editor can classify just the visible viewport of a
                // large file (the whole-doc walk is cheap off the cached CST, then filtered).
                range: Some(true),
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

    // The initialize/initialized handshake, from which we read the two client
    // capabilities that change what we may send back (see [`ClientCaps`]).
    let init = connection.initialize(serde_json::to_value(capabilities)?)?;
    let caps = ClientCaps::from_initialize(init);
    // Run the loop, then drop `connection` *before* the join: its `Sender` keeps
    // the stdout writer thread alive, so the thread only sees its channel close
    // (and exits, letting `io_threads.join()` return) once this drop happens.
    // Skipping the drop would deadlock the join.
    let clean = main_loop(&connection, caps)?;
    drop(connection);

    io_threads.join()?;
    // The spec: a server that receives `exit` *without* a preceding `shutdown`
    // exits with code 1. It's how a supervising editor tells an orderly stop
    // from one it had to force — reporting 0 for both makes a forced kill look
    // like a clean handshake.
    if !clean {
        std::process::exit(1);
    }
    Ok(())
}

/// The client capabilities that change what the server is allowed to send back.
/// Both default to `false`, the conservative reading for a client that declared
/// nothing.
#[derive(Clone, Copy, Default)]
pub(crate) struct ClientCaps {
    /// The editor understands snippet syntax in completion items (`$0`,
    /// `${1:…}`), so the `(impl …)` op-completion may send a fillable skeleton.
    /// Without it we send a plain skeleton rather than literal `$`s.
    snippet: bool,
    /// The editor understands `WorkspaceEdit.documentChanges` — the *versioned*
    /// edit shape. It matters for rename: a bare `changes` map carries no
    /// document version, so a client applies it blind, and an edit computed
    /// against version N lands on a buffer at version N+k that the user kept
    /// typing into. With `documentChanges` each file's edits carry the version
    /// they were computed against and a stale one is rejected instead of
    /// corrupting the file.
    document_changes: bool,
}

impl ClientCaps {
    fn from_initialize(init: serde_json::Value) -> Self {
        let Ok(params) = serde_json::from_value::<lsp_types::InitializeParams>(init) else {
            return ClientCaps::default();
        };
        let snippet = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|td| {
                td.completion
                    .as_ref()?
                    .completion_item
                    .as_ref()?
                    .snippet_support
            })
            .unwrap_or(false);
        let document_changes = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.workspace_edit.as_ref()?.document_changes)
            .unwrap_or(false);
        ClientCaps {
            snippet,
            document_changes,
        }
    }
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

/// Run `f`, containing a panic instead of letting it kill the process.
///
/// The server is a long-lived host for a lot of analysis it doesn't own — the
/// CST walkers, the advisory checker, the Brood formatter — and it speaks over
/// stdio, so an unwind out of `main_loop` doesn't fail one request, it takes the
/// editor's entire language support down with no diagnostic beyond a line on a
/// stderr nobody reads. Same reasoning (and same shape) as the checker's own
/// containment in `types::check` — advisory work must not be able to tear down
/// its host. `AssertUnwindSafe` because `Interp`/`Documents` are `&mut`; the
/// state a panic could leave inconsistent is the interpreter's, and `check_file`
/// restores its own (roots, compile-ns, imports) on the panic path.
fn contain_panic<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(_) => {
            // The default hook has already printed the payload + location.
            eprintln!(
                "brood-lsp: internal error handling `{what}` — request abandoned, server continues"
            );
            None
        }
    }
}

/// Run the request loop until the client stops us. `Ok(true)` for an orderly
/// end (the `shutdown`/`exit` handshake, or the stream simply closing);
/// `Ok(false)` when `exit` arrived with no preceding `shutdown`, which the
/// caller reports as a nonzero exit status.
fn main_loop(
    connection: &Connection,
    caps: ClientCaps,
) -> Result<bool, Box<dyn Error + Sync + Send>> {
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
                    return Ok(true);
                }
                // A request MUST be answered even when the handler blows up —
                // an editor that never sees a response just wedges that feature.
                let (id, method) = (req.id.clone(), req.method.clone());
                let resp = contain_panic(&method, || handle_request(&docs, &mut interp, caps, req))
                    .unwrap_or_else(|| {
                        Response::new_err(
                            id,
                            ErrorCode::InternalError as i32,
                            format!("internal error handling {method}"),
                        )
                    });
                connection.sender.send(Message::Response(resp))?;
            }
            Message::Response(_) => {} // we issue no server→client requests yet
            // `exit` reaching the loop means it arrived on its own: a `shutdown`
            // first would have had `handle_shutdown` swallow the pair above.
            Message::Notification(notification)
                if notification.method == lsp_types::notification::Exit::METHOD =>
            {
                return Ok(false);
            }
            Message::Notification(notification) => {
                // A notification has no reply to salvage; contain and carry on,
                // so one poisonous buffer doesn't end the session. The `send`
                // failure inside still propagates (the client is gone).
                let method = notification.method.clone();
                if let Some(result) = contain_panic(&method, || {
                    handle_notification(
                        connection,
                        &mut docs,
                        &mut interp,
                        &mut bootstrapped,
                        notification,
                    )
                }) {
                    result?;
                }
            }
        }
    }
    // The stream closed under us (the editor went away) — not our doing, but
    // not an error either.
    Ok(true)
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
/// position, so the splice is panic-free on any degenerate range a client sends.
///
/// A range whose **end precedes its start** is normalized by swapping rather
/// than dropped. Dropping is the worse failure: the client believes the edit
/// landed, so our mirror silently diverges from the real buffer and *every*
/// later position — every hover, every rename range — is computed against text
/// the user isn't looking at. Splicing the span the client described, in the
/// order it makes sense, at least keeps the two in step.
fn apply_content_change(text: &mut String, change: &lsp_types::TextDocumentContentChangeEvent) {
    match change.range {
        Some(range) => {
            let idx = LineIndex::new(text);
            let a = idx.offset(text, range.start) as usize;
            let b = idx.offset(text, range.end) as usize;
            let (start, end) = (a.min(b), a.max(b));
            text.replace_range(start..end, &change.text);
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

impl Document {
    /// A document version with its analysis built once, here — every later request
    /// against this URI reads `analysis` rather than re-parsing the source.
    fn new(text: String, version: i32) -> Self {
        let analysis = analyze(&text);
        Document {
            text,
            analysis,
            version,
        }
    }

    /// The byte offset of an LSP `position` in this document.
    fn offset(&self, position: lsp_types::Position) -> u32 {
        self.analysis.line_index.offset(&self.text, position)
    }
}

/// Answer a request about one open document: decode its params, look up the document
/// `uri_of` names, and run `handle` on it. A document that is not open answers
/// `R::default()` — null or empty, the spec's "no information".
fn on_document<P, R>(
    request: Request,
    docs: &Documents,
    uri_of: fn(&P) -> &Uri,
    handle: impl FnOnce(&Document, P) -> R,
) -> Response
where
    P: serde::de::DeserializeOwned,
    R: serde::Serialize + Default,
{
    let (id, params) = match extract::<P>(request) {
        Ok(decoded) => decoded,
        Err(response) => return response,
    };
    let result = match docs.get(uri_of(&params)) {
        Some(doc) => handle(doc, params),
        None => R::default(),
    };
    Response::new_ok(id, result)
}

/// Dispatch a client request to its Tier-1 feature handler, producing the
/// response to send. An unknown method gets `MethodNotFound`; a request for a
/// document we don't have gets a null result (the spec's "no information").
fn handle_request(
    docs: &Documents,
    interp: &mut Interp,
    caps: ClientCaps,
    req: Request,
) -> Response {
    match req.method.as_str() {
        HoverRequest::METHOD => on_document(
            req,
            docs,
            |params: &HoverParams| &params.text_document_position_params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let offset = doc.offset(params.text_document_position_params.position);
                hover::hover(interp, &doc.text, &a.cst, &a.scope, &a.line_index, offset)
            },
        ),
        Completion::METHOD => on_document(
            req,
            docs,
            |params: &CompletionParams| &params.text_document_position.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let offset = doc.offset(params.text_document_position.position);
                Some(completion::completions(
                    interp,
                    &a.scope,
                    &a.cst,
                    &doc.text,
                    offset,
                    caps.snippet,
                ))
            },
        ),
        DocumentSymbolRequest::METHOD => on_document(
            req,
            docs,
            |params: &DocumentSymbolParams| &params.text_document.uri,
            |doc, _| {
                let a = &doc.analysis;
                Some(symbols::document_symbols(&a.cst, &doc.text, &a.line_index))
            },
        ),
        GotoDefinition::METHOD => on_document(
            req,
            docs,
            |params: &GotoDefinitionParams| &params.text_document_position_params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let position = params.text_document_position_params;
                definition::definition(
                    interp,
                    &position.text_document.uri,
                    &doc.text,
                    &a.cst,
                    &a.scope,
                    &a.line_index,
                    doc.offset(position.position),
                )
            },
        ),
        SelectionRangeRequest::METHOD => on_document(
            req,
            docs,
            |params: &SelectionRangeParams| &params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                Some(selection_range::selection_ranges(
                    &a.cst,
                    &doc.text,
                    &a.line_index,
                    &params.positions,
                ))
            },
        ),
        DocumentLinkRequest::METHOD => on_document(
            req,
            docs,
            |params: &DocumentLinkParams| &params.text_document.uri,
            |doc, _| {
                let a = &doc.analysis;
                document_link::document_links(interp, &doc.text, &a.cst, &a.line_index)
            },
        ),
        SignatureHelpRequest::METHOD => on_document(
            req,
            docs,
            |params: &SignatureHelpParams| &params.text_document_position_params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let offset = doc.offset(params.text_document_position_params.position);
                signature::signature_help(interp, &doc.text, &a.cst, &a.scope, offset)
            },
        ),
        // A local → single-file (its own scope). A global / free name → the whole project
        // (flat module model, ADR-019), via `workspace`.
        References::METHOD => on_document(
            req,
            docs,
            |params: &ReferenceParams| &params.text_document_position.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let uri = &params.text_document_position.text_document.uri;
                let offset = doc.offset(params.text_document_position.position);
                Some(match a.scope.resolve_at(&a.cst, &doc.text, offset) {
                    Resolution::Defined {
                        kind: BindingKind::Local,
                        ..
                    } => references::references(
                        uri,
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
                                workspace::references(interp, docs, uri, &name)
                            }
                            None => Vec::new(),
                        }
                    }
                    Resolution::NotASymbol => Vec::new(),
                })
            },
        ),
        DocumentHighlightRequest::METHOD => on_document(
            req,
            docs,
            |params: &DocumentHighlightParams| {
                &params.text_document_position_params.text_document.uri
            },
            |doc, params| {
                let a = &doc.analysis;
                let offset = doc.offset(params.text_document_position_params.position);
                Some(references::document_highlights(
                    &doc.text,
                    &a.cst,
                    &a.scope,
                    &a.line_index,
                    offset,
                ))
            },
        ),
        PrepareRenameRequest::METHOD => on_document(
            req,
            docs,
            |params: &TextDocumentPositionParams| &params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let offset = doc.offset(params.position);
                rename::prepare_rename(&doc.text, &a.cst, &a.scope, &a.line_index, offset)
                    .map(PrepareRenameResponse::Range)
            },
        ),
        // Local → single-file edit; global → a project-wide `WorkspaceEdit`. Whatever comes
        // back is sanitized + version-stamped before it goes on the wire (see
        // `finish_workspace_edit`) — a rename is the one request that rewrites user files.
        Rename::METHOD => on_document(
            req,
            docs,
            |params: &RenameParams| &params.text_document_position.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let uri = &params.text_document_position.text_document.uri;
                let offset = doc.offset(params.text_document_position.position);
                let edit = match a.scope.resolve_at(&a.cst, &doc.text, offset) {
                    Resolution::Defined {
                        kind: BindingKind::Local,
                        ..
                    } => rename::rename(
                        uri,
                        &doc.text,
                        &a.cst,
                        &a.scope,
                        &a.line_index,
                        offset,
                        &params.new_name,
                    ),
                    Resolution::Defined { .. } | Resolution::Free => {
                        match workspace::symbol_at(&a.cst, &doc.text, offset) {
                            Some(name) => {
                                let name = name.to_string();
                                workspace::rename(interp, docs, uri, &name, &params.new_name)
                            }
                            None => None,
                        }
                    }
                    Resolution::NotASymbol => None,
                };
                edit.map(|edit| finish_workspace_edit(docs, caps, edit))
            },
        ),
        SemanticTokensFullRequest::METHOD => on_document(
            req,
            docs,
            |params: &SemanticTokensParams| &params.text_document.uri,
            |doc, _| {
                let a = &doc.analysis;
                Some(SemanticTokensResult::Tokens(
                    semantic_tokens::semantic_tokens(&doc.text, &a.cst, &a.scope, &a.line_index),
                ))
            },
        ),
        SemanticTokensRangeRequest::METHOD => on_document(
            req,
            docs,
            |params: &SemanticTokensRangeParams| &params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                Some(SemanticTokensRangeResult::Tokens(
                    semantic_tokens::semantic_tokens_range(
                        &doc.text,
                        &a.cst,
                        &a.scope,
                        &a.line_index,
                        params.range,
                    ),
                ))
            },
        ),
        ResolveCompletionItem::METHOD => {
            let (id, item) = match extract::<CompletionItem>(req) {
                Ok(decoded) => decoded,
                Err(response) => return response,
            };
            Response::new_ok(id, completion::resolve(interp, item))
        }
        Formatting::METHOD => on_document(
            req,
            docs,
            |params: &DocumentFormattingParams| &params.text_document.uri,
            |doc, _| formatting::formatting(interp, &doc.text, &doc.analysis.line_index),
        ),
        WorkspaceSymbolRequest::METHOD => {
            let (id, params) = match extract::<WorkspaceSymbolParams>(req) {
                Ok(decoded) => decoded,
                Err(response) => return response,
            };
            let symbols = workspace_symbols::workspace_symbols(interp, docs, &params.query);
            Response::new_ok(id, WorkspaceSymbolResponse::Nested(symbols))
        }
        CodeActionRequest::METHOD => on_document(
            req,
            docs,
            |params: &CodeActionParams| &params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                // A diagnostic's range resolves to the byte offset of its start — where
                // the unbound name sits, for `names_in_scope`. The request's own range
                // too: "declare this signature" hangs off no diagnostic.
                let offset_of = |range: Range| doc.offset(range.start);
                code_actions::code_actions(
                    interp,
                    &params.text_document.uri,
                    &a.cst,
                    &doc.text,
                    &a.scope,
                    &a.line_index,
                    offset_of,
                    &params.context.diagnostics,
                    doc.offset(params.range.start),
                )
            },
        ),
        FoldingRangeRequest::METHOD => on_document(
            req,
            docs,
            |params: &FoldingRangeParams| &params.text_document.uri,
            |doc, _| {
                let a = &doc.analysis;
                Some(folding::folding_ranges(&a.cst, &doc.text, &a.line_index))
            },
        ),
        InlayHintRequest::METHOD => on_document(
            req,
            docs,
            |params: &InlayHintParams| &params.text_document.uri,
            |doc, params| {
                let a = &doc.analysis;
                let range = (doc.offset(params.range.start), doc.offset(params.range.end));
                Some(inlay_hints::inlay_hints(
                    interp,
                    &a.cst,
                    &doc.text,
                    &a.scope,
                    &a.line_index,
                    range,
                ))
            },
        ),
        // Nothing else is advertised: reply method-not-found rather than leave
        // the client waiting on a response.
        _ => Response::new_err(
            req.id,
            ErrorCode::MethodNotFound as i32,
            format!("unsupported request: {}", req.method),
        ),
    }
}

/// Make a [`WorkspaceEdit`] safe to hand an editor: sort each file's edits,
/// drop exact duplicates and any that would **overlap** a kept one, and — when
/// the client understands it — restamp the result as versioned
/// `documentChanges`.
///
/// Both halves guard against silent file corruption, which is the failure mode
/// a rename has and no other request does:
///
/// * **Overlap.** The spec says the edits for one document must not overlap, and
///   leaves the behaviour undefined when they do; clients variously apply both
///   (duplicating text), apply one, or reject the lot. Our cross-file rename
///   unions several independent scans — the qualified pass, the bare pass, and
///   one accessor cascade per record field — each deduping only within itself,
///   so two passes agreeing on a span is a shape the type system doesn't rule
///   out. Dropping the loser is always safe: the text it wanted to write is
///   already being written by the edit it overlaps.
/// * **Version.** A bare `changes` map says nothing about which document version
///   the ranges were computed against, so a client applies it to whatever the
///   buffer holds *now* — and a rename over a big project can easily take longer
///   than the user's next keystroke. `documentChanges` carries each file's
///   version (an unopened file gets `None`, which correctly means "don't
///   check"), so a stale edit is refused instead of landing on shifted text.
fn finish_workspace_edit(docs: &Documents, caps: ClientCaps, edit: WorkspaceEdit) -> WorkspaceEdit {
    let Some(changes) = edit.changes else {
        return edit;
    };
    let cleaned: HashMap<Uri, Vec<TextEdit>> = changes
        .into_iter()
        .map(|(uri, edits)| {
            let n = edits.len();
            let kept = non_overlapping(edits);
            if kept.len() != n {
                eprintln!(
                    "brood-lsp: dropped {} overlapping/duplicate edit(s) for {}",
                    n - kept.len(),
                    uri.as_str()
                );
            }
            (uri, kept)
        })
        .collect();
    if !caps.document_changes {
        return WorkspaceEdit {
            changes: Some(cleaned),
            ..edit
        };
    }
    let mut edits: Vec<TextDocumentEdit> = cleaned
        .into_iter()
        .map(|(uri, edits)| TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                version: docs.get(&uri).map(|d| d.version),
                uri,
            },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        })
        .collect();
    // A stable order so the same rename produces the same payload twice.
    edits.sort_by(|a, b| {
        a.text_document
            .uri
            .as_str()
            .cmp(b.text_document.uri.as_str())
    });
    WorkspaceEdit {
        changes: None,
        document_changes: Some(DocumentChanges::Edits(edits)),
        ..edit
    }
}

/// Sort `edits` by position and keep only a non-overlapping, duplicate-free
/// prefix-closed subset (first writer wins at any given span).
fn non_overlapping(mut edits: Vec<TextEdit>) -> Vec<TextEdit> {
    let key = |r: &Range| (r.start.line, r.start.character, r.end.line, r.end.character);
    edits.sort_by_key(|e| key(&e.range));
    let mut out: Vec<TextEdit> = Vec::with_capacity(edits.len());
    for e in edits {
        // An inverted range is not something we ever build; refuse to emit one.
        if (e.range.start.line, e.range.start.character) > (e.range.end.line, e.range.end.character)
        {
            continue;
        }
        let overlaps = out.last().is_some_and(|prev| {
            let (pl, pc) = (prev.range.end.line, prev.range.end.character);
            let (sl, sc) = (e.range.start.line, e.range.start.character);
            // Touching end-to-start is fine; strictly inside the previous edit
            // (or an exact repeat of a zero-width insert) is not.
            (sl, sc) < (pl, pc) || (prev.range == e.range)
        });
        if !overlaps {
            out.push(e);
        }
    }
    out
}

fn handle_notification(
    connection: &Connection,
    docs: &mut Documents,
    interp: &mut Interp,
    bootstrapped: &mut HashSet<PathBuf>,
    notification: ServerNotification,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    // Bad params must not tear down the connection: a malformed (or
    // unexpectedly-shaped) notification is logged and dropped, never fatal.
    // Only `send` failures below propagate — those mean the client is gone.
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let Some(opened) = params::<lsp_types::DidOpenTextDocumentParams>(notification) else {
                return Ok(());
            };
            let document = opened.text_document;
            let uri = document.uri;
            docs.insert(uri.clone(), Document::new(document.text, document.version));
            publish(connection, docs, interp, bootstrapped, &uri)?;
        }
        DidChangeTextDocument::METHOD => {
            let Some(changed) = params::<lsp_types::DidChangeTextDocumentParams>(notification)
            else {
                return Ok(());
            };
            // Incremental sync: start from the current buffer and apply each
            // content-change event in order (a ranged change splices its span; a
            // change with no range is a whole-document replace the client may still
            // send). Edits within one batch compound, so each resolves its range
            // against the text the prior edit produced. Re-parse ONCE after all
            // edits — incremental *sync* does not require incremental *parse*.
            let uri = changed.text_document.uri;
            let version = changed.text_document.version;
            let Some(doc) = docs.get(&uri) else {
                // A change for a document we never saw a didOpen for — ignore
                // (the protocol guarantees open before change).
                return Ok(());
            };
            let mut text = doc.text.clone();
            for change in &changed.content_changes {
                apply_content_change(&mut text, change);
            }
            docs.insert(uri.clone(), Document::new(text, version));
            publish(connection, docs, interp, bootstrapped, &uri)?;
        }
        DidCloseTextDocument::METHOD => {
            let Some(closed) = params::<lsp_types::DidCloseTextDocumentParams>(notification) else {
                return Ok(());
            };
            let uri = closed.text_document.uri;
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
            let Some(saved) = params::<lsp_types::DidSaveTextDocumentParams>(notification) else {
                return Ok(());
            };
            let uri = saved.text_document.uri;
            if let Some(path) = uri_to_path(&uri)
                && path.file_name().and_then(|n| n.to_str()) == Some("project.blsp")
                && let Some(root) = path.parent()
            {
                bootstrapped.remove(root);
            } else {
                // Any other save: the image this server checks against loaded the project
                // once, so a module saved since would otherwise be checked by its OLD
                // types in every file that uses it — the editor and `nest check`
                // disagreeing about the same tree. Reload what moved on disk.
                refresh_project(interp, bootstrapped, &uri);
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
fn params<P: serde::de::DeserializeOwned>(notification: ServerNotification) -> Option<P> {
    let method = notification.method.clone();
    match notification.extract::<P>(&method) {
        Ok(decoded) => Some(decoded),
        Err(error) => {
            eprintln!("brood-lsp: ignoring malformed `{method}`: {error:?}");
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

/// The project root above `file_path` — the nearest directory holding a `project.blsp` —
/// asked of `project/find-root`, the walk `nest` uses. `None` outside a Brood project.
fn find_project_root(interp: &mut Interp, file_path: &Path) -> Option<PathBuf> {
    let directory = file_path.parent()?.to_string_lossy().into_owned();
    let form = brood::introspect::call_form("project/find-root", &[&directory]);
    let checkpoint = interp.heap.checkpoint();
    let root = match interp.eval_str(&form) {
        Ok(Value::Str(id)) => Some(PathBuf::from(interp.heap.string(id).to_string())),
        _ => None,
    };
    interp.heap.reset_local_to(checkpoint);
    root
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
    let Some(root) = find_project_root(interp, &file_path) else {
        return;
    };
    if bootstrapped.contains(&root) {
        return;
    }
    // Load the project image for tooling through the shared seam, so the LSP
    // and `nest mcp` can't drift on which frameworks a tooling image carries.
    // Policy lives in Brood (`setup-tooling-image`).
    if let Err(error) = brood::introspect::load_tooling_image(interp, &root.display().to_string()) {
        eprintln!(
            "brood-lsp: project bootstrap failed for {}: {error}",
            root.display()
        );
    }
    // Mark bootstrapped regardless of success — a partial load is consistent
    // (each top-level form's `eval_str` is checkpointed), and re-running on
    // every publish would re-load every source on every keystroke.
    bootstrapped.insert(root);
}

/// Bring a bootstrapped project's image up to date with the files on disk
/// (`project/reload-changed`: a `stat` per source file, and a reload of each one whose mtime
/// moved since the image loaded it). A no-op outside a bootstrapped project. What the
/// reloaded files print goes to stderr — stdout is this server's protocol channel.
fn refresh_project(interp: &mut Interp, bootstrapped: &HashSet<PathBuf>, uri: &Uri) {
    let Some(path) = uri_to_path(uri) else {
        return;
    };
    let Some(root) = bootstrapped.iter().find(|root| path.starts_with(root)) else {
        return;
    };
    let code = format!(
        "(binding (*out* *err*) {})",
        brood::introspect::call_form("project/reload-changed", &[&root.display().to_string()])
    );
    if let Err(error) = interp.eval_str(&code) {
        eprintln!(
            "brood-lsp: refreshing {} after a save failed: {error}",
            root.display()
        );
    }
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
        let syntax_reported = !lsp_diags.is_empty();
        lsp_diags.extend(typecheck_diagnostics(
            interp,
            text,
            cst_root,
            index,
            syntax_reported,
        ));
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
///
/// When the READER refuses the text, there is nothing to check — and that refusal is
/// itself the diagnostic: the CST is deliberately more tolerant than the reader (a
/// stray `#'`, a `\c` escape, an odd-count map all parse as tooling nodes). The reader's
/// error is published as an `ERROR` at its position — unless `syntax_reported`, i.e. the
/// Tier-0 CST pass already flagged the text, in which case it is the same fault twice.
fn typecheck_diagnostics(
    interp: &mut Interp,
    text: &str,
    cst_root: &cst::Node,
    index: &LineIndex,
    syntax_reported: bool,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let checkpoint = interp.heap.checkpoint();
    let read = reader::read_all_positioned(&mut interp.heap, text);
    if let Err(error) = &read
        && !syntax_reported
    {
        let offset = error
            .pos
            .map(|pos| index.offset_of_char_pos(text, pos))
            .unwrap_or(0);
        let range = Range::new(
            index.position(text, offset),
            index.position(text, index.next_char(text, offset)),
        );
        let mut diag = Diagnostic::new_simple(range, format!("cannot read: {}", error.message));
        diag.severity = Some(DiagnosticSeverity::ERROR);
        diag.source = Some("brood".to_string());
        out.push(diag);
    }
    if let Ok(positioned) = read {
        let forms: Vec<Value> = positioned.into_iter().map(|(f, _)| f).collect();
        for (position, message) in check_file(&mut interp.heap, &forms) {
            // The checker reports a reader `Pos`: 1-based line, 1-based **character**
            // column. LSP wants 0-based line + UTF-16 column, so the projection goes
            // `Pos → byte offset → Position` through the `LineIndex` — never `Pos.col`
            // used *as* a `character`, which drifts by one per astral char.
            //
            // A finding with no position (a file-level one, like an unused `:use`) is
            // anchored at the start of the document rather than dropped: the CLI prints it.
            let range = match position {
                Some(position) => {
                    let offset = index.offset_of_char_pos(text, position);
                    // Refine from the form start to the *offending token* where the CST
                    // shows it — else a one-character marker the editor widens.
                    refine_diagnostic_range(cst_root, text, index, offset, &message).unwrap_or_else(
                        || {
                            Range::new(
                                index.position(text, offset),
                                index.position(text, index.next_char(text, offset)),
                            )
                        },
                    )
                }
                None => Range::new(index.position(text, 0), index.position(text, 0)),
            };
            // A deprecation is carried as a TAG as well as a warning, so the client
            // strikes the name through (ADR-283 keeps it advisory: severity WARNING).
            // Recognised by the checker's wording (`types/check/walk.rs` `stability_msg`),
            // pinned by `a_deprecated_call_is_tagged_deprecated_not_only_warned`.
            let deprecated = message.contains(" is deprecated since ");
            let mut diag = Diagnostic::new_simple(range, message);
            diag.severity = Some(DiagnosticSeverity::WARNING);
            diag.source = Some("brood".to_string());
            if deprecated {
                diag.tags = Some(vec![DiagnosticTag::DEPRECATED]);
            }
            out.push(diag);
        }
    }
    interp.heap.reset_local_to(checkpoint);
    out
}

/// Tighten a checker finding's squiggle from the whole form to the token it's
/// really about. For `unbound symbol: NAME`, the first matching symbol token in
/// the form; otherwise the form's operator (arity / type-misuse are about the
/// call head). `None` if neither is found — the caller uses a 1-char marker.
/// `off` is the finding's **byte** offset (the caller projected the reader `Pos`
/// through the line index), so this never re-does column arithmetic.
fn refine_diagnostic_range(
    root: &cst::Node,
    text: &str,
    index: &LineIndex,
    off: u32,
    msg: &str,
) -> Option<Range> {
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
    let notification = ServerNotification::new(PublishDiagnostics::METHOD.to_string(), params);
    connection
        .sender
        .send(Message::Notification(notification))?;
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

/// Robustness regressions: the protocol/position failures that are fatal or
/// silent rather than merely wrong. See the module docs.
#[cfg(test)]
#[path = "robustness_tests.rs"]
mod robustness_tests;

/// The largest JSON-RPC frame the server will read. A whole-document `didOpen` of a
/// large file is a few megabytes; 64 MiB is far past any editor's traffic and small
/// enough that a bad header cannot take the process down.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// The reader and writer threads behind [`stdio_capped`]; `join` mirrors
/// `lsp_server::IoThreads::join`.
struct CappedIoThreads {
    reader: std::thread::JoinHandle<std::io::Result<()>>,
    writer: std::thread::JoinHandle<std::io::Result<()>>,
}

impl CappedIoThreads {
    fn join(self) -> std::io::Result<()> {
        match self.reader.join() {
            Ok(r) => r?,
            Err(err) => std::panic::panic_any(err),
        }
        match self.writer.join() {
            Ok(r) => r,
            Err(err) => std::panic::panic_any(err),
        }
    }
}

/// `Connection::stdio()` with a bounded frame size — same channels, same message
/// type, same exit rule (the reader stops after an `exit` notification), but a
/// `Content-Length` past [`MAX_FRAME_BYTES`], negative, absent or unparsable is a
/// protocol error instead of an allocation the size of the lie.
fn stdio_capped() -> (Connection, CappedIoThreads) {
    let (writer_sender, writer_receiver) = crossbeam_channel::bounded::<Message>(0);
    let (reader_sender, reader_receiver) = crossbeam_channel::bounded::<Message>(0);
    let writer = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        for msg in writer_receiver {
            msg.write(&mut stdout)?;
        }
        Ok(())
    });
    let reader = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        loop {
            // One unparseable message must not end the session: the server died on it,
            // and the editor lost its language server for one bad frame.
            let msg = match read_frame_capped(&mut stdin) {
                Ok(Some(msg)) => msg,
                Ok(None) => break,
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                    eprintln!("brood-lsp: skipped a message: {error}");
                    continue;
                }
                Err(error) => return Err(error),
            };
            let is_exit = matches!(&msg, Message::Notification(n) if n.method == "exit");
            if reader_sender.send(msg).is_err() {
                break;
            }
            if is_exit {
                break;
            }
        }
        Ok(())
    });
    (
        Connection {
            sender: writer_sender,
            receiver: reader_receiver,
        },
        CappedIoThreads { reader, writer },
    )
}

/// Read one `Content-Length`-framed JSON-RPC message, or `None` at a clean EOF.
fn read_frame_capped(input: &mut impl std::io::BufRead) -> std::io::Result<Option<Message>> {
    use std::io::{Error, ErrorKind};
    let invalid = |what: String| Error::new(ErrorKind::InvalidData, what);
    let mut size: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return if size.is_none() {
                Ok(None)
            } else {
                Err(invalid("headers without a body".to_string()))
            };
        }
        // Header lines are ASCII and small; a header line past the frame cap is itself
        // a hostile client.
        if line.len() > 8192 {
            return Err(invalid("header line too long".to_string()));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(": ") else {
            return Err(invalid(format!("malformed header: {trimmed:?}")));
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            let n: usize = value
                .trim()
                .parse()
                .map_err(|_| invalid(format!("bad Content-Length: {value:?}")))?;
            if n > MAX_FRAME_BYTES {
                return Err(invalid(format!(
                    "Content-Length {n} exceeds the {MAX_FRAME_BYTES}-byte frame limit"
                )));
            }
            size = Some(n);
        }
    }
    let Some(size) = size else {
        return Err(invalid("no Content-Length header".to_string()));
    };
    let mut body = vec![0u8; size];
    input.read_exact(&mut body)?;
    // A body that is not JSON-RPC is `InvalidInput`, not `InvalidData`: its frame was read to
    // its exact length, so the stream is still in step and the next frame reads — the reader
    // skips it. A broken header (`InvalidData`) leaves the stream unreadable from here.
    let msg: Message = serde_json::from_slice(&body).map_err(|e| {
        Error::new(
            ErrorKind::InvalidInput,
            format!("malformed JSON-RPC message: {e}"),
        )
    })?;
    Ok(Some(msg))
}

#[cfg(test)]
mod frame_cap_tests {
    //! The stdio transport refuses a frame past `MAX_FRAME_BYTES` as a protocol error
    //! — `lsp_server::Connection::stdio()` allocates whatever `Content-Length` the
    //! client sends, so one header would abort the server.
    use super::*;
    use std::io::Cursor;

    fn framed(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
    }

    #[test]
    fn a_well_formed_frame_reads_as_a_message() {
        let mut input = Cursor::new(framed(r#"{"jsonrpc":"2.0","id":1,"method":"shutdown"}"#));
        let msg = read_frame_capped(&mut input)
            .expect("read")
            .expect("a message");
        assert!(matches!(msg, Message::Request(r) if r.method == "shutdown"));
        assert!(read_frame_capped(&mut input).expect("clean EOF").is_none());
    }

    #[test]
    fn a_malformed_body_is_skippable_and_the_next_frame_reads() {
        // The server died on one unparseable message (found by driving brood-lsp with
        // hostile input): the body is read to its exact length, so the stream is in step.
        let mut bytes = framed("{not json at all");
        bytes.extend(framed(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#));
        let mut input = Cursor::new(bytes);
        let err = read_frame_capped(&mut input).expect_err("not JSON-RPC");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput, "{err}");
        let msg = read_frame_capped(&mut input)
            .expect("the next frame reads")
            .expect("a message");
        assert!(matches!(msg, Message::Request(r) if r.method == "shutdown"));
        // A broken header stays fatal: the stream is unreadable after it.
        let mut input = Cursor::new(b"Content-Length: nope\r\n\r\n{}".to_vec());
        let err = read_frame_capped(&mut input).expect_err("refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
    }

    #[test]
    fn a_content_length_past_the_cap_is_a_protocol_error_not_an_allocation() {
        let mut input = Cursor::new(b"Content-Length: 99999999999\r\n\r\n{}".to_vec());
        let err = read_frame_capped(&mut input).expect_err("refused");
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn a_missing_negative_or_unparsable_length_is_refused() {
        for header in [
            "X: 1\r\n\r\n{}",
            "Content-Length: -5\r\n\r\n{}",
            "Content-Length: abc\r\n\r\n{}",
        ] {
            let mut input = Cursor::new(header.as_bytes().to_vec());
            assert!(read_frame_capped(&mut input).is_err(), "{header:?}");
        }
    }

    #[test]
    fn a_body_past_the_cap_is_refused_before_it_is_read() {
        // Exactly one byte over: the cap is on the declared length, not on what arrives.
        let header = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1);
        let mut input = Cursor::new(header.into_bytes());
        assert!(read_frame_capped(&mut input).is_err());
    }
}
