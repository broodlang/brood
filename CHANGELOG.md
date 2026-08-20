# Changelog

All notable changes to the Brood toolchain (`brood`, `nest`, `brood-lsp`) are
recorded here. Versions follow [semver](https://semver.org); the full
engineering narrative lives in [`docs/devlog.md`](docs/devlog.md).

## v0.6.0 — 2026-08-20

**One conversion-naming convention: the arrow `->` (ADR-239).** Every conversion is
now spelled with the Scheme arrow. The polymorphic ability ops are `->string`
(`Display`), `->seq` (`Seqable`), `->iso` (`Temporal`), `->json` (`JsonEncode`);
module conversions are `string/->bytes` / `string/bytes->`, `string/->list` /
`string/list->`, `stream/->vector`, `pq/->list`, `queue/->list`, and friends; the
number formatter is `->fixed`. The redundant `number->string` (it was `str`) and
`symbol->string` are removed; `string->symbol` stays.

**Kernel primitives stay flat (dash) names.** A `/` in a name is *module-member*
syntax throughout the module system — `(:use mod)` refers a module's names by prefix,
the project loader `require`s a module per image section, and the image is sectioned by
splitting names on `/`. A kernel primitive is a flat global, not a member of a module,
so a `/`-named primitive whose prefix is not a real module (`map/get`, `vector/ref`,
`table/put`) breaks all three. `string/length` is fine only because `string` **is** a
module. So `map-*`, `vector-*`, and `table-*` keep their dash names; slash primitive
names are reserved for a real module-backed namespace. A guard test now enforces this
so a future violation fails at CI, not at deploy.

This is a **breaking** release for code using the removed/renamed conversion names —
`nest check` flags every one. A `blsp-rename` codemod (`scripts/ecosystem/`) applies the
rename with proper identifier boundaries.

## v0.3.11 — 2026-08-13

**`:seed` on `tcp-read-until`** — bytes the caller already holds, treated as if they had just
arrived as the first chunk. A protocol reading frame after frame off one stream ends each read
holding the surplus that arrived past the frame it wanted, so the next read starts with bytes
in hand — and those may contain the delimiter, or its first half. Without `:seed` a caller must
either rescan them itself (re-implementing the loop these combinators exist to replace) or lose
a delimiter straddling the boundary between what it holds and what arrives next. The seed is
fed through the same step an arriving chunk takes, so that boundary is handled by exactly the
arithmetic that handles every other one; a seed already containing a whole frame returns without
touching the socket. It counts toward `:max-bytes`. From hatch again: its HTTP worker re-enters
the head read with the leftover of a pipelined request, which is precisely this case, and it was
the last thing keeping that read on a hand-rolled loop.

## v0.3.10 — 2026-08-13

**Namespaces: more than one module per file (ADR-223).** A file is now a sequence of
regions, each opened by a `defmodule`, so a small helper module no longer needs its own
file. A co-located secondary module is reachable by name via `require`, including from a
nameless project or a bare `brood run` of a single file.

**Co-located tests (ADR-225).** `describe`/`test` forms can live beside the code they
cover; they are discovered by form and stripped when the project ships, so a library
carries its tests in-tree without carrying them to its consumers.

**Execution is a tier ladder, not a choice of engine (ADR-222/ADR-224).** Evaluation
climbs tiers up to a configurable ceiling rather than picking one engine, `enum Engine`
and a `JitBackend` contract replace the ad-hoc seams, and a compiled match arm is reached
through a process-local handle instead of being re-resolved. Plus two `fold` wins —
folding a vector in a native counted loop, and testing vector first in the dispatch —
worth −19% and −4.6% CPU respectively on the spawn-live benchmark.

**`:deadline-ms` on the framed reads.** `tcp-read-until` / `tcp-read-n` take a third,
optional bound: a **total** wall-clock budget for the frame, resolved once at the call and
never reset, joining the idle `:timeout-ms` and the size cap `:max-bytes`. It closes the
gap the other two leave open — a peer drip-feeding one byte per (idle − 1)ms re-arms the
idle timeout forever, and `:max-bytes` bounds only the size that drip reaches, never the
time, so a worker can be held for `max-bytes × idle`. Reported by
[hatch](https://github.com/broodlang/hatch), whose HTTP head reader had hand-rolled
exactly this defense in all four of its read loops and so could not adopt the combinators
without it. The per-chunk wait becomes `(min idle remaining)`; an expired deadline returns
the existing `[:timeout acc]`, since "did not arrive in time" is one 408 either way. Off
by default, like the other two.

**Packaging.** `:kind` classifies a package as an app or a library; `installed-enhancers`
discovers `:enhances` packages at runtime, with a runtime install API behind it; a package
may no longer be named after a standard-library module; and the lock file sorts
deterministically instead of churning.

Fixes worth calling out:

- **`defdyn` marks survive an imaged start** (image format v5). An imaged start restored a
  `defdyn` global's value but skipped the module load that ran the `defdyn`, so the
  dynamic-var mark was missing and `binding` on it raised *"not a dynamic variable"*. The
  dynamic-var names are now recorded in the image and re-marked on open. Surfaced by the
  hatch suite (which images `*bml-source*`) going red on *repeat* imaged runs — a cold pass,
  then 38 failures — which is a failure mode CI starting from a clean checkout never sees.
  MAGIC bumped v4 → v5 so a v5 reader rejects a v4 image rather than misreading its footer.
- **No SIGABRT on a broken pipe** in `nest check` / `brood` output (`… | head` no longer
  aborts).
- **Forward-ref names are scanned from the first `defmodule`**, not the whole file — the
  region model's companion fix.
- **A docsite code example indented in a docstring renders as a `<pre>`**, not a run-on
  paragraph.
- Editor: **partial read-only spans** (ADR-219), with a namespace-aware `defonce` and the
  imaged-registry fix underneath it.

Also: `reduce-while`, the early-terminating fold, joins the prelude.

## v0.3.9 — 2026-08-08

New compression capabilities, both surfaced by [hatch](https://github.com/broodlang/hatch)
adopting brood's compression for HTTP responses:

- **An optional compression level for the zlib encoders.** `zlib/gzip` / `zlib/compress` /
  `zlib/zip` (and the `%gzip` / `%zlib-compress` / `%deflate` prims) now take a level `0..=9`
  (0 = store, 9 = best; default 6, unchanged). Reach for 9 when a compressed form is written
  once and served many times (a precompressed static asset); the default suits per-request
  work. An out-of-range level is a clean error, not a silent clamp; decoders are unchanged.
- **Brotli compression (`Content-Encoding: br`).** A fourth format beside gzip/zlib/deflate:
  `zlib/brotli` / `zlib/unbrotli` (the `%brotli` / `%unbrotli` prims, over the pure-Rust
  `brotli` crate). The encoder takes an optional quality `0..=11` (default 5 — a balanced point
  for per-request work; a static asset built once passes 11). Brotli beats gzip on text and is
  the coding a modern browser prefers.

Three gaps found by reviewing hatch, the Brood web framework, against 0.3.8 — each a feature
that had not yet met its first real consumer:

- **A `table` global no longer locks a project out of the startup image.**
  `%image-write` refused any value with no portable form, and a table handle is
  per-runtime, so one `(def *cache* (table))` forfeited the ADR-218 image for the
  *whole* project — which then reloaded from source on every start. Since `table`
  is the language's only sanctioned mutable structure (ADR-026/107), the blessed
  way to hold shared state was also the way to lose imaged startup. A table is now
  imaged **by value** (its snapshot) and rebuilt as a fresh table on restore, so
  load-time contents survive. Confined to a top-level binding, as `Value::Macro`
  already is; two globals aliasing one table raise, naming both. Image format
  bumped to **v4** (a v3 reader would bind the global to the snapshot map).
- **`tcp-read-until` / `tcp-read-n` take limits**, so a hardened server can use
  them: `:timeout-ms` (an **idle** wait, reset per chunk) and `:max-bytes` (a cap
  on the frame), both off by default. New tagged returns `[:timeout acc]` /
  `[:too-large acc]` join `[:closed acc]`, so a caller can distinguish 408 from 413
  from "peer hung up". `tcp-read-n` checks the cap against the *declared* length
  before reading, so an absurd `Content-Length` is refused rather than buffered.
  Without these the combinators could not replace a server's own read loop —
  hatch declined to adopt them for exactly that reason.
- **`nest format` only formats what the project owns.** It walked every `.blsp`
  under the root minus an ignore list, which reached `_deps/<pkg>/**` — a fetched
  dependency's source, which the author cannot edit and `nest fetch` regenerates.
  It now walks a **whitelist**: `:source-paths` + `:test-paths` + a new
  **`:format-paths`** manifest key, plus the root's own top-level `.blsp`. A tree
  of authored-but-not-built Brood must now be declared (this repo lists `std`,
  `examples`, `scripts`, `stress`, `breakage`).

## v0.3.8 — 2026-08-07

Review fixes for the doc/wasm batch:

- **wasm `receive` timeouts no longer busy-spin.** `(after ms …)` woke the process but the
  gate re-checked the *real* clock (almost no time passed) and re-parked, so the pump spun at
  100% CPU for the whole real delay — freezing the browser tab. A `cfg(wasm32)` **logical
  clock** (`timer::sched_now`) now advances to the fired deadline, so the receive resolves at
  once (a 1 s timeout returns immediately). Native uses the real clock, unchanged.
- **`markdown->html` no longer runs away on an unmatched `[`** (`index-of` returns -1, not
  nil — the missing guard recursed on the same text and stack-overflowed). An unclosed ```
  fence at end-of-input is now emitted rather than dropped, and a guide link's URL is
  attribute-escaped with `javascript:` neutralised.
- **`nest doc` (Markdown) uses namespace attribution** (`project-file-feature`), like `nest
  docs`, instead of a `global-names` load-delta — which mis-credited a module already bound
  (transitively loaded, or materialised from an ADR-218 startup image).

## v0.3.7 — 2026-08-07

- **`nest doctest`** — a new subcommand that evaluates every `expr ;=> result` example in
  the project's docstrings and checks it still holds, so a documented example can't silently
  drift from the code. Prints a line per example and exits non-zero on any mismatch (CI-
  ready). Scoped to the project's own globals; `;=>` never appears in a builtin/prelude
  docstring, so nothing else is picked up.
- **Guides in `nest docs`.** A `guides/*.md` file becomes a narrative page in the generated
  site, alongside the API reference (in the sidebar and rendered from a small Markdown subset
  — ATX headings, fenced code, `- ` lists, paragraphs, inline `code`/links). The
  guide-vs-reference split ExDoc has, from plain Markdown files, no manifest wiring.

## v0.3.6 — 2026-08-07

- **`receive` timeouts run under WebAssembly.** `(receive … (after ms expr))` used the OS
  timer thread, which wasm has none of. The cooperative pump now fires the earliest pending
  deadline when nothing else is runnable (logical time — real delays aren't honoured, which
  is fine for a playground); messages still win over a timeout, since the pump drains the run
  queues first. Behind `cfg(target_arch = "wasm32")`; the native timer thread is unchanged.
- **`doc-catalog` recategorisation.** Reflection predicates (`bound?`, `dynamic?`, `private?`,
  `satisfies?`) → *Modules and reflection*; the ETS-style shared store (`table-*`) →
  *Processes and concurrency* (it's a concurrency primitive, not an immutable map);
  record/protocol multimethod seams (`num-add`, `ord-compare`, `to-str`, `to-seq`, `-conj`,
  …) → *Modules and reflection*; tty tests (`stdin-tty?`, `stdout-tty?`) → *System*.

## v0.3.5 — 2026-08-07

- **Green processes run under WebAssembly.** `spawn`/`send`/`receive` used to trap in the
  browser (the scheduler starts a pool of OS threads, which wasm can't). On `wasm32` there
  are now no worker threads: the run queue is driven cooperatively on the single thread by a
  `pump_until_quiescent` sweep, and the top-level program runs as a green process whose
  result is rendered across the process-heap boundary (`run_program_repr`) so the playground
  can show it. Everything is behind `cfg(target_arch = "wasm32")`, so the native scheduler is
  byte-for-byte unchanged. (`now_nanos` moves to `web_time::Instant` — plain
  `std::time::Instant::now()` panics on wasm.) The in-browser playground and the runnable doc
  examples can now run concurrency snippets.

## v0.3.4 — 2026-08-07

- **`doc-catalog`** — a new CORE module mapping every public builtin/prelude function to a
  functional category (Math, Strings, Filesystem, Processes, …) plus the category order and
  titles. `nest docs --all` now emits the reference **grouped by category** instead of one
  flat list, and a shipped app (hive's `/reference`) requires the same module — so the CLI
  and hosted language reference are categorised identically from one source.

## v0.3.3 — 2026-08-07

- **`nest docs`** — a new subcommand that generates a browsable HTML documentation
  site from a project's docstrings (`doc/index.html` + `doc/model.json`); `nest docs
  --all` documents the whole builtin + prelude reference (the language reference).
- **`docsite`** — a new CORE module: a pure `model -> HTML` renderer (sidebar,
  per-module sections, signatures/types/docstrings, a client-side filter) shared by
  `nest docs` and any app that hosts docs (the styles are scoped under `.docsite` so a
  `:wrap? false` fragment embeds in a host page; the host dictates light/dark, only the
  standalone page follows the OS).
- Per-module attribution in the doc model is by namespace (via `project-file-feature`,
  accounting for ADR-070 project-name rooting), not a load-order-sensitive
  `global-names` delta.

## v0.3.0 — 2026-08-06

A maintenance release: test-runner robustness and tooling, no language or
runtime behaviour changes since 0.2.0.

- **`nest update-tooling`** — a new subcommand that re-drops the AI-assistant
  files `nest new` scaffolds (the `docs/brood-for-claude.md` reference and the
  `writing-brood` skill) from the current binary, so they don't drift as the
  language evolves. Guarded against a nil project root, and works from a
  subdirectory.
- **KI-29** — a killed test binary no longer orphans its `brood` children; the
  test harness reaps the child OS-processes it spawns.
- **KI-30** — seven temp-dir prefixes that were never purged (leaking ~168 MB of
  `/tmp` per full suite run) are now cleaned up.
- **Privacy/LSP** — review follow-ups on the ADR-146 step-2 def-site privacy
  migration; `nest doc` no longer leaks private definitions.
- Docs currency pass and perf-measurement work (both `spawn-live` "next levers"
  measured and declined — the arms already reach native code).

## v0.2.0 — 2026-08-05

- **Def-site privacy migration (ADR-146 step 2)** — `defn-`/`def-` for private
  definitions; the older `--` naming convention was removed.
- **Runtime work (ADR-213/214/215)** — shared compiled code across a runtime's
  processes and related scheduler/message-path improvements.

## v0.1.0 — 2026-08-02

- First tagged release of the Brood toolchain: the language core (immutable
  Lisp, macros, pattern matching, modules, CHAMP maps), the green-process
  runtime with distribution, the closure-compiling VM + tier-1 JIT, the
  set-theoretic advisory type checker, the self-hosted REPL, `nest` project
  tooling, and the `brood-lsp` language server.
