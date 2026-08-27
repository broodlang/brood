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

**Workaround today.** Bounded `%iterate-times` already exists in
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
- Hot-reload (`system/reload-defs`) is not a redefinition — the origin matches
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
- `defn-` / `def-` for module-internal helpers (ADR-146 def-site privacy;
  when this was written the marker was `--` in the name, since retired).
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

## 6. Call-site argument literal precision for int/bool/string — ✅ SHIPPED (B0, 2026-07-07)

**Status.** Done, not deferred — the note below is kept for the record but the feature
exists. `Ty::of_value` (`crates/lisp/src/types/mod.rs:492`) now makes int and bool
literals singletons exactly like keywords, and `expr_ty`
(`crates/lisp/src/types/check/infer.rs:296`) builds the string singleton where it has
the heap. So `(status-handler 999)` against a declared `(or 200 404 500)` is caught at
type-check today, same disjointness path as the keyword case. The approach that shipped
was the *opposite* of the "call-site-only helper" sketch below: rather than add a second
inference path (which risks drift), B0 sharpened the single shared `of_value`/`expr_ty`
primitive, so there is exactly one inference path feeding both the disjointness check and
the rendered message. The message-wording churn the note feared was accepted as a
one-time ~19-assertion update (the sharper "got 5" text is strictly better).

--- historical note (the state when this was deferred) ---

**Why deferred.** A literal *keyword* argument at a call site already gets
static disjointness checking: `Ty::of_value` (the runtime-value → static-type
bridge) turns a literal keyword appearing in code into its singleton type, so
`(c-mode :bogus)` against a declared `(or :maximized :fullboth :fullscreen
nil)` is a provable disjointness the checker catches, not just the runtime
contract (`sig!`). Int/bool/string literal *types* are fully shipped
(ADR-117/120) — but only for **declared sigs**; a literal int/bool/string
*argument* doesn't get the same `of_value` treatment keyword arguments do.
Extending `of_value` for int was tried and reverted during the ADR-117 work:
`of_value` feeds *every* literal expression's inferred type throughout the
checker, not just call arguments, so making every int literal a singleton
changed the rendered text of unrelated misuse-warning messages project-wide
(`"got int"` → `"got 5"`), breaking 7 pre-existing, unrelated tests on exact
wording. Bool/string weren't attempted at all, on the same expectation.

**Design sketch.** The wording-churn problem is `of_value`'s blast radius, not
the underlying idea — it's a single, deeply-shared function, so widening its
output type touches every call site that renders a type in a message, not
just the disjointness check this is actually for. A narrower approach: instead
of changing what `of_value` returns everywhere, add a **call-site-only** path
— a small helper that inspects a literal *argument expression* specifically
(int/bool/string, mirroring the existing keyword recognition) and feeds that
singleton type into the disjointness check alone, leaving every other
consumer of inferred literal types (rendered messages, guard narrowing,
`let`-binding types) on the current, coarser flat-tag inference. This sidesteps
the wording-churn risk because it's additive at one call site rather than a
change to a shared primitive. Needs care that the two paths (the new
call-site-only inference and `of_value`'s general one) can't disagree in a way
that produces an inconsistent warning.

**Trigger to pick this back up.** A concrete case where a literal
int/bool/string argument slips past the checker that a literal keyword
argument in the identical shape would have caught — i.e., real evidence the
asymmetry between keyword and the other three kinds costs something, not just
that it's there.

**Workaround today.** `sig!` still catches the mismatch at runtime (the
enforcement path doesn't depend on `of_value` at all); the gap is purely in
the *static* checker's precision for this one case shape.

## 7. Lexically-shadowable operators ("Option C") — Lisp-1 without reserved-word cost

**Context.** Brood is a Lisp-1 (ADR-007), so every macro/special form occupies the
single namespace and becomes a **reserved operator word** — you can't bind a local
named `when`, `for`, `cond`, `doseq`, etc. and call it in head position, because the
expander resolves the operator to the macro before lexical scope exists. (This first
surfaced with a prototyped `loop`/`recur` macro, which was then **dropped** — see
ADR-154 — so `loop` is *not* reserved today; but the ~40 other macros/special forms
still are.) **Decision taken (2026-07-26): keep reserved words (Option A)** — the
limitation is minor, universal to Lisps, and buys dead-simple certainty (`(when …)`
is *always* the macro, no scope-tracing). This item records the more ambitious
alternative for when a concrete need appears.

**The idea (C).** Make the expander resolve operator position against **lexical
scope first**: a *free* `for` expands the macro; a `for` that is `let`/`letrec`/
`fn`-param-bound calls the local. This removes the reserved-word cost entirely
while keeping Lisp-1's `(f x)` ergonomics — strictly less limiting than Clojure
(where a macro name genuinely can't be shadowed). It makes macros consistent with the
*function* shadowing Brood already allows (`(let (map …) (map x))` already calls
the local).

**Why not Lisp-2 instead.** Lisp-2 would also free the name (a *variable* `loop`
lives in a different namespace than the operator), but it taxes **every**
higher-order call with `funcall`/`#'` — which guts the fold/map/closure-passing
style that is Brood's whole idiom. Rejected.

**Properties that make C tractable.**
- **Lexical containment.** A shadow applies only within the text where it's bound;
  it does **not** leak into other functions you call (their bodies were resolved in
  their own scope) — identical to how function shadowing already behaves.
- **Composes with hygiene.** Free references in a macro template auto-qualify to
  the macro's namespace (ADR-066 α), so a macro that expands to `(for …)` emits a
  *qualified* `for` a caller's local can't capture. C relies on this, and it is
  already automatic.

**Gotchas to design around (the reason it's deferred, not done).**
1. **Scope-aware expander is a chicken-and-egg.** Binding forms like `when-let`/
   `for`/`doseq` are macros that expand *into* the core scope-introducers
   (`let`/`letrec`/`fn`), so the expander must track scope while expanding
   outside-in — turning a flat pass into a scope-tracking one and enlarging the
   compile-correctness surface.
2. **Control-flow macros are the scary shadows.** `and`/`or`/`when`/`cond` are
   macros; silently shadowing them changes control flow invisibly. C should either
   free only *library* operators (`for`, `doseq`, threading) and keep control macros
   reserved, or make the shadow-lint a **hard error** for control-flow macros.
3. **Static editor grammars can't reflect scope.** `nest grammar` emits a static
   keyword list (ADR-092); only the LSP's semantic tokens can color a shadowed
   `for` correctly. So "highlighting tells you which one" holds only in
   LSP-backed editors.
4. **Knowability must be engineered, not assumed.** Ship C *with* a shadow lint
   ("local `for` shadows the `for` macro", loud by default) + semantic
   highlighting. C without the lint is a readability hazard — the whole reason A is
   the safer default.
5. Minor: scope-tracking expansion adds cold-start cost (expansion is already the
   bulk of the ~31ms boot).

**Relation to full hygiene.** Brood is now hygienic **both ways automatically**, like
Elixir: free-reference auto-qualification (the part C needs) *and* introduced-binding
capture protection (automatic alpha-rename of template `let`/`fn` binders, `~'name` to
opt out — ADR-066 amendment, 2026-07-30) are both default. C's scope-aware *operator*
resolution is a further step in the same direction (it reuses the same binder-scope
tracking the hygiene rename and the namespace resolver already do), so they compose.

**Trigger to pick this up.** Real evidence that reserved operator words cost
something — a user (or downstream project) that genuinely wants a local named for a
reserved operator and finds `go`/rename unacceptable — plus appetite for the
scope-aware expander. Until then, A stands.

## 8. Inline `sig`s — `(defn f ((x int) -> int) …)` — deferred, optionality is the doubt

**Context.** Today a function's signature lives in a *separate* `(sig f (int -> int))`
form, conventionally below the `defn`, with an ordering constraint that only bites
under `BROOD_CONTRACTS=1` (the sig must be in scope when the contract shim wraps).
The frequently-proposed "modern" alternative is to fold the types into `defn`'s
parameter list — `(defn f ((x int) -> int) …)` — the ML/Rust/TypeScript/Elixir-`@spec`
shape. Tracked in ROADMAP.md and roadmap-for-v1.md as an ADR-082 revision.

**Why it's deferred (and the specific doubt).** It's purely additive, so it costs a
version number to wait (ADR-011). But the deeper reservation — recorded here as the
thing to resolve before picking it up — is that **a sig is *optional*, and inlining an
optional annotation into the mandatory `defn` form is in tension with that
optionality.** Keeping the signature a separate opt-in form means the common,
un-annotated `defn` stays clean and the annotation is visibly a *separate choice* you
add, rather than a slot in the definition form that reads as "left blank." Inline types
are natural in languages where the annotation is *expected* (or mandatory); Brood's
annotations are sparse and opt-in (three modules today, ADR-153), which is exactly the
regime where a separate form arguably fits better. The roadmap-for-v1 judgment call is
the same axis seen from the release side: inline only earns "do it now" *if* types are
meant to be widely annotated by 1.0; if they stay sparse, the separate form is fine and
this waits.

**What to decide when picked up.** Whether the optionality argument wins, or whether
there's a spelling that stays optional yet inline (e.g. inline types allowed but never
required, `(defn f (x y) …)` still legal alongside `(defn f ((x int) (y int) -> int) …)`)
— and whether that dual shape is worth the `defn`/`sig_of`/`defrecord`-emitted-sigs/`sig!`
churn. The ergonomic/precision gap is author-time, not a runtime footgun, so there's no
urgency.

**Workaround today.** Write the `(sig …)` form; it's the documented spelling and works.

---

## 9. `eval` runs interpreted — a 14x cliff for every runtime-evaluated form

**✅ RESOLVED 2026-08-01.** `eval_builtin` and `eval_string_inner` now route through
`eval::compile::run` when `vm_enabled()` (tree-walker under `BROOD_VM=0`), so a
runtime-evaluated form's top-level call dispatches into the compiling VM instead of the
~14× tree-walker; `compile::run` falls back to the tree-walker per-form for anything
outside the VM's vocabulary, so semantics are unchanged. Both keep the full `compile` pass, so an eval'd
form still gets namespace resolution, imports, aliases, privacy and static-quasiquote
lowering. `compile`'s `resolve` step qualifies a bare name only on positive evidence, which
a file loader supplies by pre-scanning its def heads — lookahead a one-form-at-a-time `eval`
lacks, so a forward reference across independent `eval` calls broke (KI-24); the two
pre-scan-less call sites now set `ns_assume_own`, which supplies that conclusion instead of
dropping the pass. So a runtime-evaluated form's top-level call now dispatches into the VM, where the
callee's arm compiles and tail-recurses in O(1) stack — the cliff is gone (measured: `eval`
of the million-iteration loop went from ~14× the compiled time to parity). Correctness
pinned form-by-form in
`tests/eval_vm_test.blsp` (arithmetic, deep tail recursion, closures capturing the
runtime env, macros/quasiquote, cross-process round-trip), green under VM /
tree-walker / no-JIT. **One behaviour change to note:** `std/tool/eval-server`'s
`:trace` mode — boundary tracing costs TCO (each traced call is a real frame), and the
VM's recursion budget (~1M) is far above the tree-walker's, so a *traced* deep tail
loop that used to overflow the interpreted eval now completes at moderate depth
(`eval_server_test` updated to the engine-robust fact: tracing captures each self-call,
i.e. it costs TCO, rather than the tree-walker-specific overflow depth). The original
analysis is kept below for history.

---

**The bug.** `eval` (and `eval-string`, and therefore every consumer that
evaluates source it was handed at runtime) walks the form with the tree
evaluator, while the same code loaded from a file goes down the compiled path.
The gap is not marginal. Measured on one machine, same million-iteration
tail-recursive loop, `nest run` in a bare image:

| path | time |
|---|---|
| compiled (loaded from the file) | **205 ms** |
| compiled, wrapped in `%capture-begin`/`%capture-take` | 189 ms |
| `(eval form)` on the read form | **2962 ms** |
| `(eval-string src)` | 3885 ms |

Reproduce:

```lisp
(defn cd ((0) :liftoff) ((n) (cd (- n 1))))
(let (t (now) _ (cd 1000000)) (println "compiled: " (- (now) t) "ms"))

(def SRC "(defn cd2 ((0) :liftoff) ((n) (cd2 (- n 1))))\n(cd2 1000000)")
(let (t (now) _ (fold (fn (_ f) (eval f)) nil (reflect/read-all SRC)))
  (println "eval:     " (- (now) t) "ms"))
```

Note the second row: **output capture is not the cost.** It was the first
suspect and it is innocent — 189 ms with it, 205 ms without. Anyone picking this
up should not re-chase that.

**Why it matters beyond a microbenchmark.** Everything that evaluates
user-supplied source pays it: `std/tool/eval-server` (so every playground box in
the editor's tutorial, and every `C-x C-e`), the REPL, `nest run -e`, the MCP
eval tool, and any Brood program using `eval` for configuration or plugins. In
the editor it is user-visible: a tutorial lesson demonstrating "a million tail
calls in constant stack" exceeded its 2 s evaluation budget and rendered as
`✗ timed out`, discrediting the exact claim it was making. The lesson had to be
sized down to 250,000 to fit — a workaround for this entry, and it says so at
the call site (`bedit`, `src/tutor-lessons.blsp`, "Recursion is the loop").

**Where it is.** `crates/lisp/src/builtins/system.rs`, `eval_builtin` — three
lines:

```rust
pub(super) fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    crate::eval::eval(heap, form, root)
}
```

`crate::eval::eval` is the tree-walker. `eval-string` reads and then funnels into
the same place, so there is no cheap swap at the Brood level — this has to change
in the runtime.

**Design sketch.**
- Route `eval_builtin` through whatever entry the file loader uses to compile a
  top-level form, falling back to the tree-walker if compilation declines the
  form. Establish first whether that entry is safe for a form arriving at
  runtime: the loader knows its module and file context, and `eval` may not.
- Decide the caching story. The sandbox re-evaluates the *same* box text on every
  debounce beat, so a compile-per-eval could hand back the win it just earned;
  keying compiled code by form identity (or by source string, for `eval-string`)
  is probably where the real gain is.
- Watch the semantics that make `eval` different from `load`: the environment it
  evaluates in (`env_root` here), macroexpansion order, and closures capturing a
  runtime env. A miscompile here fails silently rather than loudly, so this wants
  tests that compare tree-walked and compiled results form-by-form, not just
  timings.
- Worth checking whether the JIT tier is reachable at all from this path, or
  whether "compiled" here means only the bytecode compiler.

**Trigger to pick this back up.**
- Any interactive Brood surface where evaluation latency is felt — the editor's
  eval-on-type is the live one today.
- Or a second lesson/benchmark that has to be shrunk to fit a timeout.

**Workaround today.**
- Put hot code in a file and `require` it rather than `eval`ing it.
- Size interactive evaluation to the interpreted speed (~14x), which is what the
  tutorial now does.

---

## 10. No pty primitive — a terminal app cannot drive another terminal app

**Why it is wanted.** `proc-spawn` gives a child piped stdio, which is right for a JSON-RPC
peer or a build tool but not for anything that needs a *terminal*: an editor over
`*term-display*` puts its tty in raw mode, asks for the window size, and re-renders on
SIGWINCH. Piped stdio has none of that, so a Brood program cannot start another Brood
program's terminal UI and interact with it.

That matters because it is exactly how you verify a terminal app *as a user*: press a key,
assert on what is painted. bedit's live drivers (`tools/` in that repo) do this and have
caught two bugs the 1200-test model suite structurally could not see — a keybinding present
in the help vocabulary but never `keymap-bind`-ed (the vocabulary and the keymap are two
tables; only pressing the key crosses them), and a window attribute that lives in a
protocol message rather than in the model. They are written in **Python**, in a project whose
rule is that everything is written in Brood — the one place that rule is broken, and only
because of this gap.

**Design sketch.** A `pty-spawn` sibling of `proc-spawn`: same handle, same
`[:proc handle data]` mailbox delivery, but the child gets a pty as its controlling terminal.
Then `(pty-resize handle rows cols)` (TIOCSWINSZ + SIGWINCH, which is what makes "force a
full repaint" possible) and nothing else new — writing to the handle is the existing
`proc-send`, and the child's output arrives as it already does. Unix-only, like much of
`proc/*`; a `pty?` feature keyword keeps a portable program honest.

**Trigger to pick this back up.**
- A second consumer wanting it (a terminal-app test harness in std, `nest` driving an
  interactive subcommand, a multiplexer experiment).
- Or the moment the Python drivers need to grow: a shared assertion vocabulary in Python is
  a second, worse test framework living next to `std/tool/test`.

**Workaround today.** Python's `pty` module (`openpty` + `TIOCSCTTY` + `TIOCSWINSZ`), ~120
lines of harness — see `../bedit/tools/drive.py`, which documents the three ways such a
harness lies to you (face escapes splitting phrases, the stream being history rather than the
screen, and slow first paint).
