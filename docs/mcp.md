# MCP server (design)

A Model Context Protocol server for Brood projects, shipped as a subcommand of
the project tool: `nest mcp`. This is the agent-side counterpart to
[`lsp.md`](lsp.md) — the LSP serves editors at a cursor, the MCP server serves
*agents* operating on the project at the level of named verbs and a long-lived
image.

> Status: **implemented, v0** (2026-05-28). Recorded as
> [ADR-036](decisions.md#adr-036--nest-mcp-a-per-project-model-context-protocol-server-tools-surface-in-brood).
> Six of the eight tools live, three Tier-1 niceties in place (project-defined
> tool discovery, `prompts/get`, `.mcp.json` scaffolded by `nest new`). The
> single remaining piece is the **`*out*` dynvar / `with-out-str` work**
> (step 1c-c) — needed to fold a `:stdout` field into `EvalResult` and to
> safely redirect `print` output away from the dispatcher's stdout. Deferred
> until a concrete need; agents using `eval` should return data via the
> result value rather than calling `(print …)` (which corrupts the JSON-RPC
> stream on the current main path).
> Implementation order:
> **(1a) extract `brood::introspect` from the LSP crate into the lib — done
> (2026-05-28; lives at `crates/lisp/src/introspect.rs`, LSP migrated)**;
> **(1b) widen `brood::introspect` with the operations the MCP tools need —
> done (2026-05-28; four new helpers + a `Diag` / `SourceLoc` / `EvalResult`
> type vocabulary, 12 new tests, all green)**;
> (1c) Brood-side prereqs for the two deferred operations
> (`check_project` / `run_tests` want a *structured-result* variant in
> `std/project.blsp` and `std/test.blsp`; `eval_in_session.stdout` wants a
> `with-out-str` facility — `*out*` dynvar + a Rust capture primitive);
> **(2) `crates/nest/src/mcp.rs` — done (2026-05-28; sync JSON-RPC loop,
> `serde_json` framing, initialize / tools{list,call} / resources{list,read}
> / prompts/list / ping / shutdown / exit; Brood ↔ JSON converters; 13
> dispatcher tests; `nest mcp` subcommand wired with the strict-per-project
> bootstrap; happy-path verified against the real binary)**;
> **(3) `std/mcp.blsp` — done (2026-05-28; eight tool `defn`s + `(mcp-tools)`
> registry; six live (`eval`, `load`, `lookup`, `macroexpand`, `format`, and
> dispatch for any project-defined tools), three documented stubs (`check`,
> `run-tests`, `processes`); added to `EMBEDDED_MODULES` so `(require 'mcp)`
> finds it without a load-path)**;
> **(4) `nest new` scaffolds `.mcp.json` — done (2026-05-28; the scaffolder
> writes `foo/.mcp.json` pointing at `nest mcp`, so `cd foo && claude`
> auto-attaches the agent)**;
> **(1c-a) structured `(check-project-structured)` in `std/project.blsp` —
> done (returns `[{:file :line :col :message}]`); `check` MCP tool now
> dispatches through it**;
> **(1c-b) structured `(run-tests-structured)` in `std/test.blsp` and
> `(run-project-tests-structured)` in `std/project.blsp` — done; `run-tests`
> MCP tool now returns `{:total :passed :failed :failed-assertions :ms
> :results [...]}` instead of the stub**;
> **(1c-d) `(list-processes)` Rust primitive — done; `processes` MCP tool
> now returns `{:processes [{:$type :pid :node :id} ...]}` (an empty array,
> not nil/null, when nothing is registered)**;
> **(5a) `prompts/get` with `brood-task` — done (a single orientation
> prompt that points at `brood://docs/brood-for-claude` and lists the
> tool surface)**;
> **(5b) project-defined tool discovery — done (`std/mcp.blsp` auto-loads
> `<project-root>/mcp.blsp` if present, after `provide`ing 'mcp; the
> project file can `def mcp-tools` to extend or replace the catalogue)**;
> (1c-c) `*out*` dynvar + `(with-out-str)` + dispatcher stdout redirect
> — **deferred**; the `EvalResult` shape sketched in step 1b includes a
> `:stdout` field that will be lit up by this. Needs design for
> per-process buffering across the scheduler's worker threads (a
> thread-local would leak captures across green processes scheduled on
> the same OS thread).

## Why a server, and why not just the LSP

The temptation is to bolt agent tooling onto the LSP — it already speaks
JSON-RPC over stdio, owns an `Interp`, knows about projects. Two things make
that the wrong shape:

| | `brood-lsp` | `brood-mcp` (this) |
|---|---|---|
| Addresses things by | **cursor in an open buffer** | **name** (`map`, `tests/foo_test.blsp`) |
| Runs user code | **never** — a half-typed buffer can't be eval'd | **yes** — `eval` / `run-tests` / `load` are the point |
| `Interp` lifetime | one, never mutated by buffer text | one per session, **mutated by the agent** |
| Latency budget | sub-frame (typing) | per-call (seconds OK) |

LSP is editor-shaped: it must stay safe on a half-typed buffer and reply in
under a frame. MCP is task-shaped: the agent has decided to do something and
is calling a verb. Forcing one surface to do both compromises both.

The unique fit MCP has, and LSP can't safely give, is **a long-lived
per-session image** the agent mutates via `def`/`load`/`eval`, where Brood's
existing hot reload (ADR-013) lets the next call see the new binding. That's
the way Lisp has always wanted to be developed; MCP is the protocol that lets
an *agent* do it.

## Architecture

```
   agent ⇄ JSON-RPC/stdio ⇄  nest mcp (crates/nest)             brood (crates/lisp, the lib)
                              ┌──────────────────────┐           ┌───────────────────────────┐
   tools/call ─────────────▶  │ JSON-RPC dispatcher   │           │ Interp (long-lived)        │
   resources/read ─────────▶  │ session state         │─────────▶│   • mutable RUNTIME image  │
   prompts/get ────────────▶  │ tool registry from    │           │   • spawned processes      │
                              │   (mcp-tools) in      │           │ introspect (shared layer)  │
                              │   std/mcp.blsp        │           │   • lookup, eval, …       │
                              └──────────────────────┘           └───────────────────────────┘
```

One `Interp` per MCP session. The dispatcher receives a `tools/call`, finds
the handler in the registry the Brood-side `(mcp-tools)` produced at startup,
`eval`s the handler with JSON args converted to a Brood map, and packages the
result as the JSON-RPC response.

## The load-bearing decision: a shared `brood::introspect`

`brood::introspect` (`crates/lisp/src/introspect.rs`) is a thin Rust wrapper
that does `eval_str("(arglist FOO)")` and unpacks the result. The MCP server
wants every operation it already has and a few more. To prevent two clients
drifting on "what `map`'s signature is":

- **Moved** `global_names` / `signature` / `arglist_tokens` from the LSP crate
  to the lib (2026-05-28); both LSP and the future MCP dispatcher consume
  them from `brood::introspect`.
- **Widened** with four of the operations both surfaces want (step 1b,
  2026-05-28):
  - `source_location(name) -> Option<SourceLoc>` — wraps `(source-location 'foo)`
    (ADR-031). **Done.**
  - `macroexpand_to_string(src, recursive: bool) -> Result<String>` — parses
    `src` directly and calls `eval::macros::macroexpand_1` / `macroexpand`
    (the eval-via-string path would be vulnerable to unbalanced delimiters in
    `src`). **Done.**
  - `format_source(src) -> Result<String>` — calls into `std/format.blsp`'s
    `(format-source SRC)`. **Done.**
  - `eval_in_session(src) -> EvalResult { value, error, diagnostics }` —
    structured eval; state accumulates across calls (hot reload). **Done.**
- **Deferred to step 1c**, behind Brood-side prereqs:
  - `check_project(root) -> Vec<Diag>` — today `(check-project)` is
    print-oriented (GNU lines + an `Int` count). Needs a structured variant
    in `std/project.blsp` that returns `[file line col message]` tuples
    before a faithful Rust wrapper can land.
  - `run_tests(filter) -> TestReport` — same shape: `(run-project-tests)`
    prints GNU per-test output and raises on failure. Wants a structured
    runner result from `std/test.blsp`.
  - `EvalResult.stdout` — needs `*out*` (a dynvar) + a `with-out-str` capture
    primitive. Out of scope here; `eval_in_session` ships without it for now,
    since `value` + `error` + `diagnostics` are already useful and the agent
    can read side-effect outputs another way (`(println …)` to a known
    buffer-as-data structure, for instance).

The contract for every operation:
1. **Total** — errors become typed fields in the result, never Rust panics.
2. **LOCAL-clean** — reclaim allocations with `Heap::checkpoint` /
   `reset_local_to` before returning (the pattern at
   `crates/lisp/src/introspect.rs:30`). A long agent session must not leak
   a fresh list per tool call.

## The tool surface (Brood, in `std/mcp.blsp`)

Nine tools, each earning its place by needing the runtime to answer —
anything a plain file read or grep would answer is **not** here, because
Claude Code already has those:

| Tool | Args | Returns | Why it needs the runtime |
|---|---|---|---|
| `eval`        | `{source}`                  | `{value, stdout, error?, diagnostics}` | The point — iterate without restart |
| `load`        | `{file}`                    | `{ok, diagnostics}`                    | Reload a `.blsp` into the live image |
| `lookup`      | `{name}`                    | `{arglist, doc, source_location, kind}`| Resolves prelude, project, macros uniformly |
| `macroexpand` | `{form, mode: "1"\|"all"}`  | `{expanded}`                           | Teaches the agent quasiquote/`when-let`/etc. |
| `run-tests`   | `{file?, name?}`            | `[{name, status, output}]`             | Structured, not GNU-line parsing |
| `check`       | `{file?}`                   | `[{file, line, col, message}]`         | Advisory type-check, structured |
| `format`      | `{file?, source?}`          | `{formatted}`                          | Idempotent reformatter |
| `processes`   | `{}`                        | `[{pid, status, ...}]`                 | After `spawn`, list live green processes |
| `callers`     | `{name}`                    | `{references: [{file, line, col}]}`    | Cross-file find-references — the *use* sites of a global (complements `lookup`'s def site) |

Each tool is a `defn` in `std/mcp.blsp`; `(mcp-tools)` returns the catalogue
the dispatcher reads at startup. **A project can extend the surface** by
contributing entries to a shared registry from its own `mcp.blsp` — that's
the ADR-006 part: agent surfaces are Brood, not Rust.

## Resources

URI-addressed read-only blobs, served directly by the dispatcher (no eval):

- `brood://docs/brood-for-claude` — the AI pocket reference (already
  `%builtin-doc`-baked, commit `d650bcb`); the *primary* resource the agent
  fetches at session start.
- `brood://docs/incarnations` — the self-improving findings index
  (`docs/incarnations.md`, [`llm-native.md`](llm-native.md) §3). Agents
  append a one-paragraph entry + a full writeup at the end of a non-trivial
  session; the next agent reads it first.
- `brood://docs/claude-demo-findings` — the first incarnation
  (`docs/claude-demo-findings.md`, Claude Opus 4.7 building a concurrent
  Mandelbrot). 511 lines of real notes on what bit a real agent.
- `brood://docs/llm-native` — the forward-looking plan for making Brood
  LLM-native; the status block at the bottom maps what's done vs open.
- `brood://docs/error-codes` — stable error codes (`E0010`/`E0030`/etc.)
  and the structured `catch` shape (`{:kind :code :message :file :line
  :col :hint}`). Agents branch on `:code` / `:kind` for programmatic
  handling; the dispatcher mirrors the same fields into
  JSON-RPC `error.data`. See [`llm-native.md`](llm-native.md) §4.
- `brood://docs/language` — language reference.
- `brood://docs/decisions` — ADRs.
- `brood://docs/types` — the type system contract.
- `brood://prelude` — the prelude source.
- `brood://project` — the project manifest (`project.blsp`).

## Prompts

A single `brood-task` prompt template that orients a fresh agent. Sourced
from [`docs/prompts/brood-task.md`](prompts/brood-task.md) — the markdown
file is `include_str!`'d into the dispatcher, so the maintainer can edit
the prompt without recompiling, *and* other agent harnesses (Cursor,
Aider, Continue per [`llm-native.md`](llm-native.md) §14) can copy the
file into their system prompts and get the same content. Points the agent
at the three reads-first resources: `brood-for-claude`, `incarnations`,
and the project's `CLAUDE.md`. Optional and additive — the agent can do
everything via tools alone.

## Session model & hot reload

- **One `Interp` per JSON-RPC connection**, alive for that connection.
  State accumulates: a `def` in one `eval` call mutates the shared RUNTIME
  region (ADR-013); the next call sees it. Green processes spawned by an
  `eval` keep running between calls; subsequent `eval`s can `send` /
  `receive` against them.
- **No cross-connection sharing.** Two agents on one project root each get a
  fresh `nest mcp` process and a fresh image — same constraint that drove the
  LSP's design (`!Sync` `Heap`). Revisiting is an additive change later.
- **Hot reload is documented as the headline behaviour** in
  `brood-for-claude.md`. The agent has to know that `def` is the loop, not
  "edit file + restart".

## The crate

`nest mcp` lives as a module inside `crates/nest/` (`crates/nest/src/mcp.rs`),
not a separate crate. Same logic as ADR-028's "thin Rust shell": the
dispatcher is small; the *policy* is Brood. Promote to `crates/mcp/` only when
something else needs to embed it (an editor host, a remote server) — the move
is mechanical.

**Protocol crates:** none, beyond `serde_json`. MCP's surface is small enough
that a direct sync JSON-RPC loop stays under a few hundred lines, matching the
`!Sync` `Heap` constraint with no `tokio`. (`rmcp` exists; the dep + async
cost outweighs the saving here, same calculus as ADR-025 over `tower-lsp`.)

**Transport — newline-delimited JSON, not LSP framing.** The MCP stdio transport
frames each JSON-RPC message as one line of compact JSON terminated by `\n` (no
embedded newlines), **not** the `Content-Length` headers LSP uses. This matters:
`brood-lsp` and `nest mcp` look alike (both sync JSON-RPC over stdio) but their
wire framing differs, and using the LSP framing here silently breaks every real
MCP client — the `initialize` handshake never completes because the client's
newline-framed bytes aren't parsed. (`read_message`/`write_message` in
`crates/nest/src/mcp.rs`; regression-tested.)

## The `.mcp.json` scaffold

`nest new foo` writes `foo/.mcp.json`:

```json
{
  "mcpServers": {
    "brood": { "command": "nest", "args": ["mcp"] }
  }
}
```

So `cd foo && claude` auto-attaches the project's MCP server. Combined with
the `%builtin-doc`-baked `brood-for-claude.md`, a fresh project is ready for
agent-assisted development from the first commit — closing the loop with the
scaffolding work already done.

## Feature roadmap (each tier earns its place)

| Tier | Surface | Needs | Status |
|---|---|---|---|
| **0** | lifecycle, `tools/{list,call}`, `resources/{list,read}`, the eight core tools, the doc resources | `brood::introspect` extracted from LSP | proposed |
| **1** | project-defined tools (a project's `mcp.blsp` extends the registry), `prompts/get` for `brood-task` | tool-extension API in `std/mcp.blsp`     | next     |
| **2** | structured progress notifications (long `run-tests`), per-tool sandboxing (`:safe` allowlist)        | progress event channel                   | later    |

Tier 0 is reachable now because the prerequisites — the introspection
primitives (ADR-025), project bootstrap (ADR-020/028), hot reload (ADR-013) —
are all in place.

## The self-hosting boundary

Per ADR-006, the Rust side is *mechanism* — JSON-RPC framing, schema
validation, the `Interp` lifetime, resource file reads — the same category as
the reader and the LSP transport. **Tool semantics are Brood**
(`std/mcp.blsp`), so a project can register its own tools without a Rust
release. That's the part that makes `nest mcp` look like a language feature
rather than a one-off binary: when the future Brood-hosted editor (M2/M3)
sprouts an MCP server, the Brood half ports unchanged; only the transport
re-hosts inside the editor's event loop.

## Related

- [`lsp.md`](lsp.md) — the editor-side counterpart; same JSON-RPC-over-stdio
  shape, opposite addressing model.
- [`shared-code.md`](shared-code.md) — the hot-reload semantics that make a
  long-lived MCP session useful in the first place.
- [`tooling.md`](tooling.md) — the GNU-line contract that the structured
  `check` / `run-tests` outputs *supersede* for MCP clients (the GNU lines
  remain for humans and editors).
- [`brood-for-claude.md`](brood-for-claude.md) — the resource the MCP server's
  `brood://docs/brood-for-claude` URI serves; the primary thing an agent
  fetches at session start.
