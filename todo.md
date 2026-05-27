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

## Language improvements surfaced by `examples/life.blsp`

Writing Conway's Game of Life (board = a set of live cells as a map `[x y] ->
true`) exposed friction worth fixing. Ordered cheap → substantial.

### Cheap, high-leverage (pure-Brood prelude policy)

- [ ] **`range`** in the prelude. The example defines it locally; it's standard
  and broadly useful. `(range n)` and `(range start end)`.
- [ ] **`mapcat`** — `(defn mapcat (f xs) (apply append (map f xs)))`.
- [ ] **`frequencies`** — count occurrences into a map. With `mapcat` it collapses
  the neighbour tally to `(frequencies (mapcat neighbours (keys board)))`.
- [ ] (maybe) `concat` as a clearer alias for variadic `append`; `repeat`, `take`/
  `drop` (currently only in `std/test.blsp`).

### Small kernel addition (removes a real O(n²))

- [ ] **`map-pairs` primitive + `reduce-kv`/`entries` in prelude.** Maps expose
  `keys`/`vals` but no way to fold over key+value together, so `step` does
  `(keys counts)` then `(get counts cell)` — a second lookup per cell, and `get`
  on the assoc-vector is O(n), making the fold O(n²). `map-pairs` returns
  `[k v]` pairs in one O(n) pass (trivial over the insertion-ordered assoc
  vector); `reduce-kv`/`entries` build on it. Both a clarity win and a speedup.

### Bigger, deliberately deferred (ADR-011 — defer power features)

- [ ] **First-class set type `#{…}`.** The board is a set wearing a map costume
  (`[x y] -> true`, `keys`, "dedup by key"). A real set (`#{…}`, `conj`,
  `contains?`, `union`) would let the code say what it means. New `Value` kind +
  `Tag` + literal syntax + ADR; check against the compatibility contract in
  `docs/types.md`. Life is a *nice* motivation, not yet a forcing one.
- [ ] **HAMT persistent map** to replace the O(n) assoc-vector with O(log n)
  `get`/`assoc`. The substantive perf fix (Life is ~O(n²) today); large,
  self-contained kernel work. Surface unchanged — `docs/language.md` already
  promises the swap is invisible. Pairs naturally with the tracing-GC migration
  (ADR-002).

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
