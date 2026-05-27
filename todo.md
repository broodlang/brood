# TODO

Running scratch list of work to pick up. Promote items to `docs/roadmap.md` /
an ADR once they're committed to. Newest section at the top.

## Possibility: compile a `nest` project into a standalone binary

Status: **idea, not committed** (discussed 2026-05-27). Captured here so the shape
doesn't have to be re-derived.

**Key call — bundle, not AOT.** Brood is a tree-walker and `def`-rebind hot reload
(the shared *mutable* RUNTIME table) is load-bearing, so "compile to a binary"
means *embed the runtime + the project's code image into a self-contained
executable* (the `deno compile` / Erlang escript model) — **not** AOT-to-machine
code, which would fight the late binding that's the whole point.

Most machinery already exists:
- `include_str!` already bakes `.blsp` into the binary — prelude (`lib.rs:152`),
  std modules (`builtins.rs` `BUILTIN_MODULES` + `%builtin-module`). A project's
  modules would just become baked-in modules like the std ones.
- Boot path is `Interp::new()` + `eval_str`; a bundled `main()` is ~10 lines.
- `nest new` already scaffolds `src/main.blsp` with `(defn main ())` + `(provide 'main)`.
- `run-process` can drive `cargo` from Brood, so build *policy* stays in
  `std/project.blsp` (ADR-006), Rust only hosts the launcher template.

Missing pieces:
1. An `argv` / command-line-args primitive (~10 lines; there's `getenv`, no argv).
2. A run contract — the binary loads the project main module and calls `(main args)`;
   let `project.blsp` optionally declare `:main module/fn`. This also yields a
   **`nest run`** (doesn't exist yet) — really step 0.
3. A launcher-crate template (generated `Cargo.toml` + `main.rs` depending on the
   `brood` lib, embedding the project image as a name→source table).
4. A `nest build` driver — mostly Brood (reuse the `nest doc` source-walk): emit
   bundle + launcher, `(run-process "cargo" ["build" "--release"])`, move binary out.

Phasing: **P0** `nest run` (½ day) → **P1** `nest build` source-bundle (a few days;
reuses all the above — needs a Rust toolchain at build time, output ≈ `brood` size,
re-parses project source each launch). Later/optional: **P2** a frozen
post-macroexpand `SharedCode` image (skips parse/expand at startup — real
serialization infra, pairs with the tracing-GC / send-functions-between-processes
work); and a no-toolchain appended-payload stub (the `deno compile` trick).

Caveats: no dependency manager yet (flat `require`/`*load-path*` + baked std), so
only **std-only projects** are bundleable until the deps story lands; and the
generated launcher must reference the `brood` lib crate (path dep locally;
publishing hits the crates.io `brood` name collision noted in project notes).

## Supervision / process-framework track (the "OTP-in-Brood" idea)

Build an Erlang/OTP-style process + supervision layer, but as **Brood policy** on
a minimal kernel (ADR-006). Decisions taken with the user (2026-05-27):

- **M0 — kernel: process monitors (monitors-only, no links yet).** ✅ DONE
  (2026-05-27). `(monitor pid)` returns a `ref`; when `pid` dies the caller gets
  `[:down <mref> <pid> <reason>]` (`:normal` / `[:error msg]` / `:noproc`).
  `(demonitor mref)` stops it. See `docs/devlog.md` + `docs/language.md`.
- **M1 — `hatch`: the Brood process-framework library.** ✅ DONE (2026-05-27).
  `std/hatch.blsp` (embedded, `(require 'hatch)`): `defprocess` (state + `cast`/
  `call` clauses), `hatch` (spawn), `!` (cast), `gen-call` (synchronous, ref-
  tagged). cast body => next state; call body => `[reply next-state]`. Tested in
  `tests/hatch_test.blsp`; `examples/life.blsp` ported to it.
  - TODO (M1.x): a clean **stop**/terminate path (a clause that doesn't recurse);
    today a hatch process loops forever. Needed before supervisors can shut
    children down. Also: a `keep` shorthand for "no state change" (vs returning
    the state var), and init args beyond the single state value.
- **M2 — `hatch` supervisor.** spawn + monitor children, restart per strategy
  (`:one-for-one` / `:rest-for-one` / `:all-for-one`), checkpoint/resume,
  topologies (`:grid-2d`). API follows current Brood idiom (no `&key`).
- **M3 — surface sugar, later.** Each its own ADR.

**Explicitly rejected (keep current surface, ADR-011):** no Clojure-isms — no
callable collections `(board cell)`, no `#(…)` reader fn, no set type `#{}`. Stay
with current primitives.

## Plan: make `examples/life.blsp` simpler

The Game of Life (board = live cells as a map `[x y] -> true`) exposed friction.
Goal here is *simpler code*, not raw speed (HAMT is a separate perf item). The
target is to shrink the two central functions and drop the local `range` helper:

```clojure
;; AFTER tiers 1+2:
(defn neighbour-counts (board)
  (frequencies (mapcat neighbours (keys board))))          ; was an 8-line nested fold

(defn step (board)
  (reduce-kv (fn (next cell n)
               (if (or (= n 3) (and (= n 2) (contains? board cell)))
                 (assoc next cell true) next))
             {} (neighbour-counts board)))                 ; was (keys …) + per-cell (get …)
```

### Tier 1 — prelude only, no kernel change ✅ DONE (2026-05-27)

- [x] **`range`** — `(range hi)` / `(range lo hi)` / `(range lo hi step)`, plus a
  full sequence library (`take`/`drop`/`take-while`/`zip`/`partition`/`sort`/…).
- [x] **`mapcat`** — `(apply append (map f coll))`.
- [x] **`frequencies`** — `(fold (fn (m x) (assoc m x (inc (get m x 0)))) {} coll)`.
- Result: `examples/life.blsp` `neighbour-counts` is now
  `(frequencies (mapcat neighbours (keys board)))`, and the local `range` helper
  is gone. Tests in `tests/sequence_test.blsp`.

### Tier 2 — one kernel change ✅ DONE (2026-05-27)

- [x] **`map-pairs` is now the single map enumerator (replaced `map-keys`).**
  Returns `[[k v] …]` in one O(n) pass; `keys`/`vals`/`contains?`/`reduce-kv` and
  `empty?`/`count`-on-maps are all Brood over it. The map kernel stays five
  primitives (hash-map/map-get/map-assoc/map-dissoc/map-pairs) and the O(n²) `vals`
  is gone. `examples/life.blsp` `step` now uses `reduce-kv`. (Did not add `entries`
  — defer until something needs it.) See `docs/devlog.md` 2026-05-27.

### Out of scope / deferred

- ~~First-class set type `#{}`~~ — **rejected** (decision above: keep the current
  surface, board stays a map `[x y] -> true`).
- [ ] **HAMT persistent map** — O(log n) `get`/`assoc` instead of the O(n) assoc
  vector. This is the *perf* fix, not a simplicity one (surface unchanged), so
  it's separate from this plan; pairs with the tracing-GC migration (ADR-002).

## Done: `sleep` (pure Brood, in `hatch`)

- ✅ `(sleep ms)` in `std/hatch.blsp` — NOT a Rust primitive. A Rust `thread::sleep`
  would block a scheduler worker and starve other green processes; instead `sleep`
  pins a fresh `(ref)` in a `receive` (a clause no message can match) with an
  `(after ms)` timeout, so it parks the process on the scheduler timer and leaves
  the mailbox untouched. The naive `(receive (after ms nil))` was wrong — it eats
  the next queued message. Can move to the prelude once the freeze landmine (below)
  is fixed, since it uses `receive`.

## Bug: docstring dropped on functions with a destructured parameter ✅ FIXED (2026-05-27)

- [x] `(defn f ([x y]) "doc" body)` kept its docstring. Fixed in `lower_fn`
  (`crates/lisp/src/eval/macros.rs`): peel a leading docstring (string + more
  body) before the refutable-bind/`do` wrap and re-insert it as the lowered `fn`'s
  first body form, where `make_closure` looks. Regression test in
  `tests/introspection_test.blsp`. (Multi-clause docstrings remain unsupported —
  separate, pre-existing.)

## Concurrency / runtime follow-ups (from the `ref` work, 2026-05-27)

- [ ] **`match`/`receive` can't be used inside a prelude-level function** (debug
  builds): their macro expansion executes lambda-building library fns (`=`,
  `map`, the match compiler) at the prelude's own compile pass, stranding
  closures that `heap.freeze_as_shared_code`'s `debug_assert!(c.env.is_none())`
  rejects. That's why `call`/`reply` live in `examples/life.blsp`, not `std/`.
  Real fix: freeze-time reachability (drop unreachable closures) — falls out of
  the tracing-GC migration. See `docs/devlog.md` 2026-05-27.
- [ ] (revisit) `await`/process monitors (`link`/`monitor`, Erlang phase 6,
  `docs/concurrency.md`). Decided *not* needed for now — synchronous call/reply
  over `ref` covers "wait for a result". Reconsider if fire-and-forget
  supervision becomes a real need.
