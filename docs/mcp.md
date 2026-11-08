# MCP server (design)

A Model Context Protocol server for Brood projects, shipped as a subcommand of
the project tool: `nest mcp`. This is the agent-side counterpart to
[`lsp.md`](lsp.md) — the LSP serves editors at a cursor, the MCP server serves
*agents* operating on the project at the level of named verbs and a long-lived
image.

> Status: **implemented, v0** (2026-05-28; catalogue since grown to 24 tools —
> see the tool table below; the whole server moved to Brood on 2026-10-09). Recorded as
> [ADR-036](decisions/adr-001-099.md#adr-036--nest-mcp-a-per-project-model-context-protocol-server-tools-surface-in-brood).
> The full catalogue is live (including the once-stubbed `check` / `run-tests` /
> `processes`, and the 2026-07-30 ability/type trio `abilities` / `ability` /
> `check-source`), with the three Tier-1 niceties in place (project-defined tool
> discovery, `prompts/get`, `.mcp.json` scaffolded by `nest new`). A handler's printed
> output is captured off the JSON-RPC stream (see *Captured stdout* below).
> Implementation order, as it happened:
> **(1a) extract `brood::introspect` from the LSP crate into the lib — done
> (2026-05-28; lives at `crates/lisp/src/introspect.rs`, LSP migrated)**;
> **(1b) widen `brood::introspect` with the operations the MCP tools need —
> done (2026-05-28; four new helpers + a `Diag` / `SourceLoc` / `EvalResult`
> type vocabulary, 12 new tests, all green)**;
> (1c) Brood-side prereqs for the two deferred operations
> (`check_project` / `run_tests` want a *structured-result* variant in
> `std/tool/project.blsp` and `std/tool/test.blsp`; `eval_in_session.stdout` wants a
> `with-out-str` facility — `*out*` dynvar + a Rust capture primitive);
> **(2) `crates/nest/src/mcp.rs` — done (2026-05-28; sync JSON-RPC loop,
> `serde_json` framing, initialize / tools{list,call} / resources{list,read}
> / prompts/list / ping / shutdown / exit; Brood ↔ JSON converters; 13
> dispatcher tests; `nest mcp` subcommand wired with the strict-per-project
> bootstrap; happy-path verified against the real binary)**;
> **(3) `std/tool/mcp.blsp` — done (2026-05-28; eight tool `defn`s + `(mcp-tools)`
> registry; six live (`eval`, `load`, `lookup`, `macroexpand`, `format`, and
> dispatch for any project-defined tools), three documented stubs (`check`,
> `run-tests`, `processes`); embedded so `(require 'mcp)`
> finds it without a load-path)**;
> **(4) `nest new` scaffolds `.mcp.json` — done (2026-05-28; the scaffolder
> writes `foo/.mcp.json` pointing at `nest mcp`, so `cd foo && claude`
> auto-attaches the agent)**;
> **(1c-a) a structured project check (`project-check/check-structured` today) —
> done (returns `[{:file :line :col :message}]`); `check` MCP tool now
> dispatches through it**;
> **(1c-b) a structured test run (`project-run/run-tests-structured` today) — done;
> `run-tests` MCP tool now returns `{:total :passed :failed :failed-assertions :ms
> :results [...]}` instead of the stub**;
> **(1c-d) a process-listing primitive — done; the `processes` MCP tool
> reports every live process**;
> **(5a) `prompts/get` with `brood-task` — done (a single orientation
> prompt that points at `brood://docs/brood-for-claude` and lists the
> tool surface)**;
> **(5b) project-defined tool discovery — done (`std/tool/mcp.blsp` auto-loads
> `<project-root>/mcp.blsp` if present; the
> project file can `def mcp-tools` to extend or replace the catalogue)**;
> **(1c-c) `*out*` dynvar + `with-out-str` + dispatcher stdout capture — done, as a
> process-scoped, spawn-inherited capture buffer (see *Captured stdout* below)**.

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
                              │   (mcp/tools) in      │           │ introspect (shared layer)  │
                              │   std/tool/mcp.blsp        │           │   • lookup, eval, …       │
                              └──────────────────────┘           └───────────────────────────┘
```

One `Interp` per MCP session. The dispatcher receives a `tools/call`, finds
the handler in the catalogue `(mcp/tools)` returns (or the project's own `mcp-tools`),
runs the handler with JSON args converted to a Brood map, and packages the
result as the JSON-RPC response.

## The load-bearing decision: a shared `brood::introspect`

`brood::introspect` (`crates/lisp/src/introspect.rs`) is a thin Rust wrapper
that does `eval_str("(reflect/arglist FOO)")` and unpacks the result. The MCP server
wants every operation it already has and a few more. To prevent two clients
drifting on "what `map`'s signature is":

- **Moved** `global_names` / `signature` / `arglist_tokens` from the LSP crate
  to the lib (2026-05-28), and the LSP consumes them from `brood::introspect`.
  The MCP server reaches the same primitives from Brood since the 2026-10-09 move.
- **Widened** with four of the operations both surfaces want (step 1b,
  2026-05-28):
  - `source_location(name) -> Option<SourceLoc>` — wraps `(reflect/source-location 'foo)`
    (ADR-031). **Done.**
  - `macroexpand_to_string(src, recursive: bool) -> Result<String>` — parses
    `src` directly and calls `eval::macros::macroexpand_1` / `macroexpand`
    (the eval-via-string path would be vulnerable to unbalanced delimiters in
    `src`). **Done.**
  - `format_source(src) -> Result<String>` — calls into `std/format.blsp`'s
    `(format/source SRC)`. **Done.**
  - `eval_in_session(src) -> EvalResult { value, error, diagnostics }` —
    structured eval; state accumulates across calls (hot reload). **Done.**
- **Delivered in Brood in step 1c**, with no Rust wrapper needed once the server moved:
  - the project check — `project-check/check-structured` returns
    `{:file :line :col :message}` maps.
  - the test run — `project-run/run-tests-structured` returns the runner's structured
    result.
  - printed output — see *Captured stdout* below.

The contract for every operation:
1. **Total** — errors become typed fields in the result, never Rust panics.
2. **LOCAL-clean** — reclaim allocations with `Heap::checkpoint` /
   `reset_local_to` before returning (the pattern in
   `crates/lisp/src/introspect.rs`). A long agent session must not leak
   a fresh list per tool call.

## The tool surface (Brood, in `std/tool/mcp.blsp`)

Twenty-four tools, each earning its place by needing the runtime to answer —
anything a plain file read or grep would answer is **not** here, because
Claude Code already has those:

| Tool | Args | Returns | Why it needs the runtime |
|---|---|---|---|
| `eval`        | `{source}`                  | `{value}` or `{error}`                 | The point — iterate without restart; runs every form and returns the last value |
| `load`        | `{file}`                    | `{ok, diagnostics, shadows}`           | Reload a `.blsp` into the live image, *and* check it: `diagnostics` are `check-file-structured` warnings (type/arity/unbound/non-tail-recursion); `shadows` flags names this file also-defines in another source file (flat-namespace collision) |
| `write`       | `{path, content}`           | `{ok, path, diagnostics, shadows}`     | Create/overwrite a project file *through* the image, not the raw filesystem: `path` is project-relative and sandboxed under the project root (absolute paths, `~`, `..` rejected); a `.blsp` file is loaded into the session and checked, so success carries `diagnostics`/`shadows` like `load` (failure is `{ok: false, error}`) |
| `edit`        | `{path, old, new}`          | `{ok, path, diagnostics, shadows}`     | Exact-string replace in a project file (sandboxed like `write`; `old` must occur exactly once); a `.blsp` file is reloaded and checked, same result shape as `write` |
| `lookup`      | `{name}`                    | `{arglist, doc, type, source_location}`| Resolves prelude, project, macros uniformly; `type` is the checker's arrow signature (shared with LSP hover) |

> **"Sandboxed" above means an ergonomic guardrail on `write`/`edit`, not a containment
> boundary for the server.** The path gate itself is real and was audited: `project-path`
> rejects `/`, `~` and `..` lexically *and* re-checks containment after `canonicalize` on
> both sides, and `canonicalize` resolves the longest existing prefix through the
> filesystem (so symlinks in it are followed) before applying any non-existent tail
> lexically — no traversal or symlink escape was found. But `eval` runs arbitrary Brood
> with the full builtin set (`file/spit`, `os/run-process`, `%os-cmd`) and `load` takes an
> unsandboxed path, and neither goes through `project-path`. So an agent driving this
> server can write anywhere the process can: **the `nest mcp` process is the trust
> boundary, not the project root.** Run it as a principal you would give that access to.
| `abilities`   | `{}`                        | `{abilities: [name]}`                  | List every ability (generic-function interface) in the image — the discovery entry point for `ability` |
| `ability`     | `{name}`                    | `{ops, sealed, members, requires, owner, derivable, impls}` or `{error}` | Describe a dispatch point: its ops (with params, provided-default flag), sealed member set, `:requires` super-abilities, and which types implement it — read it before writing an `impl` |
| `check-source`| `{source}`                  | `{diagnostics: [{line, col, message}]}`| Advisory type-check a source **string** (not the whole project) — iterate on a snippet before writing it to a file |
| `macroexpand` | `{form, mode: "1"\|"all"}`  | `{expanded}`                           | Teaches the agent quasiquote/`when-let`/etc. |
| `run-tests`   | `{}`                        | `{total, passed, failed, failed-assertions, ms, results}` | Structured, not GNU-line parsing |
| `check`       | `{}`                        | `{diagnostics: [{file, line, col, message}]}` | Advisory type-check of the project, structured |
| `format`      | `{source}`                  | `{formatted}`                          | Idempotent reformatter |
| `processes`   | `{}`                        | `{processes: [{id, status, mailbox, memory, reductions, ...}]}` | After `spawn`, snapshot every live green process with its full `proc/info` stats — the observer's per-process view (mailbox backlog, reductions/work counter, heap, monitors) |
| `process-info`| `{id}`                      | `{id, status, mailbox, reductions, ...}` or `{error}` | Drill into one process by the numeric id from `processes` |
| `node`        | `{}`                        | `{node, workers, peak-threads, spawned, process-count, mem-bytes, mem-peak, peers}` | Runtime-wide stats — the observer's header; "is the runtime healthy/busy?" |
| `vitals`      | `{}`                        | `{gc, sched, vm, processes, metrics}`  | The unified snapshot behind one call (`telemetry/runtime-snapshot`); the point-in-time companion to `watch-runtime` |
| `watch-runtime`| `{ms?, filter?}`           | `{events: [{kind, pid, detail}], count, ms}` | Watch the kernel runtime-event **stream** for `ms` (default 500, capped 5000) — GC pauses (`:gc`), spawn/exit churn (`:spawn`/`:exit`), JIT deopts (`:deopt`) — a *trace*, not a snapshot. `filter` selects kinds (e.g. `{gc: true, exit: true}`). |
| `callers`     | `{name}`                    | `{references: [{file, line, col}]}`    | Cross-file find-references — the *use* sites of a global (complements `lookup`'s def site) |
| `apropos`     | `{pattern}`                 | `{matches: [name]}`                    | Discover globals by name substring — answer "does X exist?" without guessing names |
| `all-globals` | `{}`                        | `{globals: [name]}`                    | The full name list of the live image (prelude + project) |
| `doc-search`  | `{query}`                   | `{results: [{name, doc}]}`             | Find a capability by *behaviour* — searches docstrings, not names |
| `bench`       | `{source, iterations?}`     | `{ms, iterations, per-iter-ms, value}` | Time an expression in the live image |
| `error`       | `{code}`                    | `{code, summary, causes, fix, example}` | Teach-the-error — a stable error code (`E0044`) → its Brood-idiomatic **fix**, not just the message (`std/tool/explain.blsp`) |
| `find-pattern`  | `{query}`                 | `{matches: [{intent, idiom, example, see}]}` | Intent → idiom — "how do I loop / mutate / build a string / spawn"? The Brood way, not a Clojure/Scheme reflex |

Each tool is a `defn` in `std/tool/mcp.blsp`; `(mcp/tools)` returns the catalogue
the dispatcher reads on every `tools/list` and `tools/call`. **A project can extend the
surface** by defining `mcp-tools` in its own `mcp.blsp`, typically
`(defn mcp-tools () (cons {…} (mcp/tools)))` — that's
the ADR-006 part: agent surfaces are Brood, not Rust.

**`eval`/`load` run under a 30s watchdog (2026-05-30).** They evaluate arbitrary
code, so a runaway (an infinite loop) would otherwise wedge the single-threaded
server. Every handler runs in its own process; for `eval`/`load` the dispatcher waits
at most 30s for its answer, then kills it (`(exit pid :kill)`) and replies with an
`evaluation exceeded its time limit` error. The other tools run unbounded (`run-tests` self-bounds per test). Handler output
(`io/puts`, and terminal escapes from `term/draw`/`term/emit`) is captured off the
JSON-RPC stream via a **process-scoped, spawn-inherited** capture buffer, so even a
handler that spawns can't corrupt the channel.

**A raise says where it came from.** A handler that raises is answered with a
`-32603` whose `error.data` is the kernel's error map (`kind`, `code`, `message`,
`file`/`line`/`col`, `trace`); a value the handler *threw* (`(throw "bad input")`,
`(error …)`) is `{kind: "user", message, file, line, col, trace}`. A tool's own soft
`error` field (`eval`, `load`, `write`, …) carries the same position, a thrown value as
`{kind: :user, payload, file, line, col, trace}`. `catch` binds a thrown value verbatim, so
the dispatcher reads the position through `%try-described` (see `language.md`).

## Resources

URI-addressed read-only blobs, served directly by the dispatcher (no eval):

- `brood://docs/brood-for-claude` — the AI pocket reference (already
  `%builtin-doc`-baked, commit `d650bcb`); the *primary* resource the agent
  fetches at session start.
- `brood://docs/error-codes` — stable error codes (`E0010`/`E0030`/etc.)
  and the structured `catch` shape (`{:kind :code :message :file :line
  :col :hint :trace}`). Agents branch on `:code` / `:kind` for programmatic
  handling; the dispatcher mirrors the same fields into
  JSON-RPC `error.data`. See [`archive/llm-native.md`](archive/llm-native.md) §4.
- `brood://docs/language` — language reference.
- `brood://docs/decisions` — ADRs.
- `brood://docs/types` — the type system contract.
- `brood://prelude` — the prelude source.

Notes about developing Brood itself (`docs/incarnations.md`, the archived LLM-native plan
and demo findings) are not served: the server runs in a user's project, and an agent there
should read the language reference, not the repo's working notes (2026-11-08).

## Prompts

A single `brood-task` prompt template that orients a fresh agent. Sourced
from [`docs/prompts/brood-task.md`](prompts/brood-task.md) — the markdown
file is `include_str!`'d into `nest` (`crates/nest/src/mcp.rs`), so the maintainer
edits it as a document, *and* other agent harnesses (Cursor,
Aider, Continue per [`archive/llm-native.md`](archive/llm-native.md) §14) can copy the
file into their system prompts and get the same content. Points the agent
at `brood-for-claude` and the project's `CLAUDE.md`, lists the language traps, and
names every tool. Optional and additive — the agent can do everything via tools alone.

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
- **The test registry resets per run.** `(describe …)` / `(test …)` *register*
  by accumulating into `test/*units*` (`std/tool/test.blsp`); in a long-lived image,
  `load`ing the same test file twice would register every unit twice and
  inflate the `run-tests` count. The project test runners call `reset-units`
  before (re)loading test files, so each `run-tests` reflects exactly the
  current files — no fall back to a fresh `nest test` for a trustworthy count.

## Captured stdout — `print` is safe in a handler

The transport is newline-delimited JSON over **stdout**, so a handler that
wrote straight to stdout would corrupt the JSON-RPC stream — the one
thing the skill used to have to warn against, because printing is the natural
debugging reflex. It no longer can: the dispatcher installs a process-scoped,
spawn-inherited capture buffer around every `tools/call`, so `io/puts` and `io/write`
divert into it instead of the channel. The captured text rides back as a **second** content
block in the reply, labelled `[captured stdout]`:

```json
{ "content": [
    { "type": "text", "text": "<the handler's return value as JSON>" },
    { "type": "text", "text": "[captured stdout]\n…whatever the handler printed…" }
]}
```

`content[0]` is always the return value (the stable shape an agent parses); the
stdout block appears only when something was printed. It is delivered uniformly for every
tool rather than threaded through each handler's return map. Capture is
process-scoped and **spawn-inherited** — it lives in the process `Ctx`
(`crates/lisp/src/builtins/io.rs`), so output from green processes the handler
`spawn`s on other workers is captured too, not just the synchronous handler's.
Output to `*err*` is untouched — it never shared the protocol channel.

## Where it lives

Since 2026-10-09 the whole server — transport, dispatch, JSON, error mapping and the tool
catalogue — is Brood (`std/tool/mcp.blsp`), routed like every other Brood `nest`
subcommand. `crates/nest/src/mcp.rs` supplies only what the binary compiled in: the
reference documents an agent reads as MCP resources, the prelude source and the
`brood-task` prompt, bound as `mcp/*documents*` around the dispatcher call. No protocol
crate is involved.

**Transport — newline-delimited JSON, not LSP framing.** The MCP stdio transport
frames each JSON-RPC message as one line of compact JSON terminated by `\n` (no
embedded newlines), **not** the `Content-Length` headers LSP uses. This matters:
`brood-lsp` and `nest mcp` look alike (both sync JSON-RPC over stdio) but their
wire framing differs, and using the LSP framing here silently breaks every real
MCP client — the `initialize` handshake never completes because the client's
newline-framed bytes aren't parsed. (The framing is in `std/tool/mcp.blsp`;
regression-tested.)

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
| **0** | lifecycle, `tools/{list,call}`, `resources/{list,read}`, the eight core tools, the doc resources | `brood::introspect` extracted from LSP | ✅ |
| **1** | project-defined tools (a project's `mcp.blsp` extends the registry), `prompts/get` for `brood-task` | tool-extension API in `std/tool/mcp.blsp`     | ✅ |
| **2** | structured progress notifications (long `run-tests`), per-tool sandboxing (`:safe` allowlist)        | progress event channel                   | progress ✅ (2026-07-23); sandboxing later |

**Progress notifications (Tier 2, shipped 2026-07-23).** A `tools/call` that
carries an MCP `_meta.progressToken` gets live `notifications/progress`
streamed on the stdout channel during the (synchronous) call: a Brood handler
calls `(mcp/progress progress total message)` — a no-op without a token, so
any handler can use it. The `check` tool reports per file
(`checked foo.blsp`, 3/12). Mechanism: the handler runs in its own process with
`mcp/*progress-sink*` bound; `mcp/progress` sends each report to the dispatcher,
which writes the notification while it waits for the handler's answer.
`project-check` calls it only when the `mcp` module is already loaded, so a plain
`nest check` never loads it.

## The self-hosting boundary

Per ADR-006 the server is Brood end to end (`std/tool/mcp.blsp`); the only Rust left
is the binary's embedded documents. A project can register its own tools without a
Rust release. That's the part that makes `nest mcp` look like a language feature
rather than a one-off binary: when an editor written in Brood
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
