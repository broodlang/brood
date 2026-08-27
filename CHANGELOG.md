# Changelog

All notable changes to the Brood toolchain (`brood`, `nest`, `brood-lsp`) are
recorded here. Versions follow [semver](https://semver.org); the full
engineering narrative lives in [`docs/devlog.md`](docs/devlog.md).

## v0.14.1 — 2026-08-27

**BREAKING (tooling surface) — the checker looks inside a literal, and finds a fifth dead
call site.**

- **KI-70: `nest check` never walked a vector or map literal.** `check_into_inner` returned
  for any form that was not a `Pair`, so every expression nested inside `[…]` or `{…}` was
  invisible to every lint — which is the whole Hiccup style (`std/editor/*`, and every web
  layer written in Brood). hive's `/docs` renderer carried `(str (max 2 …))` for weeks after
  `max` moved to `math`, with `nest check`, `nest test` and the boot check all green; only
  rendering the page raised it. Vectors and maps now descend (map **keys** too). No false
  positives — the checker sees macroexpanded forms, so pattern vectors are already lowered,
  and `quote`/`quasiquote` stop the walk before their data is reached.
- **`project/all-files` is public, and `project-all-files` is gone.** The first run of the
  fixed checker found the fifth dead `project-*` call in `std/tool/mcp.blsp` — the `callers`
  MCP tool called a `defn-` from another module, inside a map literal, so it raised on every
  invocation. Promoted and de-stuttered beside `project/source-files`, as KI-67 did for its
  four.
- **The v0.14.0 tree failed `nest format --check`** on three `.blsp` files. Formatted.

**Docs**

- `docs/known-issues.md` gained a **filing process** in place of a header of 2026-07 trivia:
  how to take a number without colliding, the five questions an entry answers, the
  requirement to sabotage-verify a guard and record its red output, what each status means,
  and how to tell the tree is green (a *cancelled* CI run is not evidence — that is what hid
  KI-68/KI-69 for two days).
- `doc_refs::no_two_entries_claim_the_same_number` — two sessions numbered different issues
  KI-70 minutes apart and nothing caught it, because `defined()` collected headings into a
  set. The `seq/remove-nth` note is now **KI-71**.
- `docs/handoff.md` replaced; it had claimed `main` was green on all five CI jobs since
  2026-08-19.

## v0.14.0 — 2026-08-27

**The gates that had stopped gating.** No language change — this release is the CI tree
going green again, and three separate harnesses that were passing without testing anything.

**Fixed**

- **The fuzz-differential gate was hollow (KI-68).** `stress/fuzz_programs.py` writes Brood
  source from Python, and the namespacing waves retired every name it emitted — `(table)`,
  `rem`, `quot`, `min`/`max`, `bit-and`/`bit-or`/`bit-xor`, the `table-*` family, `println`,
  and the linear-map whitelist. Every generated program died on line 1 **identically in all
  four configs**, so the differential reported agreement and every seed printed `ok`. Names
  updated, and **liveness is now asserted**: an `unbound symbol` from a generated program is
  a hard failure naming the dead names, and a run where not one seed exits cleanly fails as
  "the corpus is dead, not the engines agreeing". Sabotage-verified in the original shape.
- **The `differential (tree-walker)` CI job had been red since KI-64's fix (KI-69).** The two
  new `jit_plan` guards assert on VM-compiled arms and the job runs `BROOD_VM=0`; both now
  pin `set_forced_ceiling(Some(Tier::Native))`, as `compile/tests.rs` has since ADR-222.
- **`examples/` and the stress corpus were 22 harnesses deep in rename rot.** `examples/life`
  called `map-pairs` and `examples/node_server` called a bare `register`; `stress/` and
  `scripts/fuzz/stress/` between them named `os/getenv`, `rem`, `quot`, `mod`, `min`, `max`,
  `string-length`, `read-all`, `read-string` and `gen/spawn-server`. `make check-examples`
  and `make check-stress` are both clean again.
- **`docs/language.md` and `docs/brood-for-claude.md` taught retired names.** The whole
  `print`/`println`/`eprint`/`eprintln` family (gone since the `io/` trio landed in 0.13.0),
  `spawn-server` (now `gen/start`), and an arithmetic reference still claiming
  `quot`/`mod`/`rem`/`floor`/`min`/`max` are bare when they live in `math`. This matters
  twice over: `brood-for-claude.md` is baked into the binary and dropped into every project
  `nest new` scaffolds, so a wrong name there propagates to every assistant reading it.

**Known**

- `eval_forward_ref.blsp` opts out of the unbound lint with `(check-allow :unbound …)`: its
  two names are defined by `eval`, and that invisibility is the thing the harness exists to
  exercise (KI-24).

## v0.13.0 — 2026-08-26

**BREAKING — the output trio settles, and the two string builders stop sharing a name.**

- **`io/write` / `io/puts` / `io/inspect`.** `io/print` (which existed only in 0.12.0)
  is `io/write`; `io/inspect` is new and writes the RE-READABLE form, so
  `(io/puts "a\nb")` prints two lines where `(io/inspect "a\nb")` prints `"a\nb"`.
  The `Port` ability's op moved off `write` to **`emit`** to free the name: a caller
  reaches for `io/write` constantly, an implementor writes `(emit [p s] …)` once.
- **`string/interp` and `string/format`.** `fmt` was an abbreviation of "format"
  attached to the thing that is *not* format. They are one namespace apart now and
  share no root: `(string/interp "x={x}")` is the macro that reads values from scope,
  `(string/format "%.2f" x)` is the function whose template can be a runtime value.
  Neither subsumes the other.
- **De-stuttered:** `ansi/strip-ansi` → `ansi/strip`, `docs/generate-docs` →
  `docs/generate`, `project/check-project` → `project/check`, `project/run-project` →
  `project/run`, `task/cancel-task` → `task/cancel`, `telemetry/start-telemetry` →
  `telemetry/start`, `telemetry/stop-telemetry` → `telemetry/stop`, `url/build-url` →
  `url/build`, `test/run-test` → `test/run`.

**Added**

- `tests/doc_examples_test.blsp` — every indented `form → result` line in every public
  docstring is now EXECUTED and compared. Of the first 27 examples written against it,
  seven were wrong (map results in insertion order; `(reverse "abc")` raises).
- `scripts/stdlib-audit.blsp` — the standing surface report: stutters, docstrings,
  examples, and which names are bare.

**Fixed**

- `io/port?` was false for the default `*out*`/`*err*` (they are `:native`, and `Port`
  had only a `:fn` impl), so `:to *err*` leaked `#<native %write-err>` into its own
  message. Every test passed because `with-err-str` rebinds `*err*` to a closure.
- A registry write silently re-published a private global: `%registry-update!` reused
  `env_define`, which clears the private mark, and nothing re-marked it.
- The formatter split a keyword argument from its value (`:to` alone on a line) —
  134 files, a net 377 fewer lines once rejoined.
- Three concurrency tests that timed their races instead of observing them.
- **`docsite/render-js`** — a `:wrap? false` render hands page chrome to the host, but only
  half that hand-off existed: `render-css` was public and the filter script was private, so
  an embedding host could recover the stylesheet and had no way to recover the JS. The
  fragment carries the `Filter…` input, so every embedder shipped a search box that focuses,
  accepts typing and does nothing — correct HTML, 200 OK, no server-side symptom. Both of
  brood.fly.dev's embedders (the language reference and every package's docs page) had it.
- `nest new`'s `.gitignore` never ignored `.brood/`, the startup-image cache, so every
  scaffolded project committed a build artifact and re-committed it on each `nest check`.
  Eight ecosystem repos were carrying the churning binary.
- This repo's own `project.blsp` said `:version "0.1.0"` at Cargo's `0.13.0` — it had not
  moved since the first release. A test now asserts the two agree.
- **Every ecosystem package declared a `:brood` floor of `>= 0.5.0`**, which stopped being
  true at 0.10.0: four consecutive releases renamed the stdlib out from under them, so a
  user on 0.5.0 would resolve a package, install it, and hit unbound symbols. Since a
  published release's metadata is immutable, correcting this meant new releases —
  `store`/`s3`/`store-postgres` 0.3.0, `hatch` 0.5.0, `bedit` 0.3.0, the four themes 0.2.0,
  each now naming the version CI actually proves.

## v0.12.0 — 2026-08-25

**BREAKING — output moves to `io/`, and the bare core drops from 317 to ~280.**

- **`io/print` / `io/puts`** replace `print`/`println`/`eprint`/`eprintln`, beside the
  `Port` ability they write through. `io/write` was already taken by the right thing —
  `Port`'s one-string seam, `(io/write port s)` — so these are the convenience layer over
  it. The destination is a trailing **`:to <port>`**, which keeps printing variadic; a
  leading port would make `(io/puts p)` ambiguous (write it, or target it?). It must be
  both `:to` and something `port?` accepts, so `(io/puts :to 3)` prints `:to 3`.
  Stderr is `:to *err*` — the dynamic var, not `(io/stderr-port)`, so `with-err-str`
  still captures. `inspectln` is deleted (no callers); bare `inspect` stays, since it is
  the `Inspect` ability op and returns a string.
- **`timer/`** — `timer/send-after`, `timer/send-interval`, `timer/cancel`, one family in
  one namespace instead of split across the prelude and `proc/`. **`sleep` stays bare**:
  it parks the current process (a peer of `receive`), and the module loader itself sleeps
  between polls, so rooting it in a module would have the loader require one mid-require.
- **`string/->number` / `string/->symbol`** — the scalar bridges apply the module-rooted
  rule the reference already documents (`string/->bytes`). Both moved together.
- **`span-runs` → `%span-runs`** — one caller (the editor highlighter), so it is
  mechanism, not vocabulary. The Rust `print`/`eprint` become `%print`/`%eprint`.
- **The registries are private.** `*impls*`, `*record-ids*`, `*methods*`, `*abilities*`
  and ~20 more were published API under an "Other" heading; they are `def-` now and the
  heading is gone. This needed a kernel fix: `%registry-update!` reused `env_define`,
  which clears the private mark on every global definition — right for a real `def`,
  wrong for an in-place registry write, so privacy silently undid itself on first use.
- **`disj` is NOT moved.** It looked orphaned in a "Sets" group of one, but its pair is
  `conj` and both are deliberately core and polymorphic. The orphan was a categorisation
  bug, now fixed.

**Fixed**

- The formatter split a keyword argument from its value (`:to` on its own line). A
  keyword and the form after it are one unit now — 134 files, a net 377 fewer lines.
- The playground reported `version()` as `0.1.0`, the wasm shim's own crate version,
  rather than the Brood it runs.
- Reference docs named functions the renames had moved, including two runtime error
  messages (`proc/flag` reported "process-flag: unknown flag").
- Two concurrency tests timed their races instead of observing them, and went red on a
  loaded CI box for no defect.

## v0.11.0 — 2026-08-24

**BREAKING — the bare core drops from 337 to 291.** Subsystems keep moving out of the
core namespace into modules, so the names left bare are the ones a basic algorithm
actually reaches for:

- **`path/`** — `path/join`, `path/dirname`, `path/basename`, `path/absolute`,
  `path/temp`. The prelude's `path-*` set was a second path library beside
  `std/path.blsp` with subtly different contracts (`path/join` is variadic and resets on
  an absolute segment; `path/basename` strips a trailing slash). One boot helper,
  `%path-join`, remains because the module loader needs a path before modules load.
- **`bytes/`** — 13 names, de-stuttered: `bytes/at`, `bytes/length`, `bytes/slice`,
  `bytes/concat`, `bytes/int`, `bytes/uint`, `bytes/int->`, …
- **`seq/`** — 20 names: `transduce`, `filterv`, `mapv`, `find`, `distinct`, `flatten`,
  `split-at`, `sample`, `shuffle`, `remove`, `keep`, `subvec`, `vector-ref`, … Nine had to
  stay (`but-last`, `mapcat`, `mapv`, `partition`, `take-while`, `zip`, …) because a
  prelude MACRO calls them at expansion time, which happens during boot.
- **`os/`** / **`gui/`** — `os/clipboard-get`, `os/clipboard-set!`, `gui/image-thumb`.
- **`map/reduce-kv`**. `hash-map` and `zipmap` stay core: the quasiquote lowering emits
  `hash-map` for `{…}` literals inside templates, and a prelude helper calls `zipmap`.

**Duplicates retired.** `string->utf8-bytes`/`utf8-bytes->string` were the same conversion
as `string/->bytes`/`string/bytes->`; the module pair is now the only spelling.

**`%defseq`** — the definer behind `map`/`filter`/`mapcat`/`remove`/`keep` — is no longer
published: it is prelude scaffolding with no user call sites.

Untouched by design: I/O (`println` and friends), predicates, and the reflection
leftovers, pending a decision on whether a script or REPL can refer a module's names bare.

## v0.10.0 — 2026-08-24

**BREAKING — the core reference went from 613 names to 337 (ADR-242).** Two thirds of that
was noise or misplacement, not deletion:

- **191 private helpers were being published.** The core reference page reads the live
  image, and a prelude `defn-` still binds a root global — so the match compiler's 40
  `match-*` functions, the `receive-*`/`spy-*`/`defmodule-*` helpers and the `x`/`l`
  transducers all appeared as core API. They never were.
- **`dev/`** — runtime diagnostics: `dev/mem-bytes`, `dev/gc-collect`, `dev/gc-stats`,
  `dev/vm-stats`, `dev/sched-stats`, `dev/profile-start`, `dev/bench`, … (20 names)
- **`reflect/`** — source tooling: `reflect/check-file`, `reflect/parse-source`,
  `reflect/scan-tokens`, `reflect/source-location`, `reflect/type-signature`, … (18 names)
- **`%`-prefixed** — the ability/multimethod registry the `defability`/`impl`/`defmulti`
  expansions emit (`register-impl`, `impl-for`, `identity-of`, …; 26 names)

Interactive introspection stays bare on purpose — `doc`, `arglist`, `bound?`, `apropos`,
`doc-search`, `macroexpand`, `special-forms` are what you type at a REPL. So does
`check-allow`, a source pragma, and `system-monitor`, which arms the production event
stream `telemetry/watch-runtime` re-emits.

**The reference has no "Other" section any more.** Uncatalogued names fell into a junk
drawer 41 entries deep that was hiding real vocabulary: `stop`, `cast`, `call`,
`spawn-server` and `defprocess` (uncategorised only because `gen` became core) and the
`tap`/`then` pipeline helpers. Every public core name now has a category, enforced by a
test.

**Fixed:** `remote-spawn`, `remote-spawn-sync` and `bench` were still registered as
highlighter keywords after moving to `node/`/`dev/`, so editors highlighted three unbound
names as core syntax.

## v0.9.0 — 2026-08-24

**BREAKING — the bare core is smaller: five subsystems moved into namespaces.** Each is a
Brood module over a `%`-prefixed primitive, so the language core stays close to what a
basic algorithm actually reaches for and stops colliding with your own names:

- `os/` — `getenv`, `hostname`, `run-process`, `exe-path`, `canonicalize`,
  `stdin-tty?`, `stdout-tty?`, `now`, `now-ns`
- `table/` — `new` (was the bare `table` constructor), `get`, `put`, `delete`, `has?`,
  `count`, `drop`, `incr`, `snapshot`
- `proc/` — `info`/`flag` (were `process-info`/`process-flag`), `list`, `mailbox-size`,
  `hibernate`, `cancel-timer`, and the OS-subprocess API `spawn`/`send`/`close`/`set-binary`
- `audio/beep`, `rand/token`

`table?` stays a bare predicate, `send-after`/`send-interval` stay bare (only the timer
*cancel* moved), and `offload` stays bare — it is core dirty-native concurrency, used
during `nest fetch` before any module loads.

**BREAKING — `supervisor/stop` is gone; use `stop`.** A supervisor is an ordinary server
process and answers the same `[:$stop]` message the core `stop` sends, so the wrapper was
a verbatim duplicate whose only effect was to shadow `stop` for `(:use supervisor)`.
`(stop sup)` tears it down exactly as before, children first. If a module of yours defines
its own `stop`, reach the core one as `/stop`.

**Library names and core names (ADR-241).** A library export may shadow a core name only
when the module is meant to be used *qualified*. `wasm/call`, `version/compare`,
`log/error` and `package/update` keep their names on that basis; duplicates were deleted
and bare-use modules renamed.

**Fixed: `nest run` was broken for every invocation** with `unbound symbol: getenv` — the
pre-run check is built as a Brood snippet inside a Rust string, which no checker reads.
Ten sibling sites were repaired with it, plus 13 stress scripts under `scripts/fuzz/`.
`scripts/stale-names.sh` now finds this class after a rename.

**`supervisor/stop` is gone — use `stop`.** A supervisor is an ordinary server process and
answers the same `[:$stop]` message the core `stop` sends, so the wrapper was a verbatim
duplicate whose only effect was to shadow `stop` for anyone writing `(:use supervisor)`.
`(stop sup)` tears it down exactly as before, children first. If a module of yours defines
its own `stop`, reach the core one as `/stop`.

**Library names and core names (ADR-241).** A library export may shadow a core name only
when the module is meant to be used *qualified*. `wasm/call`, `version/compare`,
`log/error` and `package/update` keep their names on that basis; duplicates were deleted
and bare-use modules renamed.


**`nest rename` is context-aware.** It now parses each file into its lossless CST and
rewrites only *symbol tokens*, so a docstring or `;` comment mentioning the name, and
symbols inside `(quote …)` / `'…` data, are left byte-for-byte alone — a text-level rename
corrupts all three (it rewrote comment prose like "the offload pool" and clobbered the very
`defn` head it should have left alone). New flags:

- `--refs-only` — rename callers, leave the `defn`/`def` head alone
- `--defs-only` — rename only the definition head
- `--in-quote` — also rewrite symbols inside quoted data (off by default: a quoted symbol
  is inert data, e.g. a name in a registry table)
- `--text` — the old context-blind whole-token replace, for a rename that must touch prose

Verified lossless by round-tripping every in-repo `.blsp` file (320) through a no-op rename
and requiring byte-identical output. `codemod/cst-rename` / `codemod/cst-rename-text` are
the in-language entry points.

**Fixed: `(temp-path …)` raised `unbound: rand/token`** unless the caller had required the
`rand` module. It is prelude code, so it now uses the `%random-token` primitive directly.
Found by a new build-time lint that rejects any prelude reference to a module wrapper that
is not loaded at boot.

**Internal (ADR-240): a primitive's name has one definition site.** The `PRIMITIVE_DOCS`
array is merged into the `def()` registrations (name, arity, signature, arg list and
docstring in one expression), and a primitive named in more than one Rust file now flows
from a single `kw::` constant. Renaming a primitive is a one-line edit the compiler then
enforces at every site, instead of a string-literal hunt whose misses surface as runtime
`unbound` errors.

## v0.8.0 — 2026-08-21

**Module-name stutter and abbreviations removed from public APIs.** A qualified call
should read `csv/parse`, not `csv/csv-parse` — the module name already namespaces it.
Every public function that repeated its module name (or abbreviated it) was de-stuttered:

- `csv/csv-parse` → `csv/parse` (+ `emit`, `parse-maps`, `emit-maps`)
- `diff/diff-lines` → `diff/lines` (+ `patch`, `summary`, `unified`, `seq`)
- `http/http-*` → `http/read-request`, `http/listen`, `http/request`, `http/post`; the
  GET convenience is `http/fetch` (a public `get` would shadow core `get` in `:use`rs)
- `debug/debug-*` → `debug/attach`, `debug/report`, `debug/watch`, `debug/repl-session`
- `coverage/coverage-*` → `coverage/report`, `coverage/results`, `coverage/begin!`, …
- `eval-server/eval-server-*` → `eval-server/answer`, `eval-server/run`; `repl/repl-run` → `repl/run`
- `supervisor/start-supervisor` / `stop-supervisor` → `supervisor/start` / `supervisor/stop`
- `complete/complete-*` → `complete/modules`, `complete/tags`, … (`complete/for-kind`,
  `complete/print-candidates` where a bare `for`/`print` would clash)
- `project/project-*` (public) → `project/find-root`, `project/setup`, `project/parse-deps`,
  … (`project/apply-config`, since a public `apply` would shadow core `apply` in `:use`rs)

**`datetime` loses the `dt` abbreviation.** Accessors and operations are spelled out:
`datetime/year`, `datetime/month`, `datetime/add`, `datetime/diff`, `datetime/format`,
the conversions `datetime/->epoch-ms` / `datetime/epoch-ms->` / `datetime/->date` /
`datetime/->time`, and the comparisons as word predicates —
`datetime/before?`, `datetime/after?`, `datetime/same?`, `datetime/not-after?`,
`datetime/not-before?` (bare `<`/`=` would shadow the core operators inside the module).

Private, module-internal helpers keep their prefix (it is internal namespacing, not
public stutter). `nest rename` (the identifier-aware codemod) drove the propagation across
the ecosystem.

**Behaviour contracts are core.** `std/protocol.blsp` moved into the prelude, so
`defbehaviour`, `register-protocol`, `ops`, and the `*protocols*` registry are bare and
always available — no `require`, no `(:use protocol)`. (The type checker already read
`*protocols*` bare, so this only aligns the runtime with it.)

**Two more name fixes.** `crypto/secure=?` → `crypto/constant-time-equal?` (the `=?`
glyph read oddly; the new name states the actual security property). And the gen cast
`!` was an operator glyph that looked like raw send but actually wraps a cast envelope —
it became `cast` (raw asynchronous send is the kernel's bare `send`).

**The gen_server actor framework is core.** `gen` moved into the prelude (like
`protocol`), so `spawn-server`, `call`, `cast`, `call-timeout`, `stop`, `code-change`,
`spawn-server-link`, `spawn-server-named`, and the `defprocess` macro are **bare** — no
`require`, no `gen/` prefix. It is a peer of `spawn`/`send`/`receive`, not a library. The
old prelude-freeze concern (its `receive`/`match` expansion stranding lambdas) does not
apply: the prelude only *defines* `defprocess`, never expands it. `call`/`cast`/`stop`
join the core-reserved process verbs.

**Transcendental math moved into the `math/` namespace.** `sin`, `cos`, `tan`, `asin`,
`acos`, `atan`, `atan2`, `exp`, `ln`, `log2`, `log10` are no longer bare — they're
`math/sin`, `math/acos`, … The bare language core keeps only fundamental arithmetic
(`+ - * / < = >`, `inc`, `dec`, `min`, `max`, `mod`, `rem`, `quot`, `floor`, bitwise).
Mechanism/policy split as usual: the raw f64 kernels are now `%`-prefixed primitives
(`%sin`, `%ln`, …) and the `math` module wraps them, so they document under `math` in
the reference rather than cluttering the core.

**Subsystem primitives moved out of the bare core into namespaces.** The same
mechanism/policy split (raw primitive → `%`-prefixed, a Brood module wraps it) was applied
to five subsystems, so the language core keeps only fundamental operations:
- **terminal** → `term/*` (`term/enter`, `term/poll`, `term/draw`, …)
- **windows** → `gui/*` (`gui/open`, `gui/draw`, `gui/title!`, …)
- **rope text-engine** → `text/*` (`text/insert`, `text/slice`, `text/line`, `text/from-string`, …)
- **PRNG** → `rand/*` (`rand/int`, `rand/float`, `rand/rng`, `rand/seed`); the `%rand-*`
  mechanism stays internal because the sequence ops `sample`/`shuffle` build on it
- **sockets** → `tcp/*` and `tls/*` (`tcp/connect`, `tcp/send`, `tls/request`, …)

Bitwise (`bit-and`/`bit-or`/…) stays bare — it is a fundamental integer operation, not a
subsystem.

**Distribution moved into a `node/` module.** `connect`, `disconnect`, `nodes`,
`node-start`, `node-cookie`, `monitor-node`, `remote-spawn`, … were bare core primitives —
they are now `node/connect`, `node/disconnect`, `node/list`, `node/start`, `node/cookie`,
`node/monitor`, `node/spawn`, `node/spawn-sync`, `node/name`, `node/also-listen`,
`node/serve-spawns`. The raw kernel primitives are `%`-prefixed (`%nodes`, `%disconnect`,
`%node-name`, …) and `std/node.blsp` wraps them. Freeing bare `connect` means a `(:use tcp)`
no longer needs to exclude it — only the kernel `send` remains genuinely core (so a socket
send is `tcp/send`, qualified). The core `send`/`spawn`/`receive`/`monitor`/`link`/`self`
process primitives stay bare.

## v0.7.0 — 2026-08-21

**Standard-library consistency pass.** A full review of the prelude and standard
library settled three naming inconsistencies and closed the public-API
documentation gap.

**One empty-collection constructor: `empty`.** The eager collections now agree with
`stream` — `pq/empty`, `queue/empty`, and `multimap/empty` replace the old `new`
(they were always nullary "make the empty X" builders, so the name now matches both
the `empty?` predicate and their own docstrings). `stream/empty` is unchanged.

**Conversion/word names unabbreviated.** `crypto/encrypt-str` / `crypto/decrypt-str`
become `crypto/encrypt-string` / `crypto/decrypt-string` (the string-oriented twins of
the `bytes` `crypto/encrypt` / `crypto/decrypt`), and the bootstrap path helper
`parent-dir` becomes `parent-directory`.

**Public surface is now fully documented; internals are private.** Every public
function in `std/` carries a docstring (previously ~64 did not — the `sexp` command
layer, the `format` CST API, the `project`/`test`/`debug` tooling, and more). The 53
functions that were only ever implementation helpers of `format`, the test runner,
`project`, `workspace`, `resolver`, `stats`, and `scaffold` are now `defn-` (private),
so they no longer leak into each module's API. A reader-based audit confirms zero
undocumented public functions remain across the standard library.

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
