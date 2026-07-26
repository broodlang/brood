# The Gabriel / Larceny Scheme benchmarks

Real Lisp programs with known answers, ported to Brood and run as a conformance suite.
Every other corpus in `tests/corpus/` feeds hostile *data* to one parser; this one runs
whole *programs*, which is the only way to get an oracle for the evaluator itself. A
theorem prover that reports 95,024 rewrites, a maze whose 121 cells are all correct, or
ten exact 500-digit integers cannot be produced by an engine that mis-stages a closure
slot, loses a branch under GC, or miscompiles an arm.

- **Upstream**: <https://github.com/ecraven/r7rs-benchmarks> — "Taken with kind permission
  from the Larceny project, based on the Gabriel and Gambit benchmarks."
- **Pinned commit**: `85f6acdc4cc4e2b857f307ba56bd0ba931dcccd1`
- **Runners**: `tests/conformance_gabriel_test.blsp` (upstream's oracles, on the VM) and
  `crates/lisp/tests/gabriel_engines.rs` (the same oracles, tree-walker *and* VM)
- **Ports**: `tests/support/gabriel/<name>.blsp`
- **Refresh**: `scripts/fetch-corpus.sh gabriel` (add `--full` for the unported sources)

## What is committed, and why the sources are not

| path | committed | what it is |
|---|---|---|
| `data/*.input` | **yes** | the benchmark's own driver data: an iteration count, the arguments, and the **expected result**, in upstream's `(read)` order |
| `reference/*.scm` | **no** (gitignored) | the Scheme sources the ports were written from |

Only `nboyer.scm`/`sboyer.scm` carry an explicit `Status: Public Domain`. For the rest the
chain — Gabriel's book → Gambit (LGPL/Apache) → Larceny (a notice-only permissive grant) →
ecraven (no `LICENSE` file) — is not cleanly attributable per file, so this repo vendors
only the `.input` oracle data and fetches the sources on demand. The ports are our own
expression of the algorithms; each one's header records exactly what changed and why.

## The ported programs

| program | what it exercises | oracle | cost (release) |
|---|---|---|---|
| `deriv` | symbolic differentiation; `cons` per node | the full ~90-node result tree | <1 ms |
| `takl` | Takeuchi over lists as counters; triple non-tail recursion | result length 7 | 31 ms |
| `cpstak` | CPS: a fresh capturing closure per step, all calls in tail position | 7 | 29 ms |
| `mazefun` | a purely functional maze generator (already immutable upstream) | all 121 cells | 19 ms |
| `nqueens` | backtracking; short-lived list allocation | solution counts, n = 1..10 | 0.3 s |
| `primes` | the list sieve; cascading rebuilt lists | every prime ≤ 1000, in order | 4 ms |
| `nboyer` | 106 rewrite rules, unification, tautology checking | **the rewrite count** | 0.26 s |
| `chudnovsky` | π by binary splitting — sustained big-integer arithmetic | ten exact 50–500-digit integers | 17 ms |

### Where each expected value comes from

Three provenances, because they differ per benchmark and this is the basis of the suite's
authority:

**(a) The vendored `.input` file's live stanza** — upstream's own bytes, read with
`corpus-forms` and never retyped: `deriv`, `mazefun`, `primes`, `chudnovsky`.

**(b) An older stanza upstream still keeps in the same `.input` file.** The current stanzas
are sized to *time* a native-compiling Scheme, not to be checked: Chez takes 3.97 s for one
iteration of `takl:40:20:12` and 3.46 s for `cpstak:40:20:11`. `cpstak.input` keeps three
historical stanzas as live data (only the prose between them is commented), so the runner
reads the third. `takl.input`'s older stanzas are `;;`-commented, so its three sizes are
transcribed from that comment block:

```
;; ; The old inputs and output for takl were:
;; 600
;; (a list of 18 elements)
;; (a list of 12 elements)
;; (a list of 6 elements)
;; 7
```

**(c) An external published table**, for the two cases with no machine-readable upstream
value at a size that fits a test suite. From `nboyer.scm`'s header — it reports rewrites
rather than the boolean "because it is too easy for a buggy version to return the correct
boolean result":

```
;;     n      rewrites       peak live storage (approximate, in bytes)
;;     0         95024           520,000
;;     1        591777         2,085,000
;;     2       1813975         5,175,000
;;     3       5375678
;;     4      16445406
;;     5      51507739
```

The port reproduces **95024 / 591777 / 1813975** exactly. Matching a six-figure count is a
much stronger statement than matching the boolean: it pins rule *order*, the unifier and
the tautology walk together, so a port or an engine that takes one wrong branch lands on a
different number rather than a near miss. `nboyer.input`'s own stanza is n=5 (51.5 M
rewrites, 2.2 s in Chez) — minutes here, so it is not run.

For `nqueens`, upstream's stanza is n=13 → 73712, which costs ~40 s here; the runner uses
[OEIS A000170](https://oeis.org/A000170) for n = 1..10 instead — ten independent answers
including the two n with no solutions, which is a better test than one number anyway.

## Not ported, and why

Each of these was read before being excluded; none is a "todo".

- **`gcbench`** — no oracle *and* structurally impossible. Its correctness predicate is
  literally `(lambda (result) #t)`, so it checks nothing. More fundamentally, its subject
  is `Populate`, which builds a tree top-down "assigning to older objects" — the write
  barrier / old-to-young pointer case. Brood has no such case by construction: data is
  immutable, so old never points to young, which is exactly why the collector needs no
  write barrier for data (`CLAUDE.md`). There is nothing here to port and nothing to test.
- **`destruc`** — the "destructive operation benchmark": `set-car!`/`set-cdr!` splicing over
  a list of lists, where the aliasing *is* the program. A functional port would not be the
  same benchmark, and reproducing observable sharing semantics without mutation is a
  rewrite with no oracle to catch a mistake.
- **`peval`** — Feeley's partial evaluator, which rewrites the program tree **in place**
  through twelve `set-car!`/`set-cdr!` sites over a `where` pointer into the AST. Porting
  it means re-deriving the algorithm in rebuilding form; the oracle (one output program)
  would not localise a mistake. A genuine candidate for later, not a cheap one.
- **`earley`** — Feeley's Earley parser: 25 `vector-set!` sites into mutable state vectors.
  Same shape of work as `peval`, same reason deferred.
- **`conform`** — Jim Miller's type-lattice checker; mutable tables plus `set-car!`.
- **`nucleic`** — the pseudoknot benchmark. Mostly functional and a superb float oracle
  (upstream expects `33.797594890762724`), but 3,485 lines. The best next candidate.
- **`ctak`/`fibc`** — need `call/cc`, which Brood does not have.
- **`tak`/`ack`/`fib`/`sum`/`fibfp`/`sumfp`/`diviter`/`divrec`** — trivial to port, but our
  own benchmark suite already covers these shapes; the corpus's value is where we have no
  independent oracle.

## Findings

Both findings are *outside* the programs under test — one in the type checker, one in the
test harness. That is a first for these corpora, and it is what running whole programs
through the real toolchain buys over feeding bytes to a parser.

**KI-13: `nest check` hangs on the `deriv` port (2026-07-26, open).** Cross-module
return-type inference for an undeclared recursive callee blows up exponentially in the
number of `cond` branches that build nested list structure — 2/3/4/5 branches cost
105 ms / 105 ms / **8.7 s** / did not finish in 900 s. The same call inside the defining
module is instant, so it is specifically the `sig_of` → `infer_sig` → `expr_ty` path across
a module boundary, where nothing bounds the *size* of the inferred type. `nest check` is a
CI gate and the checker backs the LSP, so a hang there is worse than a wrong warning.
Worked around in `tests/support/gabriel/deriv.blsp` with `(sig deriv (any -> any))`, which
is consulted before body inference — honest as a type, and load-bearing. Full repro,
scaling table and likely fix in `docs/known-issues.md`.

**A gap in the engine-differential gate (2026-07-26).** Wiring this suite turned up
something about the *harness* rather than the language: `BROOD_VM=0` does not give the
in-language test suite tree-walker coverage. A test body run by `nest test` (or
`brood --test`) under `BROOD_VM=0` shows no slowdown at all, and `BROOD_JIT_DUMP_IR=1`
lists its arms reaching the JIT — the env var gates how a *top-level form* is run, while
the test framework invokes each test as an already-compiled closure. The same function at
top level (`brood file.blsp`) correctly interprets and produces zero JIT arms, and is ~10x
slower.

So the tree-walker leg of `make test-both` does not exercise the ~3400-case in-language
suite the way the Makefile comment implies; per-expression engine agreement comes from
`crates/lisp/tests/differential.rs`, which pins the engine with `set_forced_engine` rather
than the env var. That is why this corpus has a Rust runner as well as a `.blsp` one:
`gabriel_engines.rs` uses the same `set_forced_engine` mechanism, so these programs really
do run on both engines.

**No divergences, and no wrong answers.** All eight ports match upstream on the tree-walker
and on the VM+JIT, including `nboyer`'s three rewrite counts and `chudnovsky`'s ten exact
big integers. Two engine-specific limits were measured and are worth recording:

- The debug-build tree-walker spends ~12.6 kB of native stack per frame, so `primes<=1000`
  (999 levels of non-tail `interval-list`) exceeds the 12 MB budget there and raises a
  clean `recursion too deep` — correct behaviour, but it is why the Rust runner sieves to
  100 instead. The release tree-walker handles 1000 fine, as does the VM in either profile.
- Tree-walker cost in a debug build, versus the VM: `nboyer` n=0 is 38 s vs 0.25 s, `takl`
  13 s vs 30 ms. Those two are therefore an `#[ignore]`d test, run with
  `cargo nextest run -p brood --test gabriel_engines --run-ignored all`.

That a 40-year-old theorem prover, a CPS benchmark and a big-integer π series all landed on
upstream's exact numbers on the first run of both engines is a real result for the
evaluator — and the finding above is the argument for running them through
`set_forced_engine` rather than trusting an env var.
