# TODO

Running scratch list of work to pick up. Promote items to `docs/roadmap.md` /
an ADR once they're committed to. Newest section at the top.

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

### Tier 1 — prelude only, no kernel change (do first; unblocks the rewrite)

- [ ] **`range`** — `(range n)` (and `(range start end)` via `&optional`). Deletes
  the example's local `range`/`range-down`.
- [ ] **`mapcat`** — `(defn mapcat (f xs) (apply append (map f xs)))`.
- [ ] **`frequencies`** — `(fold (fn (m x) (assoc m x (inc (get m x 0)))) {} xs)`.
  Collapses `neighbour-counts` to one line.
- [ ] (optional) `repeat`, `take`/`drop` (today only in `std/test.blsp`), `concat`
  alias for variadic `append`. Not needed by Life; add if cheap.

### Tier 2 — one kernel change, and it *shrinks* the kernel

- [ ] **Make `map-pairs` the single map enumerator (replacing `map-keys`).** Maps
  have no way to fold key+value together, so `step` does `(keys counts)` then a
  per-cell `(get counts cell)` — a second lookup, and `get` on the assoc-vector
  is O(n) → an O(n²) fold. Fix: one primitive `map-pairs` returning `[[k v] …]`
  in one O(n) pass (trivial over the insertion-ordered assoc vector), and derive
  *everything else in Brood*: `keys` = `(map first (map-pairs m))`, `vals`,
  `contains?`, `reduce-kv`, `entries`, even `count`. Net: the kernel loses a
  primitive (drop `map-keys`) instead of gaining one — fits the current
  minimization (you already dropped `map-vals`/`map-contains?`) — and the O(n²)
  is gone. Then `step` uses `reduce-kv`.

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

## Bug: docstring dropped on functions with a destructured parameter

- [ ] `(defn f ([x y]) "doc" body)` loses its docstring — `(doc f)` → nil — because
  the single-clause pattern-param path (which delegates to the match compiler in
  `std/prelude.blsp`) doesn't thread the leading-string docstring through. Plain
  params keep it. Hit by `neighbours` in `examples/life.blsp`. Fix in the `fn`
  pattern-param handling: peel a leading docstring before compiling the param
  pattern, and reattach it to the closure.

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
