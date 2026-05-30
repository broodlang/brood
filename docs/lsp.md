# Language server (design)

A Language Server Protocol (LSP) server for Brood, shipped as a separate binary
(`brood-lsp`) in its own workspace crate. This is the cross-editor generalisation
of the editor contract in [`tooling.md`](tooling.md): instead of every editor
re-implementing "run the file and parse the GNU error lines", they speak LSP to
one server that owns the language knowledge.

> Status: **Tiers 0–2 live, plus a "developer ergonomics" pass** —
> diagnostics (syntactic + semantic), completion, hover, signature help, document
> symbols, goto-definition (in-file, cross-module, stdlib, and `require`-target),
> references, document-highlight, rename, semantic tokens, **document formatting,
> workspace symbol search, code actions, folding ranges, and inlay hints**.
> Recorded as
> [ADR-025](decisions.md#adr-025--a-lossless-span-carrying-cst-for-tooling-separate-from-the-eval-value);
> this document is the full plan it points to (the `types.md` ↔ ADR-024 pattern).
> **Done:** Foundation A — the CST (`syntax::cst`) + shared lexical rules
> (`syntax::atom`) + `error::Span`; Foundation B — the CST scope resolver
> (`syntax::scope`); Foundation C — leading-string docstrings and the
> introspection primitives `doc` / `arglist` / `global-names` / `bound?`.
> **Tier 0** — the `crates/lsp` → `brood-lsp` binary: stdio lifecycle, full
> document sync, and syntactic `publishDiagnostics` read off the CST
> (`lsp-server` + `lsp-types`, no async runtime). `Uri`→text document store, a
> `LineIndex` for byte↔UTF-16 `Position` (both directions), and
> `diagnostics::collect` over the CST's `Error` nodes.
> **Tier 1** (complete) — `textDocument/{completion,hover,documentSymbol,
> definition,signatureHelp}`, wired to the Foundation B/C surface: completion
> offers locals-in-scope (`scope::names_in_scope`) + interpreter globals
> (`global-names`); hover renders a local note, a document def's
> signature+docstring (read off the CST, `defs`), or a prelude/builtin name's
> `arglist`+`doc`; `documentSymbol` outlines top-level `def`/`defn`/`defmacro`;
> goto-definition resolves through `scope::resolve_at`; signature help shows the
> enclosing call's parameters with the active argument highlighted (params from
> the CST def, or `arglist` for a prelude/builtin). The server holds one `Interp`
> for introspection only — it still never evaluates the open buffer.
> Hover/`doc`/`arglist` now cover **primitives** too (the `NativeFn` carries a
> docstring + param names from the `PRIMITIVE_DOCS` table), and the public stdlib
> carries leading-string docstrings — so a hover shows real documentation across
> the surface.
> **Beyond Tier 1 (also done):**
> • **Semantic diagnostics** — `publish` runs the advisory checker
> (`types::check::check_file`) over the positioned forms and emits its
> unbound-name / arity / type-misuse findings as `WARNING`s (located; a 1-char
> marker at the form). It bootstraps the enclosing project first through the
> shared tooling-image seam (`introspect::load_tooling_image` →
> `std/project.blsp`'s `setup-tooling-image`: `project-setup` +
> `project-load-sources` + `require 'test` + `require 'format`), so cross-module
> names and the test/format-framework macros resolve and don't false-positive as
> unbound. `nest mcp` boots through the *same* Brood function, so the two servers
> can't drift on what a tooling image contains.
> • **Cross-file goto-definition** — the §Cross-file hybrid is wired: a name that
> resolves `Free` in the buffer falls back to `(source-location 'name)` against
> the bootstrapped image, including **into the standard library** (the prelude is
> materialized to a cache file so its def sites are openable — see §Cross-file).
> **Tier 2 (also done):** find-references and document-highlight (both off
> `scope::references`), rename with `prepareRename` (single-file
> `WorkspaceEdit`, new name validated through the atom classifier), and
> whole-document semantic tokens. Completion gained the special forms + lazy
> `completionItem/resolve` (signature + docstring), diagnostics now carry the
> document version, and an unbound-symbol squiggle narrows to the offending token
> (`refine_diagnostic_range`).
> **Cross-file references & rename (also done):** `references_to_global` over
> every `project_files` entry (`workspace.rs`); rename emits a multi-file
> `WorkspaceEdit`. Locals stay single-file; no project → just the open buffer.
> The same engine is exposed to agents as the MCP **`callers`** tool, via the
> pure `(references-in-source name src)` primitive (docs/mcp.md).
> **Developer-ergonomics pass (also done):**
> • **`textDocument/formatting`** (`formatting.rs`) — whole-document reformat,
> delegated to the Brood formatter (`std/format.blsp`) via
> `introspect::format_source`. One full-document `TextEdit`; `None` on a parse
> error (don't mangle an un-parseable buffer) or when already canonical. Honors
> "policy in Brood" — the server only transports. No range/onType (the formatter
> works on whole files).
> • **`workspace/symbol`** (`workspace_symbols.rs`) — project-wide symbol search
> over every file's top-level `def`/`defn`/`defmacro` (reusing `defs::top_level`
> and a new `workspace::all_sources` that unions project files + every open
> buffer). Case-insensitive **subsequence** matching (`fs` → `format-source`);
> empty query lists all.
> • **`textDocument/codeAction`** (`code_actions.rs`) — quick-fixes off published
> diagnostics. Today: **"did you mean?"** for `unbound symbol: X` — Levenshtein
> against locals-in-scope + special forms + globals, within a length-relative
> threshold, top-3 nearest, edited onto the diagnostic's (already token-narrowed)
> range. Marked `isPreferred`.
> • **`textDocument/foldingRange`** (`folding.rs`) — collapsible regions off the
> CST: every multi-line container (`()`/`[]`/`{}`) and every run of consecutive
> comment lines. Pure structural walk, no eval.
> • **`textDocument/inlayHint`** (`inlay_hints.rs`) — parameter-name hints at
> call sites from `arglist` (the signature-help source). Conservative: only the
> **leading required** params (stops at the first `&optional`/`&` marker, since
> `arglist` drops `(opt default)` groups); a head resolving to a **local** is
> skipped; per-name `arglist` memoized per request; range-scoped to the visible
> region.
> **Still next:** incremental document sync; range / delta semantic-token
> requests; finer spans for arity/type findings (wants spans threaded through the
> checker, not just the call operator); and more code actions (remove unused
> `require`, create-missing-`defn`).

## Why a server, and why not brute-force it

The temptation is to bolt one feature at a time onto the existing reader:
positions for diagnostics here, an `arglist` lookup there, a symbol scan for
completion. That path duplicates a parser's worth of position bookkeeping across
features and never quite agrees with itself. The cheaper foundation is to decide
**once** how source text maps to *spans* and to *meaning*, then let every
feature read off that. Two substrate decisions carry the whole server:

1. **A lossless, span-carrying CST**, separate from the evaluation `Value`
   (below). This answers "what is at this cursor / in this range?" — the
   question under hover, go-to-definition, completion context, semantic tokens,
   and rename.
2. **Reuse the analysis we already have.** Syntactic diagnostics fall out of the
   CST; semantic diagnostics come from the advisory checker (ADR-024), *not*
   from evaluating the user's buffer. The server never runs user code.

Everything else (transport, the per-document store, capability wiring) is
well-trodden plumbing handled by off-the-shelf crates.

## Architecture

```
   editor ⇄ JSON-RPC/stdio ⇄  brood-lsp (crates/lsp)         brood (crates/lisp, the lib)
                               ┌──────────────────────┐      ┌───────────────────────────┐
   didOpen/didChange ───────▶  │ document store        │      │ syntax::cst::parse(&str)  │
                               │  (text + parsed CST)  │─────▶│   → lossless span tree    │
   publishDiagnostics ◀──────  │ LineIndex (utf-16)    │      │ types::check (advisory)   │
   hover/completion/... ◀────▶ │ feature handlers      │─────▶│ introspection primitives  │
                               └──────────────────────┘      └───────────────────────────┘
```

The server holds, per open document, the source text, its parsed CST, and a
`LineIndex`. It re-parses on each change (full-document sync to start with — the
reader is fast enough that incremental sync is premature). It owns one `Interp`
for introspection queries (`arglist`, global names); it does **not** evaluate
document text.

## The load-bearing decision: a separate CST

The evaluation `Value` cannot carry per-occurrence source positions, and
shouldn't be made to:

- Symbols are `Value::Sym(u32)` — `Copy`, interned, deduplicated, **not
  heap-addressed**. The same `foo` at line 4 and line 9 is *one* value. The
  existing `form-pos` side-table is keyed by a heap pair-index, so it can only
  position **list** forms, start-only — never "the symbol token under the
  cursor", which is what most LSP features need.
- Bolting positions onto `Value` (boxing symbols, wrapping every read node)
  would tax every evaluation forever to serve tooling. The eval tree must stay
  lean; the tail-call loop and the `Copy` value model are load-bearing.

So tooling gets its **own** tree. The two parsers have genuinely different
contracts, which is why they are different functions rather than one shared one:

| | `syntax::reader` (eval) | `syntax::cst` (tooling) |
|---|---|---|
| Output | `Value` (heap-allocated, executable) | owned `Node` tree (heap-free) |
| Malformed input | **rejects** with a precise `LispError` | **tolerates**: emits `Error` nodes and continues |
| Trivia (whitespace, comments) | discarded | **kept** (the tree is lossless / round-trippable) |
| Quote sugar `'` `` ` `` `~` `~@` | lowered to `(quote x)` … | kept *as written* (a `Quote` node wrapping its target) |
| Spans | top-level form starts only (`form-pos`) | every node, start..end |

An LSP must parse a half-typed buffer on every keystroke; the evaluator must
refuse to run one. That difference is the justification for two parsers — not
accidental duplication. They **share the lexical rules** (`is_delimiter`, atom
classification, the string-escape table); those helpers should be factored so
the two cannot drift on what a token *is*.

### The node model

Heap-free and owned, so a server can hold many documents cheaply and move them
between threads. Tokens carry only their kind and span; a consumer slices
`&src[span]` and reuses `classify` / the escape table when it needs the decoded
value. (See [the sketch](#sketch-parse_cst) below.)

```rust
// crates/lisp/src/syntax/cst.rs
pub enum NodeKind {
    Root,
    List, Vector,                    // ( … )  and  [ … ]
    Quote, Quasi, Unquote, Splice,   // ' ` ~ ~@  — kept as written, not lowered
    Symbol, Keyword, Int, Float, Str, Bool, Nil,
    Whitespace, Comment,             // trivia — present so the tree is lossless
    Error,                           // an unparseable run; parsing resumes after it
}

pub struct Node {
    pub kind: NodeKind,
    pub span: Span,                  // byte offsets into the source, half-open
    pub children: Vec<Node>,         // empty for token nodes
}
```

`Span { start: u32, end: u32 }` lives in [`error.rs`](../crates/lisp/src/error.rs)
beside `Pos`: `Pos` is the 1-based line/col *projection* of a byte offset, used
for the GNU diagnostics today; `Span` is the raw byte range the CST records.

### Error recovery (always returns a tree)

`parse` is total. The recovery rules are deliberately boring:

- **Unmatched close** `)` `]` `}` → an `Error` token for that char; continue.
- **Unclosed open** at EOF → close the `List`/`Vector` at EOF (its span runs to
  end-of-input) so its children stay navigable; mark it for a "unclosed"
  diagnostic.
- **Unterminated string / bad escape** → a `Str` node spanning to EOL/EOF tagged
  as recovered.

These are exactly the situations a buffer is in *while you type*, so navigation
and completion keep working through them.

### "What is under the cursor?"

```rust
impl Node {
    /// The innermost node whose span contains `offset` (a byte offset).
    /// Drives hover / goto / completion-context / semantic tokens.
    pub fn node_at(&self, offset: u32) -> Option<&Node>;
}
```

Most features start here: find the node at the cursor; if it's a `Symbol`,
resolve it (below).

## Positions: bytes ↔ LSP `Position`

LSP `Position` is `{ line, character }` where **`character` is a UTF-16 code-unit
offset by default** (not bytes, not Unicode scalar values). The CST records byte
offsets; the existing `Pos` counts *characters*. Neither is UTF-16. So the
server owns a `LineIndex` that maps byte offset ↔ `Position` with UTF-16 column
arithmetic. Flagged here so we build it once, correctly, rather than discovering
multibyte off-by-N bugs feature by feature. (We can negotiate UTF-8 positions
via `positionEncoding` in `initialize` if the client supports it, which makes
the map trivial — but the UTF-16 fallback must exist.)

## Diagnostics: two sources, never by evaluating

**Syntactic** (always available, mid-edit): walk the CST for `Error` nodes and
unclosed-delimiter markers → `Diagnostic`s. This needs no evaluation and works
on buffers that can't yet run.

**Semantic** (unbound symbol, arity, provable type misuse): from the **advisory
checker** ([`types::check`](../crates/lisp/src/types/check.rs), ADR-024), which
is *designed* to analyse without executing and to never reject. The server must
**not** call `eval` on document text — that would run side effects and could
loop forever.

Two honest gaps to close as increments, not now:

- The checker currently returns `Vec<String>` — **un-located** messages. To
  surface them as diagnostics it must carry spans. It runs over *macro-expanded*
  forms, where original spans are already gone (the same macro caveat
  `tooling.md` notes for runtime-error positions). The principled fix is to
  check the **un-expanded** form and attribute findings to CST spans, accepting
  that we don't see *into* macro-generated code at first.
- Unbound/arity checks aren't in the advisory checker yet (it only flags
  primitive type misuse). A name-resolution pass over the CST (next section)
  gives "unbound symbol" cheaply and safely.

## Resolution, scopes, and introspection

Go-to-definition, references, rename, and "unbound" all need to know **what a
symbol binds to**. Two layers:

- **Globals** — enumerable from the runtime global table. Add small primitives
  (Rust *mechanism*; the policy that consumes them can be Brood):
  - `(arglist f)` — the parameter list of a closure (`Closure` already stores
    `params` / `optionals` / `rest`) or a builtin (from its `Arity`). Feeds
    signature help and hover.
  - `(global-names)` / `(bound? sym)` — for completion and workspace symbols.
  - `(doc f)` — **implemented** (ADR-025): a docstring is an optional leading
    string in a `fn`/`defn`/`defmacro` body (only when more body follows it),
    stored on the closure. A module documents itself the same way — a leading
    string as the file's first top-level form. `nest doc` extracts both as
    Markdown by loading + introspecting (see `docs/tooling.md`).
- **Locals** — a scope walk over the CST tracking binders: `def`/`defn`/
  `defmacro`, `let`/`let*`, `fn`/`lambda` params (including `&optional` and
  `& rest`), and `match`/`fn`-clause patterns. This is pure CST analysis, no
  heap. It should be **shared with the checker's own scope tracking** so scope
  resolution isn't written twice.

## Cross-file resolution: an image query, not a static index (ADR-031)

Everything above is **single-file**. The server's knowledge of names has exactly
two sources, and neither crosses a `require`:

- the **open buffer's** CST + scope tree (locals and this file's `def`s), and
- the **interpreter's globals** — the *prelude + Rust builtins* only. The server
  deliberately never evaluates the buffer (no side effects, no loops), so it also
  never runs a `(require 'foo)`; a symbol another module `provide`s is invisible
  to it. Today such a name resolves as `Free` (no goto target, no hover, but no
  false "unbound" error either — diagnostics are syntactic-only at Tier 1).

The tempting fix is a **static workspace-indexer** (rust-analyzer's model: walk
the `require` graph off `*load-path*`, parse every file's CST, never run
anything). We **reject that as the primary path** (ADR-031). Brood is an
image-based, hot-reloadable Lisp (ADR-013), and the endgame is an editor that
*is* a running Brood image editing Brood — so the running runtime already holds
every loaded module's globals (it's what `global-names` enumerates). A static
index just re-derives, approximately, what the image already knows for certain —
and can't follow computed/conditional `require`s or see through macros.

So cross-file is the **SLIME/CIDER/Emacs-xref model**: the image recorded *where
each thing was defined as it loaded*, and `M-.` is a lookup against it. The plan
(**all four steps done**):

1. ✅ **Record def sites at load/`def` time** — `name → (file, pos)` into the
   shared, mutable `RuntimeCode` region (so a redefinition updates it and spawned
   processes see it, per ADR-013). `file` is the existing `current-file`; `pos`
   the form's start. **Span-accurate through macros for *definitions***, because
   the site is captured before macroexpansion (ADR-022) discards spans. (The file
   loaders — the `load` builtin and `eval_source` — call `Heap::note_definition`
   on each un-expanded top-level form. *Top-level only* for now; a `def` nested in
   a `do` isn't recorded yet.)
2. ✅ **`(source-location 'foo) → [file line col]`** (or nil) — one Rust
   primitive; policy on top is Brood. Already useful standalone (error provenance,
   `nest`, a self-hosted REPL `M-.`) before any LSP wiring consumes it.
3. ✅ **Stay a hybrid:** the live (half-typed) buffer keeps using the CST + scope
   walker; a name that resolves `Free` there falls back to `source-location`,
   yielding a cross-file `Location` (LSP `Location` already carries a `Uri`).
   (`definition::definition` — `Resolution::Free` → `introspect::source_location`
   → `path_to_uri`. Works once the project is bootstrapped, which the first
   `didOpen` under a `project.blsp` arranges.)
4. **Definitions go image-based; references stay static** — references through
   macro-generated code have no faithful spans, so "find references" remains
   CST-level source occurrences aggregated across files; "go to definition"
   becomes the name→site lookup.

**Navigating to a `require`'d module.** Goto-definition on the module name in
`(require 'foo)` doesn't go through the def-site table (the feature name binds
nothing); instead `definition.rs` detects the `require` call context (an
enclosing `List` whose head is `require`) and resolves the name with
`introspect::module_file` — `require--find "foo.blsp" *load-path*`, the same
lookup `require` itself uses, against the bootstrapped project's load-path. It
lands at the top of the module's file. (A baked-in std module — `%builtin-module`
source, no file — has no target.)

**Navigating into the standard library.** The prelude is `include_str!`'d, so it
has no source file at runtime — `M-.` on `map` would have nowhere to land. The
prelude build therefore *materializes* a copy to `$XDG_CACHE_HOME/brood/prelude.blsp`
(falling back to `~/.cache`), sets `current-file` to it, reads positioned, and
records each prelude def's site there. These sites are immutable, so they live in
the shared `SharedCode` region (not per-runtime `RuntimeCode`); `Heap::def_site`
consults the runtime table first (a user redefinition wins) then the prelude. The
cache copy is rewritten only when a new build's embedded prelude differs. Builtins
implemented in Rust (`cons`, `rem`, …) have no Brood source and remain `nil` —
hover still documents them via `PRIMITIVE_DOCS`. Best-effort: if the cache can't
be written, stdlib goto is simply unavailable and nothing else is affected.

The cost is a **loaded image**: cross-file answers require the project to have
been *run* (top-level side effects on load) — the line this doc draws at Tier
0–1. That's a deliberate, opt-in step (the server owns a project image it loads
explicitly, or talks to a running one), gated so the safe single-file features
never depend on it; and the image can be **stale** between an edit and a reload
(SLIME's `C-c C-c` workflow), which the CST covers for the current buffer. The
static indexer survives only as the *fallback* when no image is available.

## The crate

`crates/lsp` → a `brood-lsp` binary depending on the `brood` lib. This mirrors
the existing `crates/lisp` + `crates/cli` + `crates/nest` split (and the planned
`crates/editor` / `crates/server`); a loose top-level `tools/` dir would break
that pattern for no gain.

**Protocol crates:** `lsp-server` + `lsp-types` (the synchronous stack
rust-analyzer uses) rather than `tower-lsp`. Reasons:

- `Interp`/`Heap` is single-threaded-per-process and not `Sync`. A synchronous
  request loop owning the document store + one `Interp` sidesteps all `Send` /
  `Sync` friction.
- `tower-lsp` drags in `tokio`; nothing else in the tree uses an async runtime,
  and a server fielding one editor's requests doesn't need one. (Per ADR-014,
  crates are welcome where they remove real complexity — this is the binary, not
  Lisp-callable behaviour, so the bar is just "does it help"; an async runtime
  we don't otherwise want fails it.)

If multi-client / heavy concurrency ever matters, revisiting `tower-lsp` is an
additive change behind the same feature handlers.

## Feature roadmap (each tier builds on the last)

| Tier | Features | Needs | Status |
|---|---|---|---|
| **0** | `publishDiagnostics` (syntactic), document sync, lifecycle | `cst::parse` + `LineIndex` | **done** |
| **1** | completion (locals + globals), hover, `documentSymbol`, **goto-definition**, **signature help** | `arglist` / `global-names` primitives; CST top-level walk (`defs`) + scope walker | **done** |
| **1+** | semantic diagnostics ("unbound" / arity / type misuse), **cross-file & stdlib goto**, `require`-target goto | located `check_file`; project bootstrap; `source-location` + prelude-cache; `require--find` | **done** |
| **2** | **cross-file** references & rename (+prepareRename), document-highlight, semantic tokens, completion resolve | `scope::references` / `references_to_global`; `project_files`; CST token classification | **done** |
| **2+** | formatting, workspace symbol, code actions (did-you-mean), folding ranges, inlay hints | `introspect::format_source`; `defs::top_level` + `workspace::all_sources`; `global_names`/`names_in_scope` + Levenshtein; CST container/comment walk; `arglist_tokens` | **done** |

Tier 0 was reachable immediately because syntactic diagnostics need only the
CST. Goto-definition landed early with Tier 1 (rather than Tier 2 as first
sketched) because the CST scope walker — its one prerequisite — was already
built as Foundation B; references, document-highlight and rename then all rode
the same `scope::references` engine (a local stays scoped to its block, a
document global spans the file; rename validates the new name through the shared
atom classifier and emits a single-file `WorkspaceEdit`). Semantic tokens are a
straight CST + scope walk (`semantic_tokens.rs`): `def`-family heads → keyword,
the defined name → function + `definition`, locals → variable, call heads →
function, with multi-line tokens split per the protocol. Completion now also
offers the special forms (not in the global table) and fills each item's
signature/docstring lazily via `completionItem/resolve`. Tiers unlock together
once their one prerequisite lands — which is the point of deciding the CST and
the introspection surface up front.

**Cross-file references and rename.** A name that resolves to a **global** is
one binding across the whole project under the flat module model (ADR-019), so
references and rename span every project file: `workspace.rs` gets the file set
from `introspect::project_files` (`(project--all-files *project-root*)`, the same
set `check-project` walks), preferring an open document's in-memory text over its
on-disk copy, and unions `ScopeTree::references_to_global` over each. Rename
emits a multi-file `WorkspaceEdit`. **Locals stay single-file** (routed to the
cursor-keyed `references`/`rename` path), and with no project bootstrapped the
set degrades to just the open buffer. This is the static, CST-level reference
model ADR-031 keeps — *definitions* are image-based, *references* stay static
because macro-generated references have no faithful spans; so references are
source occurrences, and a name another module synthesises via a macro won't
appear. **Quoted symbols are excluded** (`collect_symbols` doesn't descend into a
`'…` quote): the module name in `(require 'foo)` and quoted data `'(a b)` are
*data*, not references, so rename never rewrites them. (Quasiquote is left as-is
— its unquoted `~x` parts are live; untangling those is deferred.)

**Other caveats (deliberate).** Semantic tokens are whole-document only (no
range/delta requests — the parse is cheap). Semantic diagnostics anchor to the
offending token where the message names it (unbound symbol) or to the call
operator otherwise; deeper attribution wants spans threaded through the checker.

## The self-hosting boundary

Per the core principle (`CLAUDE.md`, ADR-006), as much as possible lives in
Brood. An LSP is *mostly mechanism* — JSON-RPC transport, the document store,
the byte↔UTF-16 map, the CST itself — legitimately Rust, the same category as
the reader and the scheduler. But *policy* can be Brood: which findings become
diagnostics, completion candidate ranking, what hover renders. A clean split:
transport + CST + position mapping in Rust; expose `arglist`, `global-names`, and
eventually an `analyze` / `completions-at` surface that is ultimately Brood
source the server calls into. We don't owe that on day one, but designing the
boundary this way keeps faith with why the project exists.

## Sketch: `parse_cst`

A skeleton, mirroring `reader.rs`'s dispatch but recording spans and recovering
instead of erroring. Elided arms are marked `…`.

```rust
//! crates/lisp/src/syntax/cst.rs — lossless, span-carrying CST for tooling.
//! Heap-free and total: `parse` ALWAYS returns a tree. Contrast `reader.rs`,
//! which yields evaluable `Value`s and rejects malformed input.

use crate::error::Span;

pub fn parse(src: &str) -> Node {
    Cst { chars: src.chars().collect(), pos: 0 }.parse_root()
}

struct Cst { chars: Vec<char>, pos: usize }

impl Cst {
    fn parse_root(&mut self) -> Node {
        let mut children = Vec::new();
        while self.pos < self.chars.len() {
            children.push(self.trivia_or_form());     // trivia kept as nodes
        }
        Node { kind: NodeKind::Root, span: self.span(0, self.pos), children }
    }

    /// One whitespace/comment run, or one form. (Lossless: trivia is in the tree.)
    fn trivia_or_form(&mut self) -> Node {
        match self.peek() {
            Some(c) if c.is_whitespace() || c == ',' => self.trivia(NodeKind::Whitespace),
            Some(';')                                => self.trivia(NodeKind::Comment),
            _                                        => self.form(),
        }
    }

    fn form(&mut self) -> Node {
        let start = self.pos;
        match self.peek() {
            Some('(') => self.seq(NodeKind::List, ')', start),
            Some('[') => self.seq(NodeKind::Vector, ']', start),
            Some('\'') => self.wrap(NodeKind::Quote, start),
            Some('`')  => self.wrap(NodeKind::Quasi, start),
            Some('~')  => { /* ~@ → Splice else Unquote */ … }
            Some('"')  => self.string(start),
            // a stray close, or EOF where a form was expected → Error, then resume
            Some(')') | Some(']') | Some('}') => { self.bump(); self.error(start) }
            Some(_)    => self.atom(start),           // classify(text) → Symbol/Int/…
            None       => self.error(start),
        }
    }

    /// `( … )` / `[ … ]`. Recovers: unmatched close ⇒ Error child; EOF ⇒ close
    /// the node at end-of-input so its children stay navigable.
    fn seq(&mut self, kind: NodeKind, close: char, start: usize) -> Node {
        self.bump();                                  // opening delimiter
        let mut children = Vec::new();
        loop {
            match self.peek() {
                None                  => break,       // unclosed — recover at EOF
                Some(c) if c == close => { self.bump(); break }
                _                     => children.push(self.trivia_or_form()),
            }
        }
        Node { kind, span: self.span(start, self.pos), children }
    }

    // wrap(kind): consume the sigil, then attach one child form.
    // atom(start): consume to the next delimiter; kind from a shared `classify`.
    // string(start): shared escape table; recover on unterminated → Str (tagged).
    // trivia / error / span / peek / bump: the obvious one-liners.
    …
}
```

The implementation note that matters: factor `is_delimiter`, the atom
classifier, and the escape table into shared helpers used by **both**
`reader.rs` and `cst.rs`, so "what counts as a symbol / number / escape" has one
definition.

## Related

- [`tooling.md`](tooling.md) — the existing editor contract (GNU errors,
  structured test output, `form-pos` / `current-file`); the LSP is its Stage-3
  generalisation.
- [`types.md`](types.md) — the advisory checker (ADR-024) that becomes the
  semantic-diagnostics engine.
- [`architecture.md`](architecture.md) / [`components.md`](components.md) — where
  the `syntax` layer and the crate split sit.
</content>
</invoke>
