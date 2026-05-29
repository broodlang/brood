# Deferred work

Things that are *worth doing* but were intentionally **not** done in their
triggering session — either because the design needs an ADR, the scope is
bigger than the immediate need justifies, or the workaround today is good
enough. Each entry captures the why, a design sketch (so picking it up later
doesn't restart from zero), the trigger that should pull it back in, and the
workaround available today.

This is a holding pen, not a backlog: items that get picked up land in
[`devlog.md`](devlog.md) and (for design decisions with trade-offs)
[`decisions.md`](decisions.md). Items here can be edited, merged, or dropped
as the situation changes.

---

## 1. First-class set type + `#{…}` literal

**Why deferred.** Maps-as-sets work today — the Game-of-Life prototype used
`{cell true}` and the only thing it cost was a useless `true` value and a
`(keys m)` round-trip on read-back. Shipping a real set type is a coordinated
change across **reader** (the `#{…}` literal — currently `#` is an unbound
symbol error), **printer** (sets must print distinctly from maps),
**value/heap** (new `Tag::Set` or a "set-shaped map" tagged sub-form, plus
GC integration), **structural `=`** and **`hash_value`** (order-independent),
the **type checker** (one new bit in `types.rs`, sigs for the new builtins),
and ~8–10 stdlib functions (`set`, `conj`/`disj`, `union`/`intersection`/
`difference`, `contains?`-on-set, `set?`, seqable like maps). That's an ADR's
worth of work for an ergonomic win — not a blocker.

**Design sketch.** Re-use the CHAMP trie (ADR-040): a set is structurally a
map with a singleton "present" sentinel value (or an empty-payload variant of
`MapNode`). The handle is a new `Value::Set(SetId)` so equality and printing
don't conflate set and map. Reader: `#{ a b c }` → `(hash-set a b c)`,
matching the existing `{ k v … }` shape. Iteration follows the maps-are-
seqable rule but yields **elements**, not `[k v]` pairs. Hashing must be
order-independent (XOR-fold of element hashes is the standard trick) so
`#{1 2 3} = #{3 2 1}`.

**Trigger to pick this back up.**
- A second prototype where "set of X" is the natural model (the editor
  buffer's "set of dirty regions" / "set of subscribers" / "set of
  registered features" are plausible — M2+).
- *Or* the type checker grows union types and needs to express
  "set-of-Tag" as a first-class value (today it's `u16` bitsets inside the
  checker; if that ever leaks Brood-side, a real set type pays off).

**Workaround today.** `{cell true}` for membership; `(keys m)` for elements;
`(contains? m x)` for membership test; `(merge a b)` for union;
`(reduce dissoc a (keys b))` for difference. Wrap once in a project-local
`set` helper if it gets noisy.

---

## 2. Real laziness + `iterate`

**Why deferred.** Stack-safe tail recursion already covers most "evolving
state" cases through the `name--at`/`name--loop` accumulator idiom that
runs throughout `std/prelude.blsp`. The friction is genuinely real — every
state-evolution program reinvents `--at` — but it's a duplication tax, not
a missing capability. A real lazy-sequence type is a new `Value` kind, a
new GC story (the thunk closes over an env), new seqable rules, and a
mental-model shift (force semantics, head-holding pitfalls). That's a big
change for an ergonomic gain.

**Design sketch.** Add `Value::Lazy(ThunkId)` — an unforced thunk that on
first deref produces a `(cons head tail-thunk)` shape. `iterate f x` is
the canonical producer (`x` then `(f x)` then `(f (f x))` …); `take n`
forces `n` heads and stops; `map`/`filter`/`take-while` operate lazily;
`force` realises the whole sequence. The big design question — chunked
(Clojure-style) vs unchunked (Scheme-style) — leans **unchunked**: simpler
GC story, simpler reasoning about side effects in the producer fn, and
the editor's use cases (frame sequences, generators over file lines)
don't need chunked throughput. Equality on a `Lazy` is **identity** (don't
force just to compare); printing shows `#<lazy>` unrealised.

**Trigger to pick this back up.**
- A real editor feature that wants an unbounded sequence (animation
  frames, undo history fold, lines of a streaming file) and where the
  accumulator-helper workaround is materially worse than the lazy
  spelling.
- *Or* a benchmark where pre-materialising an intermediate list is
  measurable hot-path cost.

**Workaround today.** Bounded `iterate-times` already exists in
`std/prelude.blsp` for the "n successive states" case. For unbounded
evolution, write a tail-recursive `--at`/`--loop` helper — the pattern is
mechanical (state + step → next state, in tail position) and stays O(1)
stack.

---

## 3. MCP worker-panic isolation — ✅ landed 2026-05-29

**Status.** Shipped — see the second 2026-05-29 devlog entry. A Rust panic
inside any tool-call code path (Brood-callable Rust, `eval`, `apply`,
`defn` body) now surfaces as a structured JSON-RPC error and the server
keeps serving. Regression test in `crates/nest/src/mcp.rs`
(`handler_panic_is_caught_and_server_keeps_serving`) pins the behaviour.
Entry kept here as the reference for *why* this was the shape it took.

**Why it was deferred (then).** The KI-1/KI-2 scheduler race that was
triggering panics was the urgent fix; isolating panics from the server
boundary was recognised as a separate concern that the same session
shouldn't conflate with the race work. Now that the race is fixed (devlog
2026-05-29), the isolation is the next blocking issue for `nest mcp` as a
stable surface.

**The behaviour to fix.** A single panicking green process — any
`unimplemented!`, any `unwrap` on `None`, any out-of-bounds index inside
the kernel or a Brood-callable Rust path — currently takes down the
entire `nest mcp` process. The MCP client sees `Connection closed` and
every `mcp__brood__*` tool drops for the rest of that session. A user
evaluating arbitrary code against the live image must never be able to
kill the server with one bad expression.

**Design (as built).**
- The whole `call_tool` body in `crates/nest/src/mcp.rs` runs inside
  `std::panic::catch_unwind(AssertUnwindSafe(|| …))`. `AssertUnwindSafe`
  is sound here because the MCP server is single-threaded (a synchronous
  `main_loop` over stdio); the heap reset that already runs on the
  no-panic path also runs on the unwind path, discarding any partial
  LOCAL allocations the panicking handler left behind.
- `RpcError::from_panic` projects the unwind payload (downcast as
  `&'static str` or `String`) into the JSON-RPC `error` object, with
  `error.data.kind = "panic"`, the original panic message, and a `hint`
  string that calls it an interpreter bug. The default Rust panic hook
  still runs (to stderr, useful for server-side debugging) — only the
  *propagation* is contained; stderr stays separate from the stdio
  JSON-RPC channel.
- Worker-thread panics (a green process on a scheduler thread that
  panics) are *not* covered by this change — the existing scheduler is
  expected to keep workers alive across one process's panic. Revisit
  only if a real worker-thread panic surfaces.
- Regression test (`handler_panic_is_caught_and_server_keeps_serving`)
  triggers a panic via a new debug-only `%force-panic` primitive
  (`#[cfg(debug_assertions)]`) and asserts (1) the response is a
  structured `error` with `kind: "panic"`, and (2) the *next* tool call
  on the same `Interp` succeeds.

**Workaround that was needed before.** Restart `nest mcp` after every
crash — which broke the whole point of the live image's persistent state.

---

## 4. Cross-module redefinition warning

**Why deferred.** ADR-019 made the namespace **flat** by deliberate
choice — names are globals, modules are a load convention, not a barrier.
The Game-of-Life report hit a `render-row` collision with `mandel/render-row`
and the only signal was a `[reload] arity changed for render-row: 3 -> 2`
line buried in load output. Adding a definition-time warning is small
**implementation-wise** but requires a design call on suppression: every
intentional override needs a way to say "yes, I meant to shadow that".
Without a clear opt-out the warning becomes noise, not signal.

**Design sketch.**
- At every global-table `define` site, record the *origin file* of the
  current binding (a `(SourceFile, Pos)` shadow table keyed by
  `Symbol`).
- When a `define` arrives for a name that already has an origin **from a
  different file**, emit a checker-style warning at the new def's
  position: `life/render-row shadows mandel/render-row (defined at
  src/mandel.blsp:42:1)`. Suppression: `(def ^:override foo …)`
  metadata, or `(defn ^:override foo …)` — silenced explicitly per
  binding so the warning stays useful where it isn't.
- Hot-reload (`reload-defs`) is not a redefinition — the origin matches
  the existing binding, so no warning fires.
- Same-file redefinitions are silent (already handled cleanly by the
  load process).
- This is **diagnostics-layer**, not core-language: a warning, not an
  error; advisory in the spirit of the type checker.

**Trigger to pick this back up.**
- A project with ≥ 3 modules where a user actually loses time to a
  silent collision (the Game-of-Life prototype was the first; the
  *second* report of this is the trigger).
- *Or* `nest new` starts scaffolding multi-module projects by default.

**Workaround today.**
- `foo--private` for module-internal helpers (the existing convention —
  `--` is the privacy marker).
- For public names: self-prefix (the report's `life-row` for what was
  originally `render-row`). Manual discipline, fragile across modules.
- The `[reload] arity changed for foo: N -> M` line on load is a partial
  signal — visible if you read the output.

---

## 5. `nest format --changed`

**Why deferred.** `nest format` is whole-tree by default, and `nest format
<path>` handles single-file. The formatter is **idempotent**, so re-running
on unchanged files is safe — the real complaint is *diff noise*: a single
`nest format` rewrites every `.blsp` in the project even if you only
touched two of them, and the rewrites are real edits to lines you don't
own in your current change. That's a working-tree-hygiene issue, not a
formatter-behaviour issue.

**Design sketch.**
- `nest format --changed` resolves the set of changed `.blsp` files
  through git: `git diff --name-only HEAD` ∪ `git diff --name-only
  --cached` ∪ `git ls-files --others --exclude-standard`, filtered to
  files under the project's source roots and ending in `.blsp`. Each is
  fed to the same in-Brood CST walker `nest format` uses today.
- Optional companions: `--staged` (just staged files), `--since REF`
  (changed since `REF`).
- If git isn't available or the cwd isn't a repo, fall back to whole-tree
  with a one-line note on stderr.
- The flag is **additive** — existing `nest format` and `nest format
  <path>` keep their current behaviour; the user opts in when they want
  the narrower scope.

**Trigger to pick this back up.**
- A second project where the diff-noise from whole-tree formatting
  produces an unreviewable commit.
- *Or* `nest format` becomes slow enough on a multi-hundred-file project
  that the change-only path is also a speed win.

**Workaround today.**
- `nest format path/to/file.blsp` per touched file (manual but precise).
- Shell loop: `git diff --name-only HEAD | grep '\.blsp$' | xargs -r -I{}
  nest format {}` (one liner; works fine, just not built in).
- Stash unrelated formatter changes (`git stash --keep-index`, then
  `nest format`, then `git stash pop` — fiddly).
