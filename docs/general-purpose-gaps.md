# Brood as a general-purpose language — what is missing

**Status 2026-11-06, checked against the tree at v0.50.0:** gaps 3, 4, 5, 6 and 7 carry their
2026-10-26 updates; gap 2 is updated below; gap 8 was no gap. Open: gap 1 (no policy or 1.0
yet), the release half of gap 2, build-on-fetch and a C API (gap 3), the rest in the package
lists, and gaps 9 and 10.

**Status: research note, 2026-10-25, read against `origin/main` at `288dfe03` (v0.43.0 + Unreleased).**
Nothing here was run; every claim cites the file it was read from. Claims marked
*unverified* could not be checked from the tree.

## The question

ROADMAP.md opens by calling Brood "a general-purpose language and runtime". If someone who
writes Go, Rust, Zig, Node/Deno, Java/.NET, Python or an actor-runtime language picked it up for a service, a
CLI or a desktop program today, what would they reach for and not find? This note lists
those gaps in the order they would stop someone from adopting it, not in order of how
interesting they are to build.

Each gap says what is missing, where that was checked, what "done" looks like, and which
side of the line it falls on. **Kernel/language** means a new primitive or a change to a
contract, and needs an ADR. **std** has to pass ADR-417's three questions: it is a
mechanism, something besides the editor needs it, and it has to version with the kernel.
**hive** means a package: a format or protocol library that other programs want but that
does not version with the kernel.

## What Brood already has

Some of this is ahead of most of the reference languages. A gap list should not hide that.

- **A mature actor runtime's process model, with a JIT.** Preemptive M:N scheduling with work stealing and
  live migration (docs/scheduler.md, ADR-100), and preemption inside native code since
  ADR-400..403. Links, monitors, selective receive, mailbox bounds (ADR-307), supervisors,
  `gen`, `defstatem` (ADR-437), `defapp` with signal-driven shutdown (ADR-435/436) and a
  local registry (ADR-445) and cluster-wide process groups (ADR-446). Encrypted multi-node
  distribution (docs/distribution.md). In the cross-language benchmark it has the
  second-best p99 latency in the field, behind the reference actor runtime and ahead of Go
  (brood-benchmark/README.md).
- **Live redefinition is the core design.** Redefining a function reaches every process on
  its next call. Startup images (ADR-218/314) and hot reload (docs/shared-code.md).
- **A real numeric tower.** Bigints widen on overflow, ratios are exact (ADR-196), decimals
  have an `M` literal, and floats are opt-in through `->float` (docs/language.md).
- **Unicode done properly.** Graphemes, display width and NFC/NFD/NFKC/NFKD
  (`crates/lisp/src/builtins/string.rs`). Strings are deliberately not seqable (ADR-159).
- **One toolchain.** `nest` covers new, run, test (property tests, coverage, doctests),
  check, format, doc, publish, rename (a codemod), ws, observe and mcp
  (`std/tool/nest.blsp`). There is an LSP with most of the protocol (docs/lsp.md), an
  Emacs mode, a VS Code extension and a tree-sitter grammar (sibling repos).
- **A package manager with a real solver.** PubGrub resolution (ADR-209), sha256 locks,
  ed25519 signing with trust on first use (ADR-212), and a hosted registry (ADR-211).
- **Single-binary releases with no Rust toolchain needed** (`nest release`, docs/release.md),
  and a wasm32 build that runs in a browser tab (CI job "wasm32").
- **A sound, advisory, set-theoretic type checker** with occurrence typing, record and tuple
  unions, and exhaustiveness over literal enums (docs/type-system-status.md).

## Gaps, ranked by how much they block adoption

### 1. There is no stability promise

**Update 2026-11-09:** ADR-673 (accepted 2026-11-10); the work to make it true is
[`road-to-1.0.md`](road-to-1.0.md).

**Missing.** README.md says the language, the standard library, the tooling and the
on-disk formats "can change dramatically (and break) without notice or migration path …
don't build anything you depend on against it yet." The 1.0 freeze gate is closed
(ADR-170), but there has been no 1.0. CHANGELOG.md runs from v0.1.0 (2026-08-02) to v0.43.0
in about twelve weeks and says "breaking" 24 times. ADR-417 removed ten std modules in one
release. A team will not adopt a language on those terms, however good it is.

**Already there to build on.** `:since` / `:deprecated` / `:beta` metadata (ADR-283), the
rename ledger with `nest rename` (`std/tool/codemod.blsp`, `std/tool/renames.blsp`), the
`:brood "<constraint>"` runtime gate (ADR-209), and docs/ecosystem-playbook.md, which already
treats a breaking change as a minor bump.

**Done looks like.** A written compatibility policy that says what is covered: the
language, std's public names, the `nest` CLI, the lock format, the image format, and
whether the dist wire format is compatible across versions. It also says how long a
deprecation lasts before removal, and that every removal ships a codemod. Then a 1.0 that
means it.

**Kind.** Policy, recorded as an ADR. No code. It is first because nothing below matters
until it exists.

### 2. Platforms: tested on Linux only

**Update 2026-11-06: the test half is done, the release half is not.** CI runs the platform
smoke set on macOS and Windows (`macos-check`, `windows-smoke`), and `windows-smoke` has been a
gate since 2026-11-04. Paths and program lookup were fixed on the way (ADR-592, ADR-607). Those
are smoke sets, not the whole suite. `release.yml` still builds the same four targets, no
Windows and no musl, and `nest release --target` still takes a runtime copied in by hand.

**Missing.** `.github/workflows/ci.yml` runs the tests on Linux only. macOS is
`cargo check` only ("the target only a tag used to reach"). On Windows the tree compiles,
but the job that runs a program has `continue-on-error: true`: "Whether the runtime RUNS on
Windows, which nothing has yet shown". `release.yml` builds four targets: x86_64 and
aarch64 Linux (gnu), and aarch64 and x86_64 macOS. It builds no Windows and no musl binary.
`nest release --target` only appends to a runtime the user has built on that target and
copied into `~/.cache/brood/runtimes/` by hand. Cross-compiling the runtime is out of scope
(docs/release.md, ADR-038). Go and Zig cross-compile from any host, and Deno ships its
runtime for every target.

**Done looks like.** The test suite runs on macOS and Windows in CI. The Windows smoke job
becomes a gate. release.yml also publishes Windows and musl runtimes, and
`nest release --target` downloads the matching runtime for the installed version, checking
its hash, instead of asking the user to scp one into place.

**Kind.** Kernel and build infrastructure. Downloading the runtime changes ADR-038, so it
needs an ADR.

### 3. Native interop: no C FFI, and the WASM route is half built

*Checked 2026-11-06: as the update below says. Build-on-fetch and a C embedding API are still
open; `crates/lisp` builds no C library.*

**Update 2026-10-26: the WASM route is built.** A guest gets WASI preview 2 by grant
(`:dirs`, `:env`, `:args`, `:stdio`; never the network), interface exports are callable, a
guest resource is a token, and enums, variants and records marshal both ways (ADR-453). A
package ships its component with `include-bytes`, pinned by the package's own hash, so no
`:native` clause was needed (ADR-454). The test passed: SQLite runs as a component from a
package (`broodlang/sqlite`, not yet on hive). Build-on-fetch and a C embedding API remain.

**Half the usual complaint.** There is no C FFI, and that is deliberate.
docs/interop.md rejects a native `.so` ABI because it would be tied to the kernel's `Value`
layout, could crash the runtime and would reopen ADR-037's supply-chain hole. The answer
it chose is WebAssembly components, and slices 1 and 2 have shipped (ADR-145): wasmtime is
embedded, WIT-typed calls go through `std/wasm.blsp`, `bytes` are marshalled, and calls are
fuel-metered. `libloading` is used only to load tree-sitter grammars
(`crates/lisp/Cargo.toml`).

**Missing.** A guest has no WASI. `host/wasm.rs` says "no WASI wired in", so a component
cannot touch files, the network or the clock. Guests cannot hold `resource` handles. There
is no `:native` manifest clause, lock entry or build-on-fetch, which docs/interop.md calls
"shovel-ready" but deferred until a consumer exists (`std/tool/package.blsp` has no
`:native`). In practice, any native capability, such as a new hash, a codec or SQLite,
still means a Rust builtin and a new runtime. Embedding works the other way round only
from Rust: `Interp::{new, eval_str, eval_source, print}` (`crates/lisp/src/lib.rs`). There
is no C ABI, no `cdylib`, and no API for a host program to register its own functions.

**Done looks like.** WASI preview 2 with grants declared in the manifest. Guest resources
as opaque handles. `:native` with `:wasm-build` and `:wasm-artifact`, hash-pinned in the
lock. The test is SQLite running as a component from a hive package (gap 5). Separately, a
small C embedding API (create a runtime, evaluate, call a function, register a host
function), so Brood can be a scripting language inside other programs, as Lua and
Guile are.

**Kind.** Kernel. ADR-071/145 cover the WASM work. A C embedding API needs a new ADR.

### 4. Networking stops at HTTP/1.0 over TCP

*Checked 2026-11-06: `std/net/dns` still runs over TCP, so the UDP resolver below remains.*

**Update 2026-10-26: the kernel half is done.** UDP sockets deliver to the mailbox like TCP
(`std/net/udp`, ADR-449); `%tls-connect` / `%tls-listen` negotiate ALPN and `tls/protocol`
reads the result; `http/connection` + `http/request-on` are an HTTP/1.1 keep-alive client
with chunked decoding (ADR-451). HTTP/2, gRPC and a UDP DNS resolver remain, as packages.

**Worse than it looks.** `std/net/http.blsp` speaks HTTP/1.0, client and server,
with no keep-alive and no chunked encoding ("deliberately small"). SSE is HTTP/1.0 for the
same reason (`std/net/sse.blsp`). The kernel has **no UDP** (`std/net/dns.blsp`;
`builtins/modules.rs:395`), so DNS runs over TCP and returns A and AAAA records only.
There is no ALPN in the TLS primitives (no match in `crates/lisp/src`), so HTTP/2 cannot
even be negotiated. HTTP/1.1 and WebSocket do exist, but downstream in hatch
(`hatch/src/http/response.blsp`, `hatch/src/http/websocket.blsp`). Nothing provides
HTTP/2, HTTP/3, gRPC or QUIC.

**Done looks like.** UDP sockets in the kernel, delivered to the mailbox the same way TCP
is (ADR-062). ALPN and client certificates in `%tls-*`. In std, an HTTP/1.1 client with
keep-alive and chunked decoding, because `nest` downloads tarballs and every program
calls APIs. hatch's server stays the server. HTTP/2 and gRPC become hive packages once
ALPN exists.

**Kind.** UDP and ALPN are kernel mechanisms and need an ADR. An HTTP/1.1 client passes
ADR-417 because the toolchain is its second user. HTTP/2, gRPC and WebSocket clients are
hive packages.

### 5. Data access: Postgres only

**Update 2026-10-26: SQLite exists**, as the `sqlite` package: SQLite 3.50 compiled to
`wasm32-wasip2`, a connection process per handle, values both ways, transactions. It runs
without file locks (WASI has none), so one handle per database file. Redis and MySQL remain.

**Verified.** std has no database drivers, and that is correct under ADR-417.
`store-postgres` is a Postgres wire driver and pool written in pure Brood, with SCRAM and
the extended protocol, behind `store`'s adaptor contract (`store-postgres/README.md`).
There is no SQLite. It cannot be written in pure Brood and is blocked on gap 3. MySQL,
Redis and the message brokers (Kafka, NATS, AMQP) were not found in the sibling
repositories. Whether they exist on the live registry is *unverified*.

**Done looks like.** SQLite as a WASM component package (after gap 3), and Redis and MySQL
as pure-Brood packages over `tcp`'s binary mode, following store-postgres's pattern.

**Kind.** hive packages. The only kernel dependency is gap 3.

### 6. The library ecosystem is small, and there are std holes in time and crypto

**Update 2026-10-25: the two std holes are closed.** Time zones are `std/tz` (zoneinfo and
POSIX rules); ECDSA, RSA, X25519 and Argon2id are in `std/crypto` (ADR-448). The ecosystem
part below stands.

**Missing.** The first-party packages in the sibling repositories are hatch, store,
store-postgres, s3, mail plus two adapters, observe plus two adapters, b2d and four bedit
themes. That is about a dozen. The live registry count is *unverified*. Two holes sit at
std's level rather than in the ecosystem:

- **No time zones.** `std/datetime.blsp`: "There is no zone *type* and no zone database …
  Rendering is always UTC." Go, Java, .NET, Python (`zoneinfo`) and Temporal all ship IANA
  zones.
- **Thin public-key crypto.** `std/crypto.blsp` has ChaCha20-Poly1305, PBKDF2, HMAC,
  SHA-1/2, BLAKE3 and ed25519 (`keypair` / `sign` / `verify`). It has no RSA, ECDSA/P-256,
  X25519 or argon2/scrypt, so JWT RS256/ES256, OIDC, and SSH or WebAuthn verification all
  need native code.

YAML, TOML, protobuf and msgpack are absent from std, which is correct by the rule. Whether
any of them is on hive is *unverified*.

**Done looks like.** IANA zones: the zone engine in std, with tzdata as a separately
updatable hive data package. RSA, ECDSA and X25519 primitives next to ed25519. Then
protocol and format packages as programs ask for them.

**Kind.** Crypto primitives are kernel, since they need Rust or a WASM component. The time
zone engine is std, a mechanism that logging, `nest` and every service need. Formats are
hive.

### 7. Debugging and observability do not connect to the outside world

**Update 2026-10-26: done, bar publishing.** `nest dap` is a Debug Adapter Protocol server
over `debug` (ADR-452: function and line breakpoints, threads, stack, variables, evaluate,
continue); `nest profile` writes folded stacks (`dev/profile->folded`) that flamegraph.pl,
inferno and speedscope read; `observe-otlp` (OTLP/HTTP JSON: logs, errors as
`exception.*` log records, metrics) is written and tested as a package, not yet on hive.
W3C `traceparent` propagation is still open — `observe` has no traces to propagate.

**Missing.** There is no Debug Adapter Protocol. `std/tool/debug.blsp` speaks its own wire
protocol, a process-native debugger that is better than a breakpoint in several ways, but
no editor other than bedit can drive it. `std/jsonrpc` mentions DAP only as a possible
framing. Profiling has a sampling CPU profiler (`profile-start` / `profile-stop`) and
`perf/report`, whose counters need a `--features perf-stats` build
(`std/tool/perf.blsp`). It has no flamegraph, pprof or speedscope export, and no
`nest profile` command. Telemetry is good inside the process (`std/telemetry.blsp`,
aggregators, `system-monitor`), and the `observe` package ships logs, metrics and errors to
AppSignal. There is no OTLP / OpenTelemetry exporter and no trace context propagated over
HTTP (W3C `traceparent`). ROADMAP's observability item is still 🟡 (`defevent` schemas, the
remote tier).

**Done looks like.** A DAP server that translates DAP into `debug`'s existing wire
protocol, so VS Code, Neovim and Zed can set breakpoints, read paused processes and
evaluate in their scope. `nest profile` writing folded stacks or pprof. An `observe-otlp`
provider.

**Kind.** The DAP adapter is std (`std/tool`): it versions with `debug`'s wire, and an
editor other than bedit needs it. Profile export is std tooling. OTLP is a hive package.

### 8. The error convention: decided, and enforced — this gap was misread

**Update 2026-10-26: there was no gap; the doc read a stale line.** The first draft of this
section took ROADMAP's "`[:ok v]` / `[:error reason]` … deliberately not started" (written
2026-08-30) as current. Two days later ADR-310 decided the question the other way, after
measuring tuples, raising and a falsy failure on the tree's real call sites: **a function
that cannot interpret its input returns a `failure`**, a value kind of its own, and a raise
stays the channel for bugs. Tuples lost because they are "structurally at war with `->`"
and need a parametric `(result T)` in the checker; `result` / `unwrap` were rejected by
name. ADR-315 added the mechanisms that stop on a failure — `ok->` (the pipe), `with` (the
`let`) and a fold that stops once its accumulator is one — and the checker already enforces
it: `failure` is a tag, so `(or T failure)` is an ordinary union, a failure handed to a
function wanting `T` is a type finding, and a `failure?` that can never be true is reported.

That is Go's `error` and Rust's `Result` + `?` in a form that survives a pipe. What was left
was the tree not yet following its own rule: `tempo/new`, `tempo/parse` and
`tempo/parse-span` still answered `[:ok …]` / `[:error …]` (ADR-310 named tempo as the case
it was fixing), and so did `resolver/resolve`. All four return a failure now; the `!`
siblings still raise. That is enforcement the tuples never had: `(tempo/resolution (tempo/parse s))`
now reads `argument 1 expects tempo, got failure | tempo` in `nest check`, where the tuple
was silent. The `[:ok …]` tuples left in std are **process replies** — a
message's tag, a protocol rather than a parse result — which ADR-310 does not cover and
should not.

**Kind.** None left. A newcomer's habit of reaching for tuples is a documentation matter,
and `docs/brood-for-claude.md` and the writing skill already teach the failure channel.

### 9. Cost: memory per runtime and per process, and compute speed

**Status 2026-10-26: open, and measurement-bound.** What is left on runtime-frontier's
queue is M2 (shared IC tables, the largest per-process item at 664 B — a lock-free design that
needs TSAN/loom) and then M3/M4/M6/M7. Each is gated on benchmark runs, which happen on the
test machine, not here.

**Measured in brood-benchmark/README.md, 2026-09-16.** Base RSS is 52.8 MB, against Go 1.8,
Python 9.8 and Node 42.8. Holding 300k live processes takes 1.31 GB, against the
reference actor runtime's 924 MB. A tree of 20k supervised children takes 491 MB, against its
153 MB. Compute is 7.9× C on the geomean of the shared rows, between Node (5.3) and the actor
runtime (10.4). Unboxed
numeric arrays are still listed as "immutability-fraught" (docs/runtime-frontier.md §D),
which rules Brood out for numeric work.

**Done looks like.** The open items in docs/runtime-frontier.md (shared IC tables M2, one
memory block per process M7), a lean runtime without the GUI (docs/release.md notes that
every release carries about 4 MB of GUI), and an answer on numeric arrays, perhaps
immutable unboxed vectors behind a `bytes`-like handle.

**Kind.** Kernel. Unboxed arrays need an ADR.

### 10. Very large codebases, and an advisory type system

**Status 2026-10-26: the scaling half is done; the numbers below are the before.** The
module-index cache (ADR-380), an incremental check (ADR-382), a parallel walk (ADR-386,
11× on 12 cores), O(changed) image rebuilds (ADR-419) and a parallel cold load (ADR-426, the
default) are all in. The check's memory spike is fixed (0.2 GB over the load floor,
sequential). Left: the parallel arm's +0.7 GB, and re-timing the 2026-10-23 load-path work
on the test machine (docs/large-project-scaling.md, the queue).

At 3.07M lines a cold `nest check` takes 161 s and 3.8 GB on one thread, and a cold
`nest run` takes 39 s (docs/large-project-scaling.md). Warm runs are fine, and the doc
already holds the queue (kernel tooling). Separately, the checker is advisory by design: it
never gates the live image, globals stay `dynamic()` because of hot reload, `sig` covers
787 of 3357 std `defn`s, and type variables are at slice 2 (docs/type-system-status.md,
docs/type-variables.md). That is a choice, not a defect, but a Go, Rust or TypeScript team
will read it as one. The fix is documentation: say that a project wanting a gate runs
`nest check --strict` in CI.

## Corrections to the first, informal list

This note started as a quick list from skimming `std/`, README and ROADMAP. Checked against the
code, some of it was wrong:

- **"No single static binary story"** is wrong. `nest release` ships one binary without a
  Rust toolchain (ADR-038). What is missing is cross-*compiling* the runtime, a Windows or
  musl runtime in releases, and an asset filesystem (gap 2).
- **"No C FFI"** is right, but it leaves out that a WASM component interop path exists and
  partly works (ADR-145). The real gap is WASI, resources and `:native` packaging (gap 3).
- **"Roadmap defers keyword args, hygienic macros, true namespace isolation"** is stale on
  two of the three. The "Out of scope" list in ROADMAP.md is out of date. Namespaces
  shipped: `defmodule` namespaces, package-rooted modules (ADR-065/070) and enforced
  privacy (ADR-146). `&key` is refused, not deferred (ADR-163, in ADR-170's freeze list).
  Rationals, also on that list, shipped as ADR-196. Only full macro hygiene is still held
  back (ROADMAP "Held candidates", ADR-066), and auto-qualification in quasiquote covers
  most of it.
- **"WebSocket only downstream"** is right, but the larger fact is that std's HTTP is 1.0
  and the kernel has no UDP (gap 4).
- **Supply chain** is better than the list suggested. Lock hashes, ed25519 signing
  with trust on first use, and a structured conflict proof have all shipped. Still open: an
  enforced signing mode (docs/packages.md "Future work"), yanking, and a vulnerability
  advisory database. No `yank` or `advisory` was found in hive/src or std/tool/package.blsp.
  `std/tool/audit.blsp` audits documentation coverage, not security.
- **OpenTelemetry** is partly covered: telemetry and the `observe` package exist. What is
  missing is the OTLP exporter (gap 7).

**Smaller, unranked.** No locale-aware collation. No `nest` benchmarking command (like
`go test -bench`). The grammar is not in github/linguist. Marketplace or MELPA publication
of the editor extensions is *unverified*.

## Recommended order

1. **The stability policy (gap 1).** It costs only writing and unblocks everything else.
   Do it before 1.0.
2. **Platforms (gap 2).** Make Windows and macOS gates, and ship their runtimes. Without
   them, half the developers who would try Brood cannot.
3. **UDP, ALPN and an HTTP/1.1 client (gap 4).** Small kernel additions with a large reach.
4. **Finish WASM interop (gap 3).** WASI, resources and `:native`, proven by an SQLite
   package (gap 5). After this, the ecosystem can grow without a kernel release.
5. **DAP and profile export (gap 7).** They connect Brood's existing debugger and profiler to
   the editors and tools people already use.
6. **Time zones and crypto (gap 6)** — done; gap 8 turned out to be decided already.
7. **Ecosystem packages (gaps 5 and 6).** Redis, MySQL, OTLP, YAML/TOML, HTTP/2 and gRPC, in
   whatever order programs ask for them.
8. **Footprint and scale (gaps 9 and 10).** Continue the frontier docs' queues, which are
   measured and owned already.
