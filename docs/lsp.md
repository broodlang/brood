# Language server (design)

A Language Server Protocol (LSP) server for Brood, shipped as a separate binary
(`brood-lsp`) in its own workspace crate. This is the cross-editor generalisation
of the editor contract in [`tooling.md`](tooling.md): instead of every editor
re-implementing "run the file and parse the GNU error lines", they speak LSP to
one server that owns the language knowledge.

> Status: **Tier 0 live.** Recorded as
> [ADR-025](decisions.md#adr-025--a-lossless-span-carrying-cst-for-tooling-separate-from-the-eval-value);
> this document is the full plan it points to (the `types.md` ↔ ADR-024 pattern).
> **Done:** Foundation A — the CST (`syntax::cst`) + shared lexical rules
> (`syntax::atom`) + `error::Span`; Foundation B — the CST scope resolver
> (`syntax::scope`); Foundation C — leading-string docstrings and the
> introspection primitives `doc` / `arglist` / `global-names` / `bound?`. And
> **Tier 0** — the `crates/lsp` → `brood-lsp` binary: stdio lifecycle, full
> document sync, and syntactic `publishDiagnostics` read off the CST
> (`lsp-server` + `lsp-types`, no async runtime). `Uri`→text document store, a
> `LineIndex` for byte↔UTF-16 `Position`, and `diagnostics::collect` over the
> CST's `Error` nodes.
> **Next:** Tier 1 — completion (globals), hover + signature help, and
> `documentSymbol`, built on the Foundation B/C surface.

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

| Tier | Features | Needs | Rough size |
|---|---|---|---|
| **0** | `publishDiagnostics` (syntactic), document sync, lifecycle | `cst::parse` + `LineIndex` | ~1–2 days |
| **1** | completion (globals), hover + signature help, `documentSymbol` (top-level defs) | `arglist` / `global-names` primitives; CST top-level walk | a few days |
| **2** | goto-definition, references, rename, semantic tokens, "unbound" diagnostics | the CST **scope walker**; located checker output | ~1–2 weeks |

Tier 0 is reachable immediately because syntactic diagnostics need only the CST.
Tiers unlock together once their one prerequisite lands — which is the point of
deciding the CST and the introspection surface up front.

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
