//! The module system's kernel: the catalogue of **baked-in** std modules
//! ([`CORE_MODULES`] always, [`DEV_MODULES`] under `dev-tools`), the namespace and
//! compile-context primitives (`%in-ns`, `%refer`, `%alias`, `%mark-private`,
//! `%register-sig`), and the bundle manifest a released binary answers from.

use crate::core::heap::{Heap, ImportEntry};
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::{arg, expect_symbol};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    // The side-fact journal (ADR-320), for the boot differential. `%registry-names` above
    // is one of the kinds this reports; it stays as its own primitive because
    // `std/tool/project.blsp` consumes it as symbols, while this is a rendered, sorted
    // fingerprint whose whole job is to be compared as text between two boots.
    primitives.def(
        "%side-facts",
        Arity::exact(0),
        Sig::new(vec![], any),
        &[],
        "Every SIDE FACT this runtime has recorded — what evaluating a definition recorded ABOUT a name rather than bound to it: privacy, stability meta, def site, registry-name marks, defdyn marks. One sorted string per fact, `\"<kind> <name> <payload>\"`. The boot differential compares these between an imaged and a source boot, so a fact kind that stops round-tripping fails at the boundary instead of surfacing as a distant symptom in another subsystem (ADR-320).",
        side_facts);
    primitives.def(
        "%builtin-module",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        &[],
        "",
        builtin_module,
    );
    primitives.def(
        "%builtin-module-file",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        &[],
        "",
        builtin_module_file,
    );
    primitives.def(
        "%builtin-doc",
        Arity::exact(1),
        Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)),
        &[],
        "",
        builtin_doc,
    );
    primitives.def(
        "reflect/builtin-modules",
        Arity::exact(0),
        Sig::new(vec![], list_ty),
        &[],
        "The names of every module baked into this binary, as a sorted list of strings — what a `name/…` reference or `(:use name)` resolves without a load-path. Backs `nest` shell completion and lets a name be validated before referencing it.",
        builtin_modules);
    // Release-bundle mechanism (ADR-038): an app produced by `nest release`
    // carries its source appended to the binary. These let `std/tool/project.blsp`
    // boot it; `%builtin-module` (above) already consults the bundle, so
    // `require` resolves an app's modules with no load-path change.
    primitives.def(
        "%bundled?",
        Arity::exact(0),
        Sig::nullary(bool_ty),
        &[],
        "",
        bundled_p,
    );
    primitives.def(
        "%bundle-manifest",
        Arity::exact(0),
        Sig::nullary(string.union(nil_ty)),
        &[],
        "",
        bundle_manifest,
    );
    primitives.def(
        "%bundle-module-names",
        Arity::exact(0),
        Sig::nullary(list_ty),
        &[],
        "",
        bundle_module_names,
    );
    // Namespaces (ADR-065): `%in-ns` sets the namespace being compiled into. The
    // `ns` macro (prelude) emits it; the resolver pass reads `heap.compile_ns`.
    primitives.def(
        "%in-ns",
        Arity::exact(1),
        // `(or symbol nil)` both ways. `in_ns`'s own comment states that nil is intended —
        // it "clears the namespace back to ROOT … the restore half of a save/set/restore
        // bracket", and `reflect/current-ns` answers nil at root precisely so a bracket can
        // save and put it back. The signature said `symbol`, which made
        // `%contracts-apply-pending!`'s restore read as a type error under
        // `nest check --strict` the moment `reflect/current-ns` stopped claiming it always
        // answers a symbol. The call was right; this was the lie.
        Sig::new(
            vec![sym.union(Ty::of(Tag::Nil))],
            sym.union(Ty::of(Tag::Nil)),
        ),
        &[],
        "",
        in_ns,
    );
    primitives.def(
        "reflect/current-ns",
        Arity::exact(0),
        // `(or symbol nil)`, not `symbol`: the docstring below has always said "nil at the
        // root namespace", and the signature did not. That is not cosmetic — the checker
        // read `(nil? (reflect/current-ns))` in `%defonce-qualified-name` as a test that
        // can never be true, i.e. it called a load-bearing guard dead. Found by the
        // impossible-predicate lint (ADR-315).
        Sig::new(vec![], sym.union(Ty::of(Tag::Nil))),
        &[],
        "The current compilation namespace as a symbol, or nil at the root namespace (top level).\n\n    (reflect/current-ns)   → nil",
        current_ns,
    );
    // The compile context — namespace AND `(:use …)` imports, both per process — as data,
    // and back. For an evaluator that runs each request in a fresh process and must carry
    // a `defmodule` from one request to the next (`std/tool/eval-server.blsp`).
    primitives.def(
        "%compile-context",
        Arity::exact(0),
        Sig::new(vec![], Ty::of(Tag::Vector)),
        &[],
        "",
        compile_context,
    );
    primitives.def(
        "%restore-compile-context",
        Arity::exact(1),
        Sig::new(vec![Ty::of(Tag::Vector)], sym),
        &[],
        "",
        restore_compile_context,
    );
    // Package-rooted namespaces (ADR-070): `%root-module-name` roots an intra-package
    // module reference to `prefix/name` under a dep load; `%set-package-context`
    // enters/clears a dep's load context. Emitted by the prelude loader + `defmodule`.
    primitives.def(
        "%root-module-name",
        Arity::exact(1),
        Sig::new(vec![sym], sym),
        &[],
        "",
        root_module_name,
    );
    primitives.def(
        "%set-package-context",
        Arity::exact(2),
        Sig::new(vec![any, any], any),
        &[],
        "",
        set_package_context,
    );
    // `(%refer 'mod subset exclude)` — populate the current file's import table from a
    // `(:use …)` clause. `subset` is nil (refer all public names) or a seq of bare
    // symbols (`:only`); `exclude` is nil or a seq of bare names to drop from a
    // refer-all (`:exclude`). The `defmodule` macro emits it after `(require 'mod)`.
    // `:use-internals` still emits the 2-arg form (exclude defaults to nil).
    primitives.def(
        "%refer",
        Arity::range(2, 3),
        Sig::new(vec![sym, any, any], nil_ty),
        &[],
        "",
        refer,
    );
    // `(%register-sig 'name 'type)` — record a user-declared `(sig …)` for the
    // checker, keyed by the module-qualified global name (resolved as `def` would).
    // The `sig`/`sig!` macros emit it; the checker's `sig_of` consults the store first.
    primitives.def(
        "%register-sig",
        Arity::exact(2),
        Sig::new(vec![sym, any], sym),
        &[],
        "",
        register_sig,
    );
    primitives.def(
        "%mark-private",
        Arity::exact(1),
        Sig::new(vec![sym], sym),
        &[],
        "",
        mark_private,
    );
    // `(:alias mod [:as short])` lowers to this — register a module alias so a later
    // `short/name` reference resolves to `mod/name`.
    primitives.def(
        "%alias",
        Arity::exact(2),
        Sig::new(vec![sym, sym], nil_ty),
        &[],
        "",
        alias,
    );
    // `(:use-internals mod)` lowers to this — module privacy's @testable seam
    // (ADR-146): grant this file access to `mod`'s `--` names.
    primitives.def(
        "%grant-internals",
        Arity::exact(1),
        Sig::new(vec![sym], nil_ty),
        &[],
        "",
        grant_internals,
    );
}

/// One baked-in std module: the `require` key, the source, and the **repo-relative
/// path the source came from**.
///
/// The path exists so a baked module's forms can be attributed to the file they were
/// actually written in. Without it they inherited whatever file happened to be loading
/// when the `require` ran (`%load-string` set none), so a 21-line `src/main.blsp` was
/// credited with `std/log`'s line 175 — in line coverage, and in `:trace` frames, which
/// take their file from the same `CompiledArm::src_file`.
///
/// [`embedded_module!`] derives `path` from the same literal as the `include_str!`, so
/// the two cannot drift: change the file a module loads from and its recorded path
/// follows.
pub(super) struct EmbeddedModule {
    pub key: &'static str,
    pub source: &'static str,
    pub path: &'static str,
}

/// `embedded_module!("log", "std/log.blsp")` — one [`EmbeddedModule`], with the source
/// baked in from `path` and `path` kept as the recorded origin.
macro_rules! embedded_module {
    ($key:expr, $path:literal) => {
        EmbeddedModule {
            key: $key,
            source: include_str!(concat!("../../../../", $path)),
            path: $path,
        }
    };
}

/// Standard-library modules baked into the binary (like the prelude), so they load
/// from any directory with no file paths. The require / provide / load-path
/// *policy* is written in Brood (`std/prelude.blsp`, ADR-019); Rust only exposes
/// an embedded module's source here, via `%builtin-module` (ADR-006/008).
///
/// Split into [`CORE_MODULES`] (always baked in) and [`DEV_MODULES`] (only under
/// the `dev-tools` feature), so a `nest release` lean runtime
/// (`--no-default-features`) carries no test/observer/tooling/REPL source
/// (ADR-038, docs/release.md). `builtin_module` consults both.
const CORE_MODULES: &[EmbeddedModule] = &[
    // Output ports: the redirectable sink behind print/println — a port is a 1-arg
    // string sink, with `process-port`/`fn-port` + `with-out`/`with-err`. Pairs
    // with the prelude's `*out*`/`*err*` dynamic vars. Opt-in, no dependencies.
    embedded_module!("io", "std/io.blsp"),
    // Fuzzy (subsequence) string matching + ranking: `fuzzy-match` / `fuzzy-filter`,
    // the matcher completion UIs ride on. Pure Brood, no dependencies. Opt-in.
    embedded_module!("fuzzy", "std/fuzzy.blsp"),
    // The string-manipulation library (ADR-230): every op whose subject is a string —
    // trim/pad/case/split/join/replace + char-content conversions + `fill` (greedy
    // word-wrap to a column, formerly std/text.blsp). Pure Brood over the `string/*`
    // primitives. Loaded by the prelude itself (`(require-one 'string)`) because the
    // prelude's own get/path-*/fmt helpers reference `string/char-at` etc. by late
    // binding — so it is always present, yet still an ordinary module (doc tooling sees
    // the file; a project may `(:use string)`).
    embedded_module!("string", "std/string.blsp"),
    // The project tool, split by concern (all CORE: a released bundle boots through
    // `project-release/run-bundle`, and `nest run --check-boot` through `project-run`).
    embedded_module!("project", "std/tool/project.blsp"),
    embedded_module!("project-image", "std/tool/project-image.blsp"),
    embedded_module!("project-check", "std/tool/project-check.blsp"),
    embedded_module!("project-run", "std/tool/project-run.blsp"),
    embedded_module!("project-release", "std/tool/project-release.blsp"),
    embedded_module!("stdimage", "std/tool/stdimage.blsp"),
    // The rename ledger as Brood data (ADR-304) — a thin wrapper over `%renames`, so
    // `nest check --fix-renames` reads the table the runtime error reads.
    embedded_module!("renames", "std/tool/renames.blsp"),
    embedded_module!("coverage", "std/tool/coverage.blsp"),
    embedded_module!("complete", "std/tool/complete.blsp"),
    embedded_module!("nest", "std/tool/nest.blsp"),
    // `nest new` scaffolding (templates + new-project), split out of `project` so
    // the analysis half stays lean. `(:use project)` for *config-git-init*. Opt-in.
    embedded_module!("scaffold", "std/tool/scaffold.blsp"),
    // Identifier-aware whole-token rewrites over a project's .blsp sources — drives
    // `nest rename` for an ecosystem-wide rename that a plain sed would corrupt. Opt-in.
    embedded_module!("codemod", "std/tool/codemod.blsp"),
    // Fan a check/commit/push across every sibling Brood repo in the workspace — drives
    // `nest ws`, over `%os-cmd`/`run-process`. Opt-in, never in the prelude.
    embedded_module!("workspace", "std/tool/workspace.blsp"),
    // The package manager (ADR-037): resolves the manifest's :dependencies into a
    // lock file + load-path entries. Required lazily by `project-setup` only when a
    // project actually declares deps. Opt-in, never in the prelude.
    embedded_module!("package", "std/tool/package.blsp"),
    // TCP sockets (ADR-062): active-socket helpers + a spawn-per-connection
    // server over the non-blocking tcp-* primitives. Opt-in, never in the prelude.
    embedded_module!("tcp", "std/net/tcp.blsp"),
    // The file & filesystem library: whole-file/line I/O, directory walking, path
    // helpers — Brood over the fs primitives. Opt-in, never in the prelude.
    embedded_module!("file", "std/file.blsp"),
    // A minimal HTTP/1.0 server (ADR-062) over the tcp + file libraries — request
    // parsing, response rendering, a router, static files. Opt-in.
    embedded_module!("http", "std/net/http.blsp"),
    // JSON ↔ Brood data, written entirely in Brood (a recursive-descent parser +
    // encoder over the string primitives; the reader's `\u{}` escape is the
    // codepoint→char mechanism). Opt-in, never in the prelude.
    embedded_module!("json", "std/json.blsp"),
    // WASM component interop (ADR-071/145): load sandboxed native components,
    // call exports (marshalled by WIT types), `use-native` binding. Policy over
    // the `%wasm-*` primitives (feature `wasm`; without it the primitives are
    // unbound and requiring this module errors clearly). Opt-in.
    embedded_module!("wasm", "std/wasm.blsp"),
    // Teach-the-error + intent→idiom lookup (LLM-native errors): explain/error
    // (a stable E-code → summary/causes/fix/example) and find-pattern (an
    // intent → the idiomatic Brood pattern). Curated Brood data; backs the
    // `nest mcp` tools of the same names. Opt-in.
    embedded_module!("explain", "std/tool/explain.blsp"),
    // Supervised node auto-reconnect (dist self-healing): `watch` keeps a peer
    // link alive with exponential-backoff `(connect …)` retries; subscribers get
    // [:nodeup]/[:nodedown]. Pure Brood over connect/monitor-node/nodes. Opt-in.
    embedded_module!("reconnect", "std/net/reconnect.blsp"),
    // A DNS client over TCP: resolve a name to its A/AAAA records. Enumerating the
    // addresses behind a name is how a cluster finds its own members — Fly gives one
    // AAAA per machine at `<app>.internal`, a headless Kubernetes service one A per
    // pod — and Erlang has `:inet_res` for exactly this. TCP because the kernel has
    // no UDP and DNS over TCP is a required transport (RFC 7766), so it needs no new
    // Rust. Pure Brood over tcp. Opt-in.
    embedded_module!("dns", "std/net/dns.blsp"),
    // Server-Sent Events (text/event-stream): a client reader process that streams
    // events to a subscriber's mailbox (pairs with ui's `with-events`) + server-side
    // framing. Pure frame parsing + a thin IO loop over tcp; reuses http's URL/header
    // helpers. Opt-in.
    embedded_module!("sse", "std/net/sse.blsp"),
    // The process framework, bundled in the default install (ADR-085 amended —
    // batteries-included, not externalized). All three are ORDINARY modules with bare
    // (single-segment) namespaces, so a qualified call is `gen/call`, `supervisor/start`,
    // `agent/get`.
    //
    // `gen` — the gen_server-style actor framework (ADR-243: `defserver` / `spawn-server` /
    // `call` / `cast` / `stop`). It was briefly prelude-bundled and *bare* (`7cb796f0`),
    // which seized ten very generic global names — `call`, `cast`, `stop`, … — into the
    // un-redefinable reserved set (ADR-166), so `(def call …)` was refused outright. A
    // framework, however core, does not get to own `call`: it is a module like every
    // other, and its names live behind `gen/`.
    embedded_module!("gen", "std/proc/gen.blsp"),
    // Supervision — independent of `gen`, both over the same kernel primitives.
    embedded_module!("supervisor", "std/proc/supervisor.blsp"),
    // Process-backed state cell: start/get/update/get-and-update/cast/stop.
    // A thin Brood layer over spawn/send/receive for the common "stateful process" case.
    embedded_module!("agent", "std/proc/agent.blsp"),
    // Crash reports for processes nobody supervises (ADR-305): one system-monitor
    // subscriber printing each abnormal exit once per site. CORE, not dev-tools — a
    // released bundle arms it by default, the same as `brood file` and `nest run`.
    embedded_module!("crash-report", "std/proc/crash-report.blsp"),
    // Order a flat process-info snapshot as a parent→child forest (depth-tagged, DFS
    // by id). A pure, dependency-free transform — CORE, not dev-tools: it's shared by
    // the dev observer's tree sort *and* a shipped app's process list (bedit's
    // *Process List*), so a `nest release` binary needs it baked in.
    embedded_module!("proctree", "std/tool/proctree.blsp"),
    // Run a thunk off the current process with an optional timeout + cancel
    // (ADR-006): `task` (async, tagged-reply handle), `cancel`, and the
    // synchronous `await`. Pure Brood over spawn / receive / exit — the generic
    // version of the editor's hand-rolled async-eval watchdog. Opt-in.
    embedded_module!("task", "std/task.blsp"),
    // An async, safe logger (ADR-006): a `gen` process holding a list of
    // backends, each an `io` port + a min level + a formatter. Log calls are casts
    // (fire-and-forget = async); the one process serialises writes (no interleaving)
    // and isolates a backend crash. Opt-in, never in the prelude.
    embedded_module!("log", "std/log.blsp"),
    // Erlang :telemetry-style instrumentation (ADR-106). Handlers run in a dedicated
    // LISTENER process (emit is a fire-and-forget send), so a buggy handler can never
    // crash/hang the emitting process — only the listener, which a throwing handler
    // doesn't even do (caught + detached). The handler table is a `def`-rebound global
    // that survives a listener restart (ADR-013). `span` brackets a body with
    // :start/:stop/:exception events; `forward` runs handler work in your own process.
    // Opt-in, never in the prelude.
    embedded_module!("telemetry", "std/telemetry.blsp"),
    // Date and time utilities (UTC): epoch↔datetime conversion, ISO 8601
    // format/parse, arithmetic, calendar predicates. Pure Brood over `now`.
    embedded_module!("datetime", "std/datetime.blsp"),
    // Time as an interval, not an instant (adapted from Tempo, Apache-2.0): one
    // resolution-carrying value type whose span is half-open, Allen's thirteen
    // relations, and an interval-set algebra (union/intersection/difference/gaps).
    // Layered on `datetime` for the calendar arithmetic. Opt-in, never in the prelude.
    embedded_module!("tempo", "std/tempo.blsp"),
    // Run the examples in docstrings and report the ones that do not hold — a documented
    // example is a claim about behaviour, and a wrong one is invisible to every other gate
    // (the checker does not evaluate docstrings, no test covers prose). Backs the doctest
    // pass in `nest test`; `tests/doc_examples_test.blsp` gates `std/` with the same engine.
    embedded_module!("doctest", "std/tool/doctest.blsp"),
    // Hex and Base64 encoding/decoding. Pure Brood over `string/char->int` /
    // `string->utf8-bytes` / `utf8-bytes->string`. Opt-in, never in the prelude.
    embedded_module!("encoding", "std/encoding.blsp"),
    // Descriptive statistics over numeric sequences: mean, median, stddev,
    // variance, percentile, mode, frequencies. Pure Brood over sort/fold/sqrt.
    embedded_module!("stats", "std/stats.blsp"),
    // Pull-stream protocol + combinators over green processes. Sources: list,
    // fn-generator, range, TCP socket. Transformers: map/filter/take/drop/
    // take-while/chunk/concat/lines. Terminals: fold/to-list/to-vector/
    // for-each/pipe/to-socket. Foundation for the HTTP streaming layer.
    embedded_module!("stream", "std/stream.blsp"),
    // URL encoding/decoding and parsing: percent-encode/decode, query-string
    // encode/decode, parse-url, build. Pure Brood over string primitives.
    embedded_module!("url", "std/url.blsp"),
    // CSV parsing and emitting: csv-parse, csv-parse-maps, csv-emit,
    // csv-emit-maps. Handles quoted fields, escaped quotes, \r\n endings.
    embedded_module!("csv", "std/csv.blsp"),
    // RFC 4122 version-4 UUID generation via the OS CSPRNG (random-token).
    // uuid-v4, uuid-nil, uuid?.
    embedded_module!("uuid", "std/uuid.blsp"),
    // {{var}} string templating: render a template string against a data map.
    // render, render-all.
    embedded_module!("template", "std/template.blsp"),
    // The documentation-site renderer: a pure `model -> HTML string` for `nest docs`
    // and hive's per-package doc builds (both feed it the same doc-model shape). CORE,
    // not DEV, because a shipped app (hive) requires it at runtime to render docs.
    embedded_module!("docsite", "std/tool/docsite.blsp"),
    // The function catalogue: bare builtin/prelude name -> functional category, plus the
    // category order/titles. CORE so both `nest docs --all` and a shipped app (hive's
    // /reference) present the categorised language reference from one source.
    embedded_module!("doc-catalog", "std/tool/doc-catalog.blsp"),
    // Purely functional FIFO queue (two-list, amortised O(1)) and min-priority
    // queue (sorted-list, O(n) insert / O(1) pop).
    embedded_module!("queue", "std/queue.blsp"),
    embedded_module!("pq", "std/pq.blsp"),
    // Multi-valued map: one key may hold multiple values (a map of lists).
    // multimap-assoc, multimap-get, multimap-get-all, multimap-dissoc, …
    embedded_module!("multimap", "std/multimap.blsp"),
    // MD5/SHA-1/SHA-256/SHA-384/SHA-512 + HMAC, all Brood over the two `%digest`
    // / `%hmac` prims (raw bytes); hex/string shaping via bytes->hex; hash/string is djb2.
    embedded_module!("hash", "std/hash.blsp"),
    // gzip/zlib/raw-deflate compression
    // (gzip/gunzip, compress/uncompress, zip/unzip) over the six %gzip/%deflate prims.
    embedded_module!("zlib", "std/zlib.blsp"),
    // LCS-based sequence diff: diff-seq, diff-lines, diff-summary, diff-patch,
    // diff-unified. O(m*n) time/space; suitable for small-to-medium sequences.
    embedded_module!("diff", "std/diff.blsp"),
    // Path string manipulation: join, split, basename, dirname, extension, stem,
    // normalize, relative-to. Consolidates the prelude's path-* globals under
    // a single path/ namespace with additional operations.
    embedded_module!("path", "std/path.blsp"),
    // Sequence helpers (ADR-227): the derived enumeration API layered over the bare
    // collection protocol — group-by/frequencies/chunk-by/chunk-every, distinct-by,
    // scan/reduce-while, zip-with/interleave/interpose, min-by/max-by, enumerate/
    // index-where/dedupe. The core ops (map/filter/reduce/fold/take/drop/distinct/…)
    // stay bare in the prelude. `(:use enum)` for bare access, or call qualified.
    embedded_module!("seq", "std/seq.blsp"),
    // Map-transformation helpers (ADR-227): merge-with / update-vals / update-keys /
    // select-keys, layered over the bare map protocol (assoc/get/keys/vals/merge/…,
    // which stay in the prelude). The module is named `map`; the bare `map` function
    // is unaffected. `(:use map)` for bare access, or call qualified.
    embedded_module!("map", "std/map.blsp"),
    // The math library (ADR-227): sqrt / pow / ceil / round / round-to / clamp / abs /
    // sum / product / positive? / negative? / even? / odd? + the constants pi/e, and since
    // 2026-08-27 the rest of the derived arithmetic too — min / max / rem / quot / mod /
    // floor / ->fixed / numerator / denominator, plus the `Zero` and `Numeric` abilities
    // (zero? / nan? / infinite?). Only the OPERATORS `+ - * / < =` stay bare.
    //
    // Their kernel halves keep the `%` prefix every primitive has (`%max`, `%rem`, beside
    // `%add`/`%quot`), so the PRELUDE does arithmetic without loading a module — prelude
    // code cannot reference one. `resolve_prim` keys on the call-site NAME, so the `math/*`
    // wrappers still lower to their `PrimOp`. `(:use math)` for bare access.
    embedded_module!("math", "std/math.blsp"),
    // Bitwise ops on integers + the IEEE-754 float/bit reinterpretations. The operations
    // are kernel primitives registered as `bit/*` (the `string/length` pattern); this
    // module declares the namespace they live in. Bare until 2026-08-26 — ten names for
    // one idea is what a namespace is for, and bare is the scarce resource.
    embedded_module!("bit", "std/bit.blsp"),
    // Exact base-10 decimals — the `bit` story again: kernel primitives registered as
    // `decimal/*`, this module declares the namespace. `decimal?` stays bare with the
    // other type predicates. Bare until 2026-08-26.
    embedded_module!("decimal", "std/decimal.blsp"),
    // Arithmetic for RECORD types — the `+`/`-`/`*`/`/` extension point (ADR-179). The
    // `bit`/`decimal` story with one twist: the ops are neither Rust nor loadable, they are
    // prelude MULTIMETHODS, because `%add`'s cold fallback calls them before any module can
    // load. This module declares the namespace and documents them. `num-add`… until
    // 2026-08-28 — a four-name hyphen prefix is a namespace spelled by hand (ADR-251).
    embedded_module!("num", "std/num.blsp"),
    // OS/process interface: env vars, argv, subprocess execution, OS type, halt.
    // Wraps the %env-all/%argv/%os-cmd/%os-type/%halt primitives with a clean API.
    embedded_module!("system", "std/system.blsp"),
    // Authenticated encryption (ChaCha20-Poly1305), PBKDF2 key derivation, secure
    // random bytes. Wraps the %chacha20-* and %pbkdf2-sha256-bytes primitives.
    embedded_module!("crypto", "std/crypto.blsp"),
    // The editor framework's buffer model (M2 Phase 1, ADR-045): an immutable
    // buffer over the rope primitives, opt-in, never in the prelude.
    embedded_module!("editor/buffer", "std/editor/buffer.blsp"),
    // The CLIENT half of the buffer-process protocol (ADR-134): the link record
    // + the pure push fold (echo suppression, splice transform over in-flight
    // edits, resync fallback) a subscriber uses to track a hosted document.
    embedded_module!("editor/buffer-client", "std/editor/buffer-client.blsp"),
    // The display/input seam (M3, ADR-046): `display` is the render-op protocol
    // (pure data constructors); `keymap` is the rebindable key→command dispatcher
    // shared by the line editor and the observer; `observer` is a process-viewer
    // built on them + the `term-*`/`gui-*` primitives. All opt-in, never in the prelude.
    // The shared named-face / theme registry (the counterpart to `keymap`): style
    // named once, referenced everywhere, restyled in one place. Required by `ui`
    // (so every ui-run app gets it) and the observer.
    embedded_module!("editor/face", "std/editor/face.blsp"),
    embedded_module!("editor/display", "std/editor/display.blsp"),
    embedded_module!("editor/keymap", "std/editor/keymap.blsp"),
    // Composable, runtime-reconfigurable behaviour layers over `keymap` (the
    // generic mechanism the editor's "modes" are built from; buffer-agnostic).
    // Opt-in, never in the prelude. See docs/layers.md.
    embedded_module!("editor/layers", "std/editor/layers.blsp"),
    // Structural (s-expression) navigation over the parse-source CST — reusable
    // Brood-code tooling (same tier as the formatter / LSP), not editor-specific.
    // (The text-mode/brood-mode *layers* built on it are editor policy and live in
    // the downstream editor app — brood-edit — not here.) Opt-in. (docs/layers.md)
    embedded_module!("sexp", "std/tool/sexp.blsp"),
    // A small backtracking regular-expression engine, pure Brood (literals, ., * + ?,
    // ^ $, [...] sets, \d \w \s, |, groups; no ranges/captures yet). Opt-in.
    embedded_module!("regex", "std/regex.blsp"),
    // ANSI / VT100 escape-sequence stripping for pipe output (CSI sequences + CR).
    // Used by bshell and compile to clean subprocess output before display.
    embedded_module!("ansi", "std/ansi.blsp"),
    embedded_module!("editor/ui", "std/editor/ui.blsp"),
    // Serve a `ui-run` app to remote frontends — the Emacs `--daemon`/`emacsclient`
    // model (ADR-090): the app runs on the daemon, a thin `attach` client paints
    // pushed frames + ships back keys. Pure Brood over `ui-run` + the node link.
    embedded_module!("editor/serve", "std/editor/serve.blsp"),
    // Emacs-style tiled window splits: an immutable binary layout tree + pure
    // pane/divider geometry + drag-to-resize over `:drag` mouse events (ADR-077).
    // Reusable editor toolkit (content-agnostic); the keybindings + payload are
    // editor policy. Opt-in, never in the prelude.
    embedded_module!("editor/pane", "std/editor/pane.blsp"),
    // FORM BUFFERS (ADR-199): generated text with editable regions in it — a shell's
    // input line, a commit message's help block, a tutorial's code boxes, a rebase
    // todo. Region algebra + `splice` (re-render, keep what the user typed) + the two
    // `:post-key` guard policies (`:veto` / `:clamp`). Pure over text; opt-in.
    embedded_module!("editor/formbuf", "std/editor/formbuf.blsp"),
    // Bare ANSI escape *strings* for simple terminal scripts (`print` them
    // directly) — the lightweight counterpart to the `display` render-op
    // protocol. Opt-in, never in the prelude.
    embedded_module!("editor/ansi", "std/editor/ansi.blsp"),
    // The terminal seam (ADR-046): policy over the `%term-*` primitives, so raw-mode /
    // input polling / paint live under `term/*` rather than the bare language core.
    embedded_module!("term", "std/term.blsp"),
    // Native window seam (ADR-046/080): policy over the `%gui-*` primitives → `gui/*`.
    embedded_module!("gui", "std/gui.blsp"),
    // The rope text-engine seam: policy over the `%rope-*` primitives → `text/*`.
    embedded_module!("text", "std/text.blsp"),
    // A quantity as a person reads it → `humanize/*`. Tiny and dependency-free on
    // purpose: an editor's render path loads it without loading project tooling, which is
    // where the one public byte formatter used to live.
    embedded_module!("humanize", "std/humanize.blsp"),
    // Seeded PRNG (xorshift32): public face of the prelude's `%rand-*` mechanism → `rand/*`.
    embedded_module!("rand", "std/rand.blsp"),
    // OS & environment surface → `os/*` (getenv, run-process, now, …).
    embedded_module!("os", "std/os.blsp"),
    // Sound output → `audio/*`.
    embedded_module!("audio", "std/audio.blsp"),
    // Shared mutable ETS-style store → `table/*` (`table?` stays a bare core predicate).
    embedded_module!("table", "std/table.blsp"),
    // Non-mainstream process surface (introspection/control + OS subprocesses) → `proc/*`.
    embedded_module!("proc", "std/proc.blsp"),
    embedded_module!("timer", "std/timer.blsp"),
    embedded_module!("dev", "std/tool/dev.blsp"),
    embedded_module!("reflect", "std/reflect.blsp"),
    embedded_module!("bytes", "std/bytes.blsp"),
    // Distributed nodes (ADR-033/068/073/074): policy over the `%node-*`/`%nodes`/`%disconnect`/
    // `%monitor-node` primitives → `node/*` (node/connect, node/start, node/spawn, …).
    embedded_module!("node", "std/node.blsp"),
    // TLS sockets: policy over the `%tls-*` primitives → `tls/*`.
    embedded_module!("tls", "std/net/tls.blsp"),
    // Sets as a library over maps (ADR-062): a set is a map of `element → true`,
    // so membership/elements/size reuse `contains?`/`keys`/`count`; the module
    // adds `set`/`conj`/`disj`/`union`/`intersection`/`difference`/`subset?`.
    // Opt-in, never in the prelude (no `#{…}` literal / distinct type yet).
    embedded_module!("set", "std/set.blsp"),
    // Semantic versions as data: parse / order / test against a `">= 1.2"`,
    // `"^1.2"`, `"~> 1.3"`, or `">= 1.2, < 2.0"` constraint. Written because two
    // consumers (the registry deciding which release is newest, an application
    // deciding whether a plugin's declared `:enhances` constraint is met) had each
    // hand-rolled it. Pure predicates; the version SELECTION algorithm is `resolver`.
    embedded_module!("version", "std/version.blsp"),
    // The dependency version resolver (ADR-209): a pure backtracking, newest-compatible
    // solver over an injected `provider` (what versions exist, what each requires). The
    // registry provider that fetches for real lives in `std/tool/package`; keeping the
    // search pure here is what makes it exhaustively testable offline.
    embedded_module!("resolver", "std/resolver.blsp"),
    // NOTE: behaviour contracts (`defbehaviour` / `%register-protocol` / `ops` /
    // `*protocols*`) are CORE — they live in the prelude (`std/protocol.blsp`, included by
    // lib.rs), so they are bare and always available, not an on-`require` module here.
    // Unified generic functions with NOMINAL dispatch (the value-polymorphism successor):
    // `defability` declares ops, `impl` registers per-identity impls from anywhere, and
    // dispatch is on the first argument's identity — its `type-of` kind, or a record's
    // The interactive REPL line editor (ADR-052): `highlight` is the pure lexical
    // syntax-highlighter / bracket-matcher / signature + completion scanners;
    // `lineedit` is the raw-mode, emacs-style editor built on it + the inline
    // `term-*` seam. Both opt-in, never in the prelude; `repl` requires them.
    // `highlight`/`lineedit` stay in CORE: they are reusable UI a shipped app may
    // `require` (the editor's minibuffer reuses `std/lineedit`'s core), not just
    // REPL plumbing — so a lean release keeps them.
    embedded_module!("editor/highlight", "std/editor/highlight.blsp"),
    // Generic tree-sitter language services (`fontify` + structural motions) over
    // the `tree-sitter-parse` builtin's positioned CST — the foreign-language
    // analogue of `sexp`+`highlight`. Pure UI a shipped editor `require`s for its
    // ruby/elixir/… modes (ROADMAP §C), so it stays in CORE; opt-in, never prelude.
    embedded_module!("editor/treesit", "std/editor/treesit.blsp"),
    // Lexical Markdown highlighter — the `highlight` analogue for `.md` buffers
    // (`markdown-spans` → `[start end face]` spans, ADR-092). Pure UI a shipped app
    // may `require` (bedit's markdown-mode), so it stays in CORE alongside
    // `highlight`/`lineedit`; opt-in, never in the prelude.
    embedded_module!("editor/markdown", "std/editor/markdown.blsp"),
    // Lexical `.env` and Dockerfile highlighters, the dotenv/Dockerfile analogues of
    // `markdown` (`env-spans` / `dockerfile-spans` → `[start end face]` spans). Pure
    // UI a shipped app may `require` (bedit's env-/docker-mode); CORE, like markdown.
    embedded_module!("editor/dotenv", "std/editor/dotenv.blsp"),
    embedded_module!("editor/dockerfile", "std/editor/dockerfile.blsp"),
    embedded_module!("editor/lineedit", "std/editor/lineedit.blsp"),
    embedded_module!("format", "std/format.blsp"),
    // The process-native tracing debugger — `break` (park without timeout),
    // `span`/`span-spawn` (cross-process causal tree), `spy` routed to a debugger
    // process. The actor-model answer to Elixir's `dbg`.
    //
    // CORE, not DEV, and this is the line the split turns on: a dev module is one that
    // serves *developing* an app (the test framework, `nest doc`, the hot-reload
    // watcher), not one an app's own shipped features are built from. A shipped editor
    // IS a debugger (bedit's `C-c d` session, its *Spy* trace stream), so a lean
    // release that omitted this couldn't run it — `require` fails at boot, since
    // `run-bundle` loads every bundled module.
    embedded_module!("debug", "std/tool/debug.blsp"),
    // A persistent, image-isolated evaluator: a dedicated child runtime runs
    // `(eval-server-run)` — one `pr-str`ed request map per stdin line, one reply
    // line back — so a parent (an editor playground, a remote REPL) gets
    // REPL-grade eval with per-request timeouts without exposing its own global
    // table. The pure codec half is shared by clients (ADR-198).
    //
    // CORE for the same reason as `debug` (which it requires): "evaluate this snippet"
    // is a shipped app's feature — bedit's tutorial playgrounds and `C-x C-e` ride
    // this codec — not a tool for building one.
    embedded_module!("eval-server", "std/tool/eval-server.blsp"),
];

/// Dev/tooling modules — baked in only under the `dev-tools` feature (the dev
/// `brood`/`nest` + tests). A `nest release` lean runtime
/// (`--no-default-features`) omits them, so a shipped app carries no test
/// framework, process observer, MCP/doc/hot-reload tooling, or interactive REPL
/// (ADR-038, docs/release.md). `project` stays in CORE — it boots the bundle;
/// `lineedit`/`highlight` stay too (reusable UI, e.g. the editor's minibuffer).
///
/// **The test for this list:** a module belongs here only if it serves *developing*
/// an app. If a shipped app's own features are built on it, it belongs in
/// [`CORE_MODULES`] however tool-shaped it looks — `debug` and `eval-server` live
/// there for exactly that reason (an editor ships a debugger and an eval
/// playground). Getting this wrong is not a graceful degradation: `run-bundle`
/// eagerly loads every bundled module, so one app module with a top-level
/// `(require 'missing)` makes the released binary fail to boot at all.
#[cfg(feature = "dev-tools")]
const DEV_MODULES: &[EmbeddedModule] = &[
    // The test framework — `deftest`/`describe`/`assert=`/`is`. Never shipped.
    embedded_module!("test", "std/tool/test.blsp"),
    // Doc generation (`nest doc`) — tooling, not runtime.
    embedded_module!("docs", "std/tool/docs.blsp"),
    // The surface audit — docstring / example / data-first argument order over every
    // public callable (`(audit/report)`). Tooling: it reads the live image's globals.
    embedded_module!("audit", "std/tool/audit.blsp"),
    // Generate editor syntax grammars (VS Code TextMate, Emacs font-lock) from the
    // language's own `(reflect/special-forms)` — one source of truth, no drift (ADR-092).
    embedded_module!("grammar", "std/tool/grammar.blsp"),
    // The process viewer / debug tooling (`nest observe`, `(observe)`).
    embedded_module!("observer", "std/tool/observer.blsp"),
    // The hot-reload file watcher — a dev-loop convenience.
    embedded_module!("reload", "std/tool/reload.blsp"),
    // The Model Context Protocol tool surface — `(mcp-tools)` returns the
    // catalogue the `nest mcp` dispatcher reads (ADR-036, docs/mcp.md, step 3).
    embedded_module!("mcp", "std/tool/mcp.blsp"),
    // One-call performance triage: `(perf/report)`/`(perf/summary)` read `(dev/vm-stats)` +
    // `(dev/gc-stats)` and apply `docs/benchmarking.md` §2's interpretation rules, so "is this
    // dispatch-, env-, or alloc-bound?" does not depend on recalling them. DEV: it serves
    // *developing* a program, and a shipped app has no use for it.
    embedded_module!("perf", "std/tool/perf.blsp"),
    // The read-eval-print loop itself, written in Brood (`(require 'repl)`):
    // policy over the `read-line`/`reflect/eval-string`/`pr-str` primitives. The Rust
    // binaries (`brood`, `nest repl`) just bootstrap into `(repl-run)`. A shipped
    // app runs its own `:main`, never the REPL.
    embedded_module!("repl", "std/tool/repl.blsp"),
];

/// Empty in a lean (`--no-default-features`) release runtime — the dev modules
/// above are not compiled in at all (their `include_str!` never runs).
#[cfg(not(feature = "dev-tools"))]
const DEV_MODULES: &[EmbeddedModule] = &[];

/// Baked-in reference *documents* (markdown), the counterpart to
/// [`EMBEDDED_MODULES`] for non-module text. `(%builtin-doc 'brood-for-claude)`
/// returns the language guide that `nest new` scaffolds into each new project,
/// so a freshly-scaffolded project is self-contained without depending on a
/// Brood install path.
const EMBEDDED_DOCS: &[(&str, &str)] = &[
    (
        "brood-for-claude",
        include_str!("../../../../docs/brood-for-claude.md"),
    ),
    // The Claude Code skill that `nest new` drops into each project's
    // `.claude/skills/`, so an AI assistant editing the project auto-loads the
    // Brood-writing rules. The full reference is `brood-for-claude`; this is the
    // short triggerable checklist (`SKILL.md` frontmatter + the LLM traps).
    // Canonical source lives here in `docs/` (a tracked path); the repo's own
    // `.claude/skills/writing-brood/SKILL.md` is a local symlink to it — `.claude/`
    // is gitignored, and a compile-time `include_str!` must not depend on an
    // untracked path (it would break a fresh clone's build).
    (
        "writing-brood-skill",
        include_str!("../../../../docs/writing-brood-skill.md"),
    ),
];

/// Coerce a (symbol | keyword | string) name argument to its spelling, the shape
/// every embedded-source lookup accepts. `None` for any other value.
pub(super) fn embedded_name(heap: &Heap, v: Value) -> Option<String> {
    match v {
        Value::Sym(s) | Value::Keyword(s) => Some(value::symbol_name(s)),
        Value::Str(id) => Some(heap.string(id).to_string()),
        _ => None,
    }
}

/// The lookup body shared by `%builtin-module` and `%builtin-doc`: coerce the
/// (symbol | keyword | string) name argument, find it in `table`, return the
/// baked-in source as a fresh string (or `nil` if absent). `who`/`label` are
/// used only in the type-error message.
pub(super) fn lookup_embedded(
    args: &[Value],
    heap: &mut Heap,
    table: &[(&str, &str)],
    who: &'static str,
    label: &'static str,
) -> LispResult {
    let v = arg(args, 0);
    let name = match embedded_name(heap, v) {
        Some(name) => name,
        None => return Err(LispError::wrong_type(heap, who, label, v)),
    };
    match table.iter().find(|(n, _)| *n == name) {
        Some((_, src)) => Ok(heap.alloc_string(src)),
        None => Ok(Value::nil()),
    }
}

/// The baked-in module registered under `key`, core table first then dev/tooling
/// (absent in a lean release runtime).
fn embedded_module(key: &str) -> Option<&'static EmbeddedModule> {
    CORE_MODULES
        .iter()
        .chain(DEV_MODULES.iter())
        .find(|m| m.key == key)
}

/// `(%builtin-module name)` — the source of a baked-in std module as a string,
/// or nil if there is none. Mechanism only: `require` (Brood) consults this
/// before searching the load-path.
pub(super) fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let Some(name) = embedded_name(heap, v) else {
        return Err(LispError::wrong_type(
            heap,
            "%builtin-module",
            "module name",
            v,
        ));
    };
    if let Some(module) = embedded_module(&name) {
        return Ok(heap.alloc_string(module.source));
    }
    // Not a baked-in std module — consult a mounted release bundle (the app's
    // own modules + bundled deps), so `require` resolves them with no change to
    // its load-path logic (ADR-038).
    match crate::bundle::mounted() {
        Some(b) => match b.module_src(&name) {
            Some(src) => Ok(heap.alloc_string(src)),
            None => Ok(Value::nil()),
        },
        None => Ok(Value::nil()),
    }
}

/// `(%builtin-module-file name)` — where a baked-in module's source was written: its
/// repo-relative path (`"std/tool/test.blsp"`), or `"<bundle>/<name>.blsp"` for a module
/// served out of a mounted release bundle, which genuinely has no path. Nil if `name`
/// isn't an embedded module at all (a load-path file has its own real path).
///
/// `require--force` hands this to `%load-string` so the module's forms are attributed to
/// the file they were written in. Without it they took the requiring file's name — see
/// [`EmbeddedModule`].
pub(super) fn builtin_module_file(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let Some(name) = embedded_name(heap, v) else {
        return Err(LispError::wrong_type(
            heap,
            "%builtin-module-file",
            "module name",
            v,
        ));
    };
    if let Some(module) = embedded_module(&name) {
        return Ok(heap.alloc_string(module.path));
    }
    // A bundled module has a name but no path. Say so rather than inventing one that
    // looks openable, and rather than falling back to the requiring file.
    let bundled = crate::bundle::mounted()
        .as_ref()
        .is_some_and(|b| b.module_src(&name).is_some());
    if bundled {
        let marker = format!("<bundle>/{name}.blsp");
        return Ok(heap.alloc_string(&marker));
    }
    Ok(Value::nil())
}

/// `(%bundled?)` — true when this executable is a release bundle (an app built
/// by `nest release`), false for a plain `brood`/`nest` runtime.
pub(super) fn bundled_p(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(crate::bundle::is_bundled()))
}

/// `(%bundle-manifest)` — the embedded `project.blsp` source of a release
/// bundle, or nil when not bundled.
pub(super) fn bundle_manifest(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match crate::bundle::mounted() {
        Some(b) => Ok(heap.alloc_string(&b.manifest)),
        None => Ok(Value::nil()),
    }
}

/// `(%bundle-module-names)` — the list of module names (filename stems) embedded
/// in a release bundle, or nil when not bundled.
pub(super) fn bundle_module_names(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match crate::bundle::mounted() {
        Some(b) => {
            let items: Vec<Value> = b.module_names().map(|n| heap.alloc_string(n)).collect();
            Ok(heap.list(items))
        }
        None => Ok(Value::nil()),
    }
}

/// `(%builtin-doc name)` — the source of a baked-in reference document as a
/// string, or nil if there is none. Used by `nest new` to scaffold the language
/// guide into each new project.
pub(super) fn builtin_doc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    lookup_embedded(args, heap, EMBEDDED_DOCS, "%builtin-doc", "doc name")
}

/// `(reflect/builtin-modules)` — the names of every module baked into this binary, as a
/// sorted list of strings. The module table is a Rust static, so the language has
/// no other way to see it; `std/tool/complete.blsp` uses it to offer `nest doc`
/// candidates, and it is generally useful for validating a module name before
/// `require`ing it.
pub(super) fn builtin_modules(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut names: Vec<&str> = CORE_MODULES
        .iter()
        .chain(DEV_MODULES.iter())
        .map(|module| module.key)
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(heap.alloc_string(n));
    }
    Ok(heap.list(items))
}

/// `(%in-ns 'foo)` — set the namespace being compiled into (ADR-065). Emitted by
/// the `ns` macro; the resolver pass qualifies subsequent definitions and free
/// references to `foo/…`. Returns the (possibly rooted) namespace symbol.
///
/// Under an active dependency load (ADR-070), the declared name is **rooted** to the
/// package: loading dep `foo`'s `b.blsp` — which says `(defmodule b)` → `(%in-ns 'b)`
/// — sets `compile_ns` to `foo/b`, so the file's `def`s become `foo/b/…`. Outside a
/// dep load (root project / std) the name is unchanged.
pub(super) fn in_ns(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // `(%in-ns nil)` clears the namespace back to ROOT — the restore half of a
    // save/set/restore bracket (`reflect/current-ns` returns nil at root, so a
    // bracket that saved nil needs a way to put it back). The deferred-contracts
    // machinery (`%contracts-apply-pending!` in tools.blsp) is the first user.
    if matches!(arg(args, 0), Value::Nil) {
        heap.set_compile_ns(None);
        return Ok(Value::nil());
    }
    let sym = expect_symbol(heap, "%in-ns", arg(args, 0))?;
    let rooted = heap.root_module_name(sym);
    heap.set_compile_ns(Some(rooted));
    // Region model (ADR-223): switch the active forward-ref set to the module this opens,
    // so a file's later `defmodule` resolves its own bare names — not the earlier module's.
    // Keyed by the bare declared name (pre-rooting), which is exactly `sym`.
    heap.activate_ns_region(sym);
    Ok(Value::symbol(rooted))
}

/// `(%compile-context)` — the process's compile context as data: `[ns imports]`, `ns`
/// the current namespace symbol (nil at root) and `imports` a vector of
/// `[bare qualified]` pairs (an ambiguous import, ADR-235, carries a vector of its
/// candidates in place of `qualified`). Both halves are PER PROCESS, and nothing in the
/// language could read the second — so an evaluator that runs each request in a fresh
/// process (`std/tool/eval-server.blsp`) had no way to carry a `(defmodule m (:use x))`
/// from one request to the next. The restore half is [`restore_compile_context`].
pub(super) fn compile_context(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let ns = match heap.compile_ns() {
        Some(s) => Value::symbol(s),
        None => Value::nil(),
    };
    let entries = heap.imports_snapshot();
    let mut pairs = Vec::with_capacity(entries.len());
    for (bare, entry) in entries {
        let target = match entry {
            ImportEntry::One(q) => Value::symbol(q),
            ImportEntry::Ambiguous(cands) => {
                let items = cands.into_iter().map(Value::symbol).collect();
                heap.alloc_vector(items)
            }
        };
        pairs.push(heap.alloc_vector(vec![Value::symbol(bare), target]));
    }
    let imports = heap.alloc_vector(pairs);
    Ok(heap.alloc_vector(vec![ns, imports]))
}

/// `(%restore-compile-context ctx)` — install a [`compile_context`] snapshot in THIS
/// process: the namespace exactly as `%in-ns` would (rooted, region activated; nil back
/// to root) and the import table replaced wholesale. Returns the namespace.
pub(super) fn restore_compile_context(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts = heap.seq_items(arg(args, 0))?;
    let (ns, imports) = match parts.as_slice() {
        [ns, imports] => (*ns, *imports),
        _ => return Err(LispError::runtime(
            "%restore-compile-context: expected a `[ns imports]` snapshot from %compile-context",
        )),
    };
    let mut table: std::collections::HashMap<value::Symbol, ImportEntry> =
        std::collections::HashMap::new();
    for pair in heap.seq_items(imports)? {
        let Some((bare, target)) = heap.seq_items(pair).ok().and_then(|p| match p.as_slice() {
            [Value::Sym(b), t] => Some((*b, *t)),
            _ => None,
        }) else {
            return Err(LispError::runtime(
                "%restore-compile-context: each import must be a `[bare qualified]` pair",
            ));
        };
        let entry = match target {
            Value::Sym(q) => ImportEntry::One(q),
            other => ImportEntry::Ambiguous(
                heap.seq_items(other)?
                    .into_iter()
                    .filter_map(|v| match v {
                        Value::Sym(s) => Some(s),
                        _ => None,
                    })
                    .collect(),
            ),
        };
        table.insert(bare, entry);
    }
    heap.set_imports(table);
    match ns {
        Value::Nil => {
            heap.set_compile_ns(None);
            Ok(Value::nil())
        }
        Value::Sym(sym) => {
            let rooted = heap.root_module_name(sym);
            heap.set_compile_ns(Some(rooted));
            heap.activate_ns_region(sym);
            Ok(Value::symbol(rooted))
        }
        _ => Err(LispError::runtime(
            "%restore-compile-context: the namespace must be a symbol or nil",
        )),
    }
}

/// `(%root-module-name 'b)` — root a referenced module name to the active package:
/// `foo/b` while loading dep `foo` if `b` is one of `foo`'s modules, else `b`
/// unchanged (ADR-070). The loader emits it around `(:use …)`/`(:alias …)`/`require`
/// targets and `defmodule`'s provide/doc key so intra-package references and the
/// module's own registration all agree on the rooted global identity.
pub(super) fn root_module_name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%root-module-name", arg(args, 0))?;
    Ok(Value::symbol(heap.root_module_name(sym)))
}

/// `(%set-package-context 'foo '(a b c))` — enter dep `foo`'s load with its provided
/// short module names, returning `[prev-prefix prev-modules]` (a tuple) so the caller
/// restores the enclosing context after the load (dep loads nest). `(%set-package-context
/// nil nil)` clears it. Roots every module name the load declares or references (ADR-070).
pub(super) fn set_package_context(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Nil => None,
        v => Some(expect_symbol(heap, "%set-package-context", v)?),
    };
    let modules: std::collections::HashSet<crate::core::value::Symbol> = heap
        .list_to_vec(arg(args, 1))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| match v {
            Value::Sym(s) => Some(s),
            _ => None,
        })
        .collect();
    let (prev_prefix, prev_modules) = heap.set_package_context(prefix, modules);
    let prev_mods_list: Vec<Value> = prev_modules.into_iter().map(Value::Sym).collect();
    let list = heap.list(prev_mods_list);
    let prefix_val = prev_prefix.map(Value::Sym).unwrap_or(Value::nil());
    Ok(heap.alloc_vector(vec![prefix_val, list]))
}

/// `(reflect/current-ns)` — the namespace currently being compiled into (a symbol), or
/// `nil` at root. Reflection + a handle for tests (ADR-065).
pub(super) fn current_ns(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.compile_ns().map(Value::Sym).unwrap_or(Value::nil()))
}

/// `(%register-sig 'name 'type)` — record a user-declared `(sig name type)` for the
/// advisory checker. Emitted by the `sig`/`sig!` macros alongside their existing
/// expansion. `name` is qualified to the current namespace *exactly as a `def` head
/// would be* — via [`resolve_reference`](crate::eval::macros::resolve_reference), the
/// same compile-pass entry point `def` uses (own-ns pre-scanned def heads + existing
/// `ns/name` globals qualify; root/prelude names stay bare) — so the key matches the
/// qualified global the call site resolves to. `type` is the raw type-expression form
/// (e.g. `(int -> int)`), stored verbatim on the heap; the checker parses it on read
/// and gives it precedence over inferred/curated sigs. A runtime value-producing call
/// (returns the qualified name), so it composes inside the `sig` macro's expansion.
pub(super) fn register_sig(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_symbol(heap, "%register-sig", arg(args, 0))?;
    let type_value = arg(args, 1);
    // Qualify the name to the current namespace, mirroring how `def` qualifies a
    // definition head — so the store key is the same module-qualified symbol the
    // call site resolves to (intra-module misses the bare file-local ctx; cross-module
    // the sig isn't in the caller's ctx at all).
    let qualified = crate::eval::macros::resolve_reference(heap, name);
    heap.set_declared_sig(qualified, type_value);
    Ok(Value::symbol(qualified))
}

/// `(%mark-private 'name)` — record the global `name` as module-private (ADR-146).
/// Emitted by the `defn-`/`def-` macros alongside their `def`. `name` is qualified
/// to the current namespace *exactly as a `def` head would be* — via
/// [`resolve_reference`](crate::eval::macros::resolve_reference), the same entry
/// `%register-sig` uses — so the recorded key matches the qualified global the def
/// created (and the `def` runs first, so that global already exists). Privacy is now
/// a fact the def form declares here, not one derived from the name's spelling.
/// Returns the qualified name so it composes inside the macro's `do`.
/// `(%side-facts)` — every side fact this runtime has recorded, one string per fact,
/// sorted: `"<kind> <name> <payload>"`.
///
/// Exists for the boot differential (ADR-320 step 3). The differential used to compare a
/// hand-listed set of per-name attributes, which is the same shape of hand-maintained list
/// the journal removed from the image writer — so a fact kind could be carried correctly by
/// construction and still round-trip WRONG without the differential noticing. `meta`
/// (ADR-283) was exactly that: carried since ADR-314, compared by nothing.
///
/// Rendering to strings rather than to structured data is deliberate. The consumer is a
/// differential that compares two process outputs textually, and a string per fact means a
/// **new kind shows up in the comparison automatically** — no Brood-side change, no second
/// list to update, which is the property this whole ADR is about.
pub(super) fn side_facts(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::core::heap::Fact;
    let mut lines: Vec<String> = heap
        .side_facts()
        .iter()
        .map(|f| {
            let name = value::symbol_name(f.subject());
            match f {
                Fact::Private(_) => format!("private {name}"),
                Fact::Meta(_, m) => format!(
                    "meta {name} since={:?} deprecated={:?} beta={:?} use={:?}",
                    m.since,
                    m.deprecated,
                    m.beta,
                    m.use_instead.map(value::symbol_name)
                ),
                // The def-site FILE is deliberately reduced to its basename: each
                // differential arm runs under its own `XDG_CACHE_HOME`, so the
                // materialised `prelude.blsp` sits at a different absolute path in each
                // and comparing those would compare the harness. Basename + line:col
                // still fails on a missing, wrong or shifted site.
                Fact::DefSite(_, loc) => {
                    let base = loc.file.rsplit('/').next().unwrap_or(&loc.file);
                    format!("def-site {name} {base}:{}:{}", loc.pos.line, loc.pos.col)
                }
                Fact::RegistryName(_) => format!("registry-name {name}"),
                Fact::Dynamic(_) => format!("dynamic {name}"),
            }
        })
        .collect();
    lines.sort();
    let vals: Vec<Value> = lines.iter().map(|l| heap.alloc_string(l)).collect();
    Ok(heap.list(vals))
}

pub(super) fn mark_private(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Resolves the quoted name against the namespace being compiled now, which is what makes
    // `(defn- helper …)` inside a module mark `mod/helper`. Note the standing mismatch
    // recorded with `def-` in `std/prelude/core.blsp`: `def` resolves its head at COMPILE
    // time, this resolves at CALL time, and the two disagree for a prelude function that
    // assigns a root private while some module is loading. No live path takes that shape;
    // closing it means restating `qualify_name`'s rules in Brood at expansion time.
    let name = expect_symbol(heap, "%mark-private", arg(args, 0))?;
    let qualified = crate::eval::macros::resolve_reference(heap, name);
    heap.mark_private(qualified);
    Ok(Value::symbol(qualified))
}

/// Add one `(:use …)` import (bare → qualified) to the current file's table,
/// enforcing the two Elixir-style import rules:
///
/// - **Clash (lazy, ADR-235).** If `bare` is already imported from a *different* module,
///   the two do NOT error here — importing both modules is fine as long as the caller
///   never uses the shared name bare. The name is recorded as *ambiguous*; the resolver
///   raises a use-site error naming the candidates only when `bare` is actually referenced
///   without qualification (resolvable then with `:only`/`:exclude`, an alias, or a
///   qualified call). Re-importing the *same* qualified name (a re-`:use` of the same
///   module) is a no-op, so idempotent reloads are fine.
/// - **Shadow (warning).** If `bare` already names a live root/prelude/builtin global,
///   the import shadows it — allowed (the resolver gives an import precedence over
///   root), but warned, exactly as Elixir warns when an import shadows an
///   auto-imported `Kernel` name. Reach the original with the `/name` root escape, or
///   silence per-name with `:exclude`. `BROOD_NO_SHADOW_WARN` mutes the class.
fn refer_add(
    heap: &mut Heap,
    bare: value::Symbol,
    qualified: value::Symbol,
    mod_name: &str,
) -> Result<(), LispError> {
    // An ambient (`defdyn`) name always resolves bare/root — the resolver's `is_ambient`
    // check short-circuits before it ever consults the import table — so an import for
    // one is inert, and it can neither clash nor shadow. Skip it: no entry, no error, no
    // warning (a dynamic knob like `*width*` shared by two modules must not read as one).
    if value::is_dynamic(bare) {
        return Ok(());
    }
    // Clash → ambiguous, not an error (ADR-235). A `One` from a different module or an
    // existing `Ambiguous` both fold this candidate in; the resolver reports it only if the
    // bare name is used. Re-`:use` of the same module (same qualified) stays a no-op.
    if let Some(existing) = heap.import_of(bare) {
        if existing == qualified {
            return Ok(()); // idempotent — same module referred again (e.g. reload)
        }
        heap.mark_import_ambiguous(bare, qualified);
        return Ok(());
    }
    if heap.ambiguous_import_of(bare).is_some() {
        heap.mark_import_ambiguous(bare, qualified);
        return Ok(());
    }
    if heap.env_get(value::EnvId::GLOBAL, bare).is_some()
        && std::env::var_os("BROOD_NO_SHADOW_WARN").is_none()
    {
        let b = value::symbol_name(bare);
        eprintln!(
            "warning: (:use {mod_name}) refers `{b}`, which shadows the prelude/root `{b}`; \
             reach the original as `/{b}`, or drop it with `:exclude [{b}]`"
        );
    }
    heap.add_import(bare, qualified);
    Ok(())
}

/// Is `mod_name` currently mid-load — present in the `*features-loading*` in-flight
/// table (ADR-136)? Outside a cycle this is always false at `%refer` time: a normal
/// `(:use m)` fully loads and `provide`s `m` (clearing the marker) *before* its
/// `%refer` runs. A module still loading here therefore means the current file is
/// being referred from *inside* `m`'s own load — a `(:use)` cycle, whose refer-all
/// would silently import only the names defined so far.
/// Is `mod_name` recorded in `*features*` — has some process `provide`d it? Read-only (no
/// allocation), so it is usable from an error path holding `&Heap`.
pub(crate) fn module_is_provided(heap: &Heap, mod_name: &str) -> bool {
    let map_id = match heap
        .env_get(value::EnvId::GLOBAL, value::intern("*features*"))
        .map(|v| v.unpack())
    {
        Some(crate::core::value::ValueRef::Map(id)) => id,
        _ => return false,
    };
    heap.map_entries(map_id)
        .iter()
        .any(|(k, _)| matches!(*k, Value::Str(s) if heap.string(s) == mod_name))
}

/// Is `key` one of this binary's baked-in `std/` modules?
pub(crate) fn is_embedded_module(key: &str) -> bool {
    embedded_module(key).is_some()
}

fn module_is_loading(heap: &mut Heap, mod_name: &str) -> bool {
    let map_id = match heap
        .env_get(value::EnvId::GLOBAL, value::intern("*features-loading*"))
        .map(|v| v.unpack())
    {
        Some(crate::core::value::ValueRef::Map(id)) => id,
        _ => return false,
    };
    let key = heap.alloc_string(mod_name);
    heap.map_get(map_id, key).is_some()
}

/// `(%refer 'mod subset exclude)` — add `(:use …)` imports to the current file's
/// import table (ADR-065 inc-2). `mod` must already be loaded (the `defmodule` macro
/// emits a `(require 'mod)` first). `subset` nil → refer every *public* `mod/name`
/// (not recorded private, not itself nested); else a seq of bare symbols → refer
/// just those as `mod/name`. `exclude` (a seq of bare names, or nil) drops those from
/// a refer-all — Elixir's `except:`. Each import becomes a bare → qualified entry the
/// resolver consults after the current namespace and before root; clashes and
/// prelude shadows are policed by [`refer_add`].
pub(super) fn refer(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mod_sym = expect_symbol(heap, "%refer", arg(args, 0))?;
    let mod_name = value::symbol_name(mod_sym);
    let prefix = format!("{}/", mod_name);
    // The `:exclude` set (bare symbols to skip in a refer-all).
    let excluded: std::collections::HashSet<value::Symbol> = match arg(args, 2) {
        Value::Nil => std::collections::HashSet::new(),
        ex => heap
            .seq_items(ex)?
            .into_iter()
            .filter_map(|v| match v {
                Value::Sym(s) => Some(s),
                _ => None,
            })
            .collect(),
    };
    match arg(args, 1) {
        Value::Nil => {
            // A refer-all against a module still mid-load is a circular `(:use …)`:
            // its public set is incomplete, so importing "all" of it would silently
            // miss the names defined after the cycle point. Fail clearly instead —
            // `:only` (lazy, resolved at reference time) is the cycle-safe escape.
            if module_is_loading(heap, &mod_name) {
                return Err(LispError::runtime(format!(
                    "circular `(:use {mod_name})`: `{mod_name}` is still loading (a cycle back \
                     into this module), so a refer-all would import only the names defined so \
                     far. Break the cycle, or import just what you need with \
                     `(:use {mod_name} :only [...])`, which resolves lazily and is cycle-safe."
                )));
            }
            // Refer all public names: enumerate the live globals under `mod/`.
            let mut referred = 0usize;
            // Public names the module actually EXPOSES, whether or not this `:use` took
            // them. The diagnostic below asks "are this module's globals gone?", and only
            // this count can answer that: `referred` is 0 both when the globals are missing
            // (the bug) and when the caller excluded every one of them (perfectly healthy).
            let mut public_seen = 0usize;
            for g in heap.global_symbols() {
                let name = value::symbol_name(g);
                if let Some(bare) = name.strip_prefix(&prefix) {
                    // `g` is a live enumerated global under `mod/`, so `is_private`
                    // (the recorded fact) is exact — the module is loaded here.
                    if !bare.is_empty() && !bare.contains('/') && !heap.is_private(g) {
                        let bare_sym = value::intern(bare);
                        public_seen += 1;
                        if excluded.contains(&bare_sym) {
                            continue;
                        }
                        refer_add(heap, bare_sym, g, &mod_name)?;
                        referred += 1;
                    }
                }
            }
            // KI-120 diagnostic. A refer-all of a baked-in std/editor module that imports
            // NOTHING leaves every bare use of its names unresolved, each dying later as
            // `unbound symbol: <bare>` in whichever process runs it — the wrapper's
            // `def-face`/`ui-run` wave. Restricted to EMBEDDED modules on purpose: a std
            // module always has public API, so importing nothing from one means its globals
            // are gone under a `*features*` that still says loaded (the bug). A user/test
            // module legitimately refers nothing — all-private (`priv-vault2`), everything
            // `:exclude`d (`clpb2`), or `defdyn`-only (`dynprov`, whose names are ambient, not
            // `mod/` globals) — so those are NOT the signal and must stay silent, or the line
            // becomes noise the reader learns to skip past.
            // `public_seen`, not `referred`. Excluding every public name of an embedded
            // module leaves `referred == 0` with nothing wrong: bedit's completion module
            // does exactly that — `(:use fuzzy :exclude [filter match])`, `fuzzy`'s only two
            // publics — and calls `fuzzy/filter` qualified. That warned on every cold
            // `nest check` of the flagship downstream project (the run has to rebuild
            // `.brood/image.bin` to show it, which is why it read as a one-off), and the
            // message was not merely noisy but false: it says no public `fuzzy/` global is
            // bound, while both were. The shape was already listed here as one that must
            // stay silent — `clpb2` — but the guard chosen was `is_embedded_module`, which
            // excuses a USER module that excludes everything and not a std one.
            if referred == 0 && public_seen == 0 && is_embedded_module(&mod_name) {
                eprintln!(
                    "[refer] (:use {mod_name}) imported NOTHING — no public `{mod_name}/` global is bound; \
                     *features* lists it: {}, mid-load: {}, pid={:?} scope={}",
                    module_is_provided(heap, &mod_name),
                    module_is_loading(heap, &mod_name),
                    crate::process::current_pid(),
                    crate::process::self_isolate_scope(),
                );
            }
        }
        subset => {
            // Refer just the named symbols as `mod/name` (existence not required —
            // an unbound `mod/name` surfaces as a normal unbound-reference error).
            for item in heap.seq_items(subset)? {
                let bare = expect_symbol(heap, "%refer", item)?;
                let bare_name = value::symbol_name(bare);
                let qualified = value::intern(&format!("{}/{}", mod_name, bare_name));
                // A module-private name in an explicit :only list is a privacy breach
                // unless this file holds an internals grant for the module (ADR-146) —
                // same rule the resolver enforces for qualified references. Privacy is
                // the recorded fact (`is_private`): the module is being imported, so it
                // is loaded and the record is exact.
                if heap.is_private(qualified)
                    && heap
                        .import_of(crate::eval::macros::internals_grant_key(&mod_name))
                        .is_none()
                {
                    return Err(LispError::runtime(format!(
                        "(:use {mod_name} :only [... {bare_name} ...]): `{bare_name}` is module-private (ADR-146); grant access with (:use-internals {mod_name}) or use the public API"
                    )));
                }
                refer_add(heap, bare, qualified, &mod_name)?;
            }
        }
    }
    Ok(Value::nil())
}

/// `(%grant-internals 'mod)` — the `(:use-internals mod)` header clause's
/// mechanism (ADR-146): record that the CURRENT file may reference `mod`'s
/// module-private names (qualified access), which is otherwise a compile
/// error. Stored in the per-file import table under the impossible key
/// `/internals/<mod>` (the `%alias` trick), so it rides the same save/restore
/// lifecycle as every other import.
pub(super) fn grant_internals(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let m = expect_symbol(heap, "%grant-internals", arg(args, 0))?;
    let key = crate::eval::macros::internals_grant_key(&value::symbol_name(m));
    heap.add_import(key, m);
    Ok(Value::nil())
}

/// `(%alias module short)` — register a module alias (Elixir-style): a later
/// qualified reference `short/name` resolves to `module/name`. Stored in the import
/// table under the slash-suffixed key `short/`, so it rides the same per-file
/// lifecycle as `%refer`. The `(:alias …)` header emits it. A second `short` for a
/// different module is a loud error (the ambiguous-last-segment case — disambiguate
/// with an explicit `:as`).
pub(super) fn alias(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let module = expect_symbol(heap, "%alias", arg(args, 0))?;
    let short = expect_symbol(heap, "%alias", arg(args, 1))?;
    let key = value::intern(&format!("{}/", value::symbol_name(short)));
    if let Some(prev) = heap.import_of(key) {
        if prev != module {
            return Err(LispError::runtime(format!(
                "alias `{}` is already bound to `{}` — can't also alias `{}`; give one an explicit `:as` name",
                value::symbol_name(short),
                value::symbol_name(prev),
                value::symbol_name(module),
            )));
        }
    }
    heap.add_import(key, module);
    Ok(Value::nil())
}

#[cfg(test)]
mod tests {
    use super::{CORE_MODULES, DEV_MODULES};

    /// A release bundle runs on the LEAN runtime, which compiles [`DEV_MODULES`] away
    /// entirely — and `run-bundle` loads every module the app ships, so one top-level
    /// `(require 'x)` for a dev-only `x` means the released binary cannot boot at all.
    /// These two are the capabilities a shipped app's own features are built from (an
    /// editor ships a debugger and an eval playground), so they must stay in CORE.
    /// This test is the guard: moving either back to DEV breaks `nest release`, and
    /// the symptom is a failure to start, far from the cause.
    #[test]
    fn app_runtime_capabilities_stay_out_of_dev_modules() {
        for key in ["debug", "eval-server"] {
            assert!(
                CORE_MODULES.iter().any(|m| m.key == key),
                "`{key}` must be in CORE_MODULES — a lean release runtime omits DEV_MODULES, \
                 so an app requiring it would fail to boot"
            );
            assert!(
                !DEV_MODULES.iter().any(|m| m.key == key),
                "`{key}` is in DEV_MODULES; it is a shipped-app capability, not dev tooling"
            );
        }
    }

    /// Every baked-in module is reachable under exactly one key, in one list. A stem
    /// listed twice (say `debug` left in DEV while also added to CORE) would resolve by
    /// whichever list `embedded_module` scans first — a silent split-brain.
    #[test]
    fn embedded_module_keys_are_unique() {
        let mut keys: Vec<&str> = CORE_MODULES
            .iter()
            .chain(DEV_MODULES.iter())
            .map(|m| m.key)
            .collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "a baked-in module key is listed twice");
    }
}
