# MCP server (design)

A Model Context Protocol server for Brood projects, shipped as a subcommand of
the project tool: `nest mcp`. This is the agent-side counterpart to
[`lsp.md`](lsp.md) — the LSP serves editors at a cursor, the MCP server serves
*agents* operating on the project at the level of named verbs and a long-lived
image.

> Status: **proposed** (2026-05-28). Recorded as
> [ADR-036](decisions.md#adr-036--nest-mcp-a-per-project-model-context-protocol-server-tools-surface-in-brood).
> Implementation order:
> **(1a) extract `brood::introspect` from the LSP crate into the lib — done
> (2026-05-28; lives at `crates/lisp/src/introspect.rs`, LSP migrated)**;
> (1b) widen it with the operations below;
> (2) `crates/nest/src/mcp.rs` with the JSON-RPC loop + dispatcher;
> (3) `std/mcp.blsp` with the initial tool set;
> (4) `nest new` scaffolds `.mcp.json`;
> (5) docs/devlog tick.

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
- **Widen** with the operations both will want:
  - `source_location(name) -> Option<Loc>` — wraps `(source-location 'foo)`
    (ADR-031).
  - `macroexpand_to_string(form, all: bool) -> Result<String>` — wraps
    `(macroexpand-1 …)` / `(macroexpand …)` and pretty-prints.
  - `check_project(root: &Path) -> Vec<Diag>` — wraps `(check-project)` with
    structured diagnostics. (The LSP can later adopt this once the checker
    carries spans — see [`lsp.md`](lsp.md) Tier 2.)
  - `run_tests(filter: Option<&str>) -> TestReport` — wraps the project test
    runner with structured pass/fail per test.
  - `format_source(src: &str) -> Result<String>` — calls into
    `std/format.blsp`.
  - `eval_in_session(src: &str) -> EvalResult { value, stdout, error,
    diagnostics }` — captures `*out*` and any raised error into a structured
    payload.

The contract for every operation:
1. **Total** — errors become typed fields in the result, never Rust panics.
2. **LOCAL-clean** — reclaim allocations with `Heap::checkpoint` /
   `reset_local_to` before returning (the pattern at
   `crates/lisp/src/introspect.rs:30`). A long agent session must not leak
   a fresh list per tool call.

## The tool surface (Brood, in `std/mcp.blsp`)

Eight tools, each earning its place by needing the runtime to answer —
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

Each tool is a `defn` in `std/mcp.blsp`; `(mcp-tools)` returns the catalogue
the dispatcher reads at startup. **A project can extend the surface** by
contributing entries to a shared registry from its own `mcp.blsp` — that's
the ADR-006 part: agent surfaces are Brood, not Rust.

## Resources

URI-addressed read-only blobs, served directly by the dispatcher (no eval):

- `brood://docs/brood-for-claude` — the AI pocket reference (already
  `%builtin-doc`-baked, commit `d650bcb`); the *primary* resource the agent
  fetches at session start.
- `brood://docs/language` — language reference.
- `brood://docs/decisions` — ADRs.
- `brood://docs/types` — the type system contract.
- `brood://prelude` — the prelude source.
- `brood://project` — the project manifest (`project.blsp`).

## Prompts

A single `brood-task` prompt template that pre-seeds the conventions an agent
needs (CLAUDE.md essentials + a pointer at `brood://docs/brood-for-claude`).
Optional and additive — the agent can do everything via tools alone.

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
