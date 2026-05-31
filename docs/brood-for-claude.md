# Brood — a quick reference for Claude

A pocket guide for writing `.blsp` (Brood Lisp) — what to do, what *not* to, and
the patterns that aren't shared with other Lisps. For depth see
`docs/language.md`, `docs/spec.md`, `docs/pattern-matching.md`,
`docs/concurrency.md`.

## What Brood is (and isn't)

A small, dynamic Lisp implemented in Rust.

- **Immutable data** (ADR-026). There's no `set!` / `setq`, no atoms, no cells
  — every operation returns a fresh value. The only mutation is `def`, which
  *re-binds* a global (hot reload). State that genuinely changes lives in a
  **process** (`spawn` / `send` / `receive`) or behind a Rust-backed handle.
- **No loops.** Use recursion (proper tail calls are guaranteed, including
  tail calls to *other* functions) or the higher-order combinators
  `fold` / `reduce` / `map` / `transduce`.
- **Truthy / falsy**: only `nil` and `false` are falsy. `0`, `""`, `()` are
  *truthy*.
- **Late binding**: globals can be re-defined; a redefinition is visible to
  every running process on its next lookup.

Files end `.blsp`. Run a file with `brood file.blsp`; REPL with bare `brood`;
project tooling via `nest test` / `nest run` / `nest new <name>`.

## Syntax

```
;; comment to end of line
42  -3  3.14            ; int (i64), float (f64)
"hello\n"               ; string — escapes: \n \t \r \e (ESC) \0 \\ \" plus
                        ;   \xHH (two-hex byte) and \u{H..H} (Unicode codepoint)
true  false  nil        ; booleans, nil
:keyword                ; keyword — interned, self-evaluating
name  foo-bar?  +       ; symbol (kebab-case is idiomatic)
(f a b)                 ; call / list
[1 2 3]                 ; vector — O(1) indexing
{:a 1 :b 2}             ; map — immutable, insertion-ordered (no commas)
'x   `(a ~b ~@xs)       ; quote / quasiquote / unquote / splice
```

## Special forms

Only these are *special*; everything else is a function or a macro:

```
def  defmacro  fn  lambda  quote  quasiquote  if  do
let  let*  letrec  defdyn  binding
```

Common macros (expanded once at the compile pass — runtime-free): `defn`,
`cond`, `when`, `unless`, `and`, `or`, `match`, `try` / `catch`, `->`, `->>`,
`receive`, `spawn`.

## Defining things

```lisp
(defn greet (name) (str "hello, " name))            ; defn = (def greet (fn (name) ...))
(defn add (& xs) (fold %add 0 xs))                  ; variadic via & rest
(defn opt-arg (x &optional (y 10)) (+ x y))         ; optionals with defaults

(defmacro when (test & body) `(if ~test (do ~@body) nil))
(defmacro my-or (a b) `(let (r# ~a) (if r# r# ~b)))  ; `r#` = auto-gensym: a fresh,
                                                     ; non-capturing binder (no manual gensym)

(def *flag* true)                                   ; global; def re-binds (hot reload)
(defdyn *log-level* :info)                          ; dynamic variable
(binding (*log-level* :debug) (do-thing))           ; scoped rebind
```

A `fn`/`defn` body of several forms is an **implicit `do`**: each is evaluated
for effect and the **last form's value is returned** — no explicit `(do …)`
wrapper needed (`((fn () 1 2 3))` → `3`). Same for `let`/`when`/`loop` bodies.

`fn` is multi-clause two ways (don't mix them in one `defn`):

```lisp
(defn arity-fn                       ; multi-ARITY: dispatch by arg count (Clojure-style)
  ((x)   (arity-fn x 0))             ; param lists are LISTS (x), not vectors [x]
  ((x y) (+ x y)))

(defn classify                       ; multi-PATTERN: same arity, dispatch by shape + :when guard
  ((0)               :zero)
  ((n) :when (< n 0)  :neg)
  ((n)               :pos))
```

Arity arms (plain-symbol heads, optionally with `&`/`&optional`) bind directly and
are cheap — this is how the prelude's variadic `+`/`-`/`<`/`=` stay fast. Pattern
heads (literals/destructuring) use the matcher.

**Trap — `&optional` defaults & patterns don't combine with arity overloading.** An
`&optional` slot must be a plain symbol (it can't be a pattern). A clause head with
a `(default …)` optional form or a pattern is matched as a *pattern* clause, so its
`&optional` is read literally — don't expect it to act as an arity marker. Pick one
mechanism per `defn`. Required params *can* still be patterns next to `&optional`
(only the optional/rest slot is restricted). To branch on an optional, bind it and
`match` in the body (`nil` = omitted):

```lisp
(defn h (x &optional opt) (match opt (nil [:no x]) (v [:yes x v])))
;; NOT: (defn h ((x) …) ((x &optional (y 9)) …))   ; &optional in a clause head → match error
```

Local bindings — `let` takes a **flat** name/value list (not Scheme's double-parens), and is sequential (each binding sees the earlier ones):

```lisp
(let (a 1
      b (+ a 1)         ; sees a
      [x y] some-vec)   ; destructuring works in the binding target
  (+ a b x y))
```

## Style — lists for code, vectors for data

Two rules that keep Brood code uniform and unambiguous. Both are about *idiom*;
both forms parse either way, but write the idiomatic one.

**1. Code uses `( )`; vectors `[ ]` are for data.** Param lists and the binding
forms of `let` / `for` / `doseq` / `when-let` / `if-let` are *lists*, not
Clojure-style vectors. Vectors are reserved for tuple values (`[x y]`),
sequence literals (`[1 2 3]`), and tuple **patterns** that match against tuple
values inside `match` / `let` / `receive` heads. Code is cons-lists so the
editor and macros manipulate one structure uniformly (ADR-010).

```lisp
;; good                          ;; not idiomatic
(let (a 1 b 2) …)                (let [a 1 b 2] …)
(for (x xs :when p) …)           (for [x xs :when p] …)
(doseq (x xs) …)                 (doseq [x xs] …)
(when-let (v (try-it)) …)        (when-let [v (try-it)] …)
```

**2. Don't tuple-destructure in a single-clause top-level `defn` param list.**
Name the param and unpack inside the body. Multi-clause `defn` (pattern
dispatch on clauses) is fine and encouraged — its clause heads use lists, not
vectors, so there's no ambiguity. Anonymous `fn` in higher-order context
(`map` / `reduce` / `mapcat`) **may** keep a tuple-destructured param — the
surrounding `(map …)` makes "this is a one-call function value" obvious, and
the alternative is a noisy extra `let`.

```lisp
;; good
(defn area (p) (let ([x y] p) (* x y)))

(defn neighbours (cell)
  (let ([x y] cell)
    (map (fn ([dx dy]) [(+ x dx) (+ y dy)]) offsets)))

;; multi-clause defn is fine — clause heads are lists, no [ ] collision
(defn fac
  ((0) 1)
  ((n) (* n (fac (- n 1)))))

;; not idiomatic — single-clause defn with a tuple-destructured param
(defn area ([x y]) (* x y))
(defn neighbours ([x y])
  (map (fn ([dx dy]) [(+ x dx) (+ y dy)]) offsets))
```

**Why rule 2:** `(defn f ([x y]) body)` is *single-clause* with one
tuple-destructured param, but visually collides with *multi-clause* `(defn f
((p) body))` where the outer `(…)` wraps a clause. The disambiguation is
correct (the parser checks whether the inner head is a list); the *reader*
pays a re-parse every time. The cost is highest at a top-level `defn` — that
name is the thing readers look up later. Confining the rule there preserves
the ergonomic `(map (fn ([k v]) …) …)` idiom, which reads locally and never
gets looked up by name.

## Naming & docstrings

These conventions are followed without exception across `std/` — match them and
your code will read like the standard library.

**Names carry their role in their spelling:**

```
foo?         ; predicate — returns a boolean (int? empty? starts-with?)
*foo*         ; dynamic var or module-level config/state (defdyn *log-level*)
foo--bar      ; PRIVATE helper — the double-dash infix marks "implementation
              ;   detail, not public API" (append--onto, cmp--gt, reload--loop)
foo->bar      ; conversion (number->string, vec->list)
```

There is **no `!` convention** — nothing mutates, so no name needs to warn of it.
Symbols are kebab-case (`out-of-range?`, not `outOfRange`/`out_of_range`).

**Tail-recursive helpers get a suffix naming what they accumulate or do** —
`--acc`, `--at`, `--loop`, `--onto`, `--scan`. The public function is a thin
shell; the `--`-suffixed helper does the real recursion with an accumulator:

```lisp
(defn reverse (coll) "The items of `coll` in reverse order." (fold flip-cons nil coll))

;; longer recursions split into a public shell + a private --acc helper
(defn count-newlines--at (s i acc) …)              ; private worker
(defn count-newlines (s) "Number of \\n in `s`." (count-newlines--at s 0 0))
```

**Docstrings** go on every public `defn` / `defmacro`. First line is a complete
one-sentence summary (it's what `(doc 'name)` and the LSP show on hover);
backtick code, **bold**, and `-` bullet lists are rendered, so use them. Private
`--` helpers usually skip the docstring and use a `;;` comment instead.

```lisp
(defn format-source (src)
  "Format `src` as a Brood source string. Idempotent."
  …)
```

**Every module opens with `(defmodule name "…")`** — the docstring is a short
paragraph on what the module is for and where its Rust substrate lives, if any.

**Error messages** read `"fn-name: what went wrong"`, lowercase, with the
offending value appended via `str`-style trailing args:

```lisp
(error "reload-on-change: no such path: " path)
```

## Patterns (`let`, `fn`, `match`, `receive`)

The trap: a bare symbol *binds*, it doesn't match. To match a known value,
pin it.

```
_                wildcard — matches anything, binds nothing
x                bind x; a repeated x is an equality constraint (non-linear)
42 "s" :k nil    literal match
'sym             match the symbol `sym`
~expr            pin — match the *current value* of `expr`
(p1 p2 ...)      list of exact length
(p1 & rest)      head(s) + tail
[p1 p2 ...]      vector of exact length (the tuple / tagged-data idiom)
```

```lisp
(match shape
  ([:circle r]    (* 3.14 r r))
  ([:rect w h]    (* w h))
  (_              0))
```

## Looping is recursion

```lisp
(defn sum-to (n acc)
  (if (= n 0) acc
    (sum-to (- n 1) (+ acc n))))         ; tail-recursive: O(1) stack
```

Prefer the higher-order combinators:

```lisp
(reduce + 0 xs)
(map sq xs)
(filter even? xs)
(fold (fn (m k) (assoc m k (* k k))) {} (range 10))
```

For longer pipelines, **transducers** fuse intermediate collections (one pass,
no throwaway lists):

```lisp
(transduce (comp (xmap sq) (xfilter even?)) + 0 (range 1000))
(transduce (xtake-while (fn (x) (< x 100))) + 0 (map sq (range 1000)))
```

### Hot inner loops — fuse passes, skip throwaway intermediates

The combinators above read well, but in a function called hundreds of times per
frame their *intermediate allocations* dominate. Two rules for code on a hot
path:

- **`mapcat`-then-reduce builds a list only to walk it once.** `(frequencies
  (mapcat f xs))` materialises the entire `(len-of-each × count)` list of items
  before `frequencies` tallies it — thousands of throwaway cells per frame. Fuse
  the two into one `fold` so nothing intermediate is built:

  ```lisp
  ;; allocates the full neighbour list, then counts it
  (frequencies (mapcat neighbours cells))
  ;; fused: tally straight into the map, no intermediate list
  (fold (fn (counts cell)
          (fold (fn (c n) (assoc c n (inc (get c n 0)))) counts (neighbours cell)))
        {} cells)
  ```

  Same shape for build-a-collection-then-rebuild: fold the source straight into
  the target instead of `filter`-then-`into`. (For longer pipelines, transducers
  do this fusion for you — reach for them before hand-rolling a `fold`.)

- **A comprehension over a tiny fixed set loses to an explicit literal.** `for`
  lowers to a fused `fold` (no per-element intermediate lists), but it still pays
  a closure call per item plus a final `reverse`. When the set is small and known
  — the 8 neighbours of a cell, say — list them directly and compute each shared
  sub-expression once:

  ```lisp
  ;; per-call comprehension machinery for a fixed 3×3 minus the centre
  (for (dx [-1 0 1] dy [-1 0 1] :when (not (and (= dx 0) (= dy 0))))
    [(+ x dx) (+ y dy)])
  ;; explicit: the eight cells, allocation-light, edges computed once
  (let (l (- x 1) r (+ x 1) u (- y 1) d (+ y 1))
    (list [l u] [x u] [r u] [l y] [r y] [l d] [x d] [r d]))
  ```

  A comprehension is the right tool for one-shot data shaping; in an inner loop
  run thousands of times, prefer the explicit construction. Don't guess which
  matters — `(bench "label" expr)` the sub-expressions and optimise the one the
  clock actually points at.

## Concurrency — processes, not shared state

```lisp
(def me (self))
(spawn (send me [:reply 42]))                      ; child runs the expr
(receive ([:reply x] x))                           ; selective receive
```

**Gotcha: `spawn` is a macro — pass the expression, not a thunk.** `(spawn expr)`
already wraps `expr` in `(fn () expr)`. So write `(spawn (work c))`, **not**
`(spawn (fn () (work c)))` — the latter double-wraps: the child just builds a
closure and returns, the body never runs (a silent no-op that looks like "spawn
didn't work"). Same for `(spawn name expr)`.

Each process has its own heap; messages are **deep-copied** on `send`. `(self)`
is the current process's pid. **Closures can be sent** — a `send`-ed function
carries its code and its captured locals (deep-copied with it); only its *free
global* references are late-bound on the receiver. So builtins/prelude names
always resolve, and any `def`/`defn` the receiving image also has resolves
there — but a free global that exists only on the sender raises `unbound
symbol` on the receiver (ship the `defn` first, or refer to names defined on
both sides). Same model as Erlang's `spawn`/fun-passing. `receive` takes
pattern clauses just like `match`, plus an optional `(after ms body...)`
clause for timeouts.

**`spawn` is let-it-crash.** Plain `(spawn expr)` is Erlang's `spawn/1`:
if `expr` throws, the process exits and its monitors fire
`[:down ref pid [:error msg]]`. There is no kernel-level supervisor — a
hand-written one is ~10 lines of Brood (see [`supervision.md`](supervision.md)).
Named-spawn `(spawn :name expr)` is idempotent on the name: if `:name` is
already registered to a live pid, returns that pid; otherwise spawns fresh
and registers the new pid. The name is auto-reaped on death.

```lisp
(spawn (worker))                                   ; fire-and-forget; crashes exit the process
(spawn :ticker (ticker 0))                         ; named + idempotent

;; Userland supervisor — re-spawn on crash:
(defn supervise (worker-fn)
  (let (pid (spawn (worker-fn)) ref (monitor pid))
    (receive
      ([:down ~ref _ :normal] :ok)
      ([:down ~ref _ reason]
        (println "child died: " (pr-str reason) " — restarting")
        (supervise worker-fn)))))
```

## Stateful servers — the `hatch` framework (`(:use hatch)`)

Raw `spawn`/`receive` is the substrate; for a process that **holds state and
answers messages** (a gen_server / actor), use `hatch`. State is immutable —
each clause *returns the next state* to carry through the loop. Two message
kinds:

- **cast** — fire-and-forget; the clause body is the **next state**. Send with
  `(! pid payload)`.
- **call** — synchronous; the clause body is `[reply next-state]` and the caller
  blocks for `reply`. Send with `(gen-call pid payload)`.
- **query** — synchronous read-only; the body is just the reply, state unchanged.
  Use this for "just read a field" cases to avoid the `[x s]` boilerplate.

```lisp
(defmodule my-counter "…" (:use hatch))   ; (:use hatch), not (require 'hatch),
                                           ; to write defprocess/cast/gen-call bare

(defprocess counter (n)                 ; n is the state
  (cast  :inc       (+ n 1))            ; new state = n+1
  (cast  [:add k]   (+ n k))            ; payloads can carry data (pattern binds k)
  (cast  :ping      (do (println "pong") n))  ; side effect, state unchanged
  (call  :value     [n n])              ; reply n, keep state n
  (query :double    (* n 2)))           ; reply n*2; state untouched

(def c (hatch counter 0))               ; spawn with initial state 0 → pid
(! c :inc)                              ; cast (returns immediately)
(! c [:add 10])
(gen-call c :value)                     ; => 11  (synchronous; blocks for reply)
(gen-call c :double)                    ; => 22  (query — read-only)
(stop c)                                ; graceful shutdown; ends the loop
```

Other primitives: `(sleep ms)` parks the current process without touching its
mailbox (it does *not* block a worker thread). `(stop pid)` ends a hatch
process's receive loop cleanly — every hatch automatically handles the stop
envelope, no `:stop` clause needed.

**Worker pool — fan out work, fan in results** (plain `spawn`/`receive`, the
pattern most demos want):

```lisp
(defn worker (parent i)                 ; compute, send result tagged with i
  (send parent [:done i (* i i)]))

(defn collect (got total acc)           ; tail-recursive fan-in over the mailbox
  (if (= got total)
    acc
    (collect (+ got 1) total (receive ([:done i sq] (assoc acc i sq))))))

(defn run (n)
  (let (me (self))
    (dotimes (i n) (spawn (worker me i)))   ; fan out n workers
    (collect 0 n {})))                       ; fan in n results into a map
```

Each worker is a green process on the scheduler's pool; `send` deep-copies the
result across heaps. The `collect` loop is tail-recursive, so it's O(1) stack
even for thousands of workers.

## Interactive apps — the display seam & `ui-run`

An interactive app (terminal *or* native window) is one render-op protocol with
several frontends (ADR-046). You write **one** pure `view` and `update`; the same
code paints to a terminal or a GUI window unchanged.

- **Frame** = a vector of render ops, built with `std/display` constructors:
  `(frame (clear) (text row col s face?) (cursor row col))`. A *face* is a style
  map, `{:fg :red :bold true}` (`(:use display)` for the constructors).
- **Frontend** = a map of five fns `{:enter :leave :size :draw :poll}`. `(:use ui)`
  gives you `*term-display*` (the terminal) and `(gui-display)` (a native window,
  needs a `--features gui` build); `display-broadcast` fans one frame to several.
- **The loop** = `(ui-run model view update display)` — a TEA loop: render
  `(view model cols rows)` → poll input → fold it with `(update model input cols
  rows)` → recurse, until the model is `:done`, then tear the frontend down. Set
  `:tick-ms` in the model for the refresh beat (input is `:tick` on timeout).

```lisp
(defmodule main "a counter app" (:use ui) (:use display))

(defn view (m cols rows)
  (frame (clear) (text 0 0 (str "count: " (get m :count)))))

(defn update (m input cols rows)
  (cond (= input :up)   (assoc m :count (inc (get m :count)))
        (= input :down) (assoc m :count (dec (get m :count)))
        (= input :ctrl-c) (assoc m :done true)   ; Esc/Ctrl-C → quit
        else m))

(defn main () (ui-run {:count 0 :done false :tick-ms 1000} view update *term-display*))
;; for a window instead: (ui-run … (gui-display))   ; --features gui
```

**Input vocabulary** (what `:poll` / a raw `(receive)` delivers): a printable key
is a **1-char string** (`"a"`); the rest are keywords — `:up :down :left :right
:enter :backspace :escape :ctrl-c` …; the mouse is `[:mouse action button row col]`
(`action` is `:press` / `:release` / `:drag` / `:scroll-up` / `:scroll-down` —
`:drag` is motion with a button held, delivered once per cell crossed, so a divider
drag is bounded; `button` is `:left` / `:right` / `:middle`, nil for scroll);
a resize is `[:resize cols rows]`. **A GUI window's close button (the X) is its own
`:close` message** — *not* `:escape`, so an app that binds Esc to cancel/normal-mode
can still be closed by the X. **`ui-run` quits on `:close` automatically**, so every
`ui-run` app is closeable for free; you never wire it into `update`. In a hand-rolled
`(receive)` loop (not on `ui-run`), match it yourself — `(:close :quit)` — or use the
`ui/quit-request?` predicate. (`nest new --template gui` / `--template editor`
scaffold complete `ui-run` apps; `--template tui-loop` a plain stdout animation.)

## Hot reload (`nest run --watch FILE`)

Writing a live script: just write a normal Brood file. The
`nest run --watch` wrapper re-loads on save.

```lisp
;; live.blsp — run with: nest run --watch live.blsp
(defn my-loop (n)
  (do (println "iter:" n) (sleep 1000) (my-loop (+ n 1))))

(my-loop 0)
```

What happens when you save:

- `(defn my-loop …)` re-evaluates — the global rebinds.
- `(my-loop 0)` is **not** re-run — `reload-defs` skips non-`def*` top-level
  forms, so each save doesn't fork a duplicate loop.
- The running process's next call to `my-loop` late-binds to the new
  closure, picks up your edit on the next iteration (ADR-013).

If your save introduces a runtime error (typo, unbound symbol, wrong
arity), the process **dies** — there is no kernel supervisor (ADR-039
reverted, 2026-05-29). `--watch` re-spawns from scratch when you save
again; state in the watched process is not preserved across a crash. For
state-preserving recovery, write a userland supervisor (`spawn` +
`monitor`; pattern in [`supervision.md`](supervision.md)) — but be aware
that re-spawning means losing the closure's local state and restarting
the function call from its initial args.

Outside `--watch` (`nest run FILE`, `brood FILE`), the same file runs
inline as a plain script — no reload, throws exit.

### Running a loop for a bounded time (`nest run --for DURATION`)

An infinite loop or full-screen TUI never returns, which makes it awkward
to exercise (you can't `eval` it). `nest run --for DURATION` runs the
program for at most that long, then exits **cleanly** — the first-class
form of `timeout Ns nest run`:

```
nest run --for 2s        # run :main for 2 seconds, then stop
nest run --for 500ms src/game.blsp
```

`DURATION` is `2s`, `500ms`, or a bare integer (milliseconds). Use it to
verify a whole loop end-to-end (not just its pure functions) and to make
time-based behaviour reproducible in CI. It prints `[stopped after …]` on
the cap and exits 0; the program is dropped where it stood (a TUI may not
get to restore the terminal — redirect output in CI).

To run a one-off entry point without editing the manifest's `:main`, pass
`nest run --main module/fn` (or just `--main module`, defaulting the fn to
`main`) — handy while iterating on a module that isn't the project default.

## Errors

```lisp
(try
  (work)
  (catch e
    (println "failed: " e)))

(throw [:my-error :reason])              ; throwable values are arbitrary
(error "x out of range: " x)             ; convenience: throw with a built string
```

## Common builtins

This is a curated tour, not the full list. For the **complete reference** —
every builtin and prelude fn/macro with its signature and one-line summary —
run `nest doc --all` and read it once, rather than probing names one at a time
in the REPL. (`nest doc <module>` does the same for an opt-in module like
`display`/`buffer`/`ansi`; `apropos`/`doc-search` search it interactively.)

- **list / seq**: `first` `rest` `cons` `list` `count` `empty?` `nth`
  `reverse` `map` `filter` `reduce` `fold` `append`/`concat` (variadic, over
  lists *and* vectors, returning a list) `mapcat` `sort` `take`
  `drop` `range` `zip` `partition` `frequencies` `enumerate` `repeat`
  `repeatedly`
- **iteration** (macros, for effect — there is no `while`/`for`-loop): `for`
  (list comprehension, with `:when`), `doseq` (destructuring/`:when`),
  `dotimes` `(i n)`, `dolist` `(x coll)`. All return `nil` except `for`.
- **string**: `str` `pr-str` `string-length` `substring` `char-at`
  (returns a 1-char *string* — Brood has no char type) `index-of`
  `string-contains?` `string-split` `join` `replace` `trim` `triml` `trimr`
  `blank?` `upper` `lower` `number->string` `string->number`
  `string->list` `list->string` `starts-with?` `ends-with?`
- **string formatting**: `string-repeat` `pad-left` `pad-right`
  `to-fixed` (number → string with fixed decimals, e.g. `(to-fixed 3.14159 2)`
  → `"3.14"` — `str` prints full f64 precision, so reach for this for output) ·
  `format` (small printf, e.g. `(format "x=%d y=%.2f" 42 3.14)` → `"x=42 y=3.14"`;
  specifiers `%s %d %f %.Nf %%`; width via `pad-left`/`pad-right`)
- **map**: `assoc` `dissoc` `get` `keys` `vals` `contains?` `into` `entries`
  (alias of `map-pairs`) `seq` (universal list-view — coerces a map to its
  `[k v]` pairs; lists, vectors, strings, nil pass through). **Maps are seqable**:
  `(map f m)` / `(filter f m)` / `(fold f acc m)` / `(reduce f acc m)` /
  `(count m)` / `(into [] m)` all walk the map as its `[k v]` pairs — no need
  for `(zip (keys m) (vals m))`. Iteration order is hash-driven (ADR-040), so
  compare via `frequencies` when order would otherwise matter.
- **set** (`(:use set)` in your `defmodule` header — `(require 'set)` alone
  leaves the names qualified, `set/union`): a set is a **map of `element →
  true`**, so
  membership is `(contains? s x)`, elements `(keys s)`, size `(count s)`, and it's
  seqable like any map. The module adds `(set coll)` (dedups), `conj`/`disj`,
  `union`/`intersection`/`difference`/`subset?`. `(set [[0 0] [1 2]])` is the
  natural live-cell model (structural vector keys). No `#{…}` literal or `set?`
  yet (deferred) — test a set with `map?`.
- **types**: `type-of` plus the `?` predicates — `int?` `float?` `string?`
  `symbol?` `keyword?` `bool?` `nil?` `pair?` `vector?` `map?` `fn?` `ref?`
  `pid?`
- **arithmetic**: variadic `+ - * /`; comparison variadic chains
  `< > <= >= =`; `inc` `dec` `abs` `min` `max`; integer division `quot`
  (truncating) / `rem` (truncated remainder) / `mod` (Euclidean);
  `floor` `ceil` `round` `round-to` (round to N decimals, stays a number)
  `pow` `sqrt`. Integer `+ - *` **error on overflow** (they don't wrap).
- **bitwise**: `bit-and` `bit-or` `bit-xor` `bit-not` `bit-shift-left`
  `bit-shift-right` (64-bit, arithmetic right shift; shift amount in `[0,64)`).
- **randomness** (pure & seedable — there is *no* global RNG; thread the seed):
  every step takes a seed and returns `[value next-seed]`. `rng` (→ a 32-bit
  int), `rand-int` `(seed n)` → `[i next]` in `[0,n)`, `rand-float` `(seed)` →
  `[f next]` in `[0,1)`, `shuffle` `(seed coll)`, `sample` `(seed coll)`; seed a
  stream from any int (e.g. `(now)`) with `rand-seed`. Carry `next-seed` in your
  loop/process state like any other value.
- **meta / eval**: `apply` (call a fn with a list of args — the only way to
  splat) `eval` `read-string` `eval-string` `gensym` (fresh symbol, for macros)
- **discovery / introspection**: `doc` `arglist` `bound?` `source-location`;
  and to *find* what exists rather than guess names — `all-globals`,
  `apropos` (name substring, e.g. `(apropos "rand")`), `doc-search` (matches
  docstrings). The same three are `nest mcp` tools. Reach for these instead of
  probing names one at a time.
- **timing**: `now` (ms since epoch) `now-ns` (ns since epoch) `bench`
  (macro: `(bench "label" expr)` prints `label: N ms`, returns `expr`)
- **I/O**: `print` `println` `slurp` `spit` `load` `eval-string` `read-string`.
  `print`/`println` **flush stdout every call** — there's no separate flush, so
  an animation frame paints immediately. For raw terminal control without the
  full display protocol, `(:use ansi)` in your `defmodule` header (a bare
  `(require 'ansi)` leaves them qualified, `ansi/ansi-clear`) gives
  `(ansi-clear)`/`(ansi-home)`/
  `(ansi-cursor r c)`/`(ansi-hide-cursor)` — **zero-arg functions you call**, each
  *returning* an escape string. Call them: `(print (ansi-clear))`, **never**
  `(print ansi-clear)` (a bare symbol prints `#<fn …>` and emits no escape). The
  ESC byte is the `\e` string escape. (For a render-op frame buffer, use
  `std/display`.)
- **Filesystem (stat-class)**: `file-exists?` `dir?` `list-dir` `file-mtime`
- **processes**: `spawn` (incl. named-spawn `(spawn :name expr)`)
  `send` `receive` `self` `ref` `monitor` `demonitor` `register` `whereis`
  — plus the **`hatch`** framework below
- **transducers**: `comp` `xmap` `xfilter` `xremove` `xkeep` `xmapcat`
  `xtake-while` `transduce` `reduced` `reduced?`

## Pitfalls when generating Brood code

- **No `setq` / `set!` / atoms.** State = a process, or re-bind a global with
  `def`.
- **No `while` / `for`.** Use recursion (TCO is guaranteed) or
  `fold` / `map` / `filter` / `reduce` / `transduce`.
- **Calls are `(f x)`, never `f(x)`.** Brood has no C-style call syntax: `f(x)`
  reads as *two* forms — `f`, then `(x)` — so the `(x)` tries to *call the value
  of* `x` and you get `cannot call non-function`. Write `(println "hi")`, not
  `println("hi")`. (The evaluator now hints this when the mis-called head is a
  literal.)
- **Bare symbols in patterns *bind*.** Match a literal symbol with `'foo`;
  match a runtime value with `~expr`.
- **`=` is structural** and recursive — two unrelated structures that look the
  same compare equal.
- **Variadic operators**: `(+ a b c)` works. The fast 2-arg primitives, when
  you really need them, are `%add` `%sub` `%mul` `%div` `%lt` `%eq`.
- **No commas in maps**: `{:a 1 :b 2}` — spaces only.
- **`let` bindings are flat**: `(let (a 1 b 2) ...)`, not Scheme's `(let ((a 1) (b 2)) ...)`. Same for `let*` / `letrec` / `binding`.
- **`nil` is distinct from `false`** — `(nil? false)` is `false`,
  `(false? nil)` is `false`. Both are falsy, neither is the other.
- **Tail position matters**: deep *non*-tail recursion overflows the
  green-process stack. Use a tail-recursive helper with an accumulator — make
  the self-call the *last* thing the function does. The advisory checker
  (`nest check`, the LSP, `nest mcp`) **warns** when a function calls itself in
  non-tail position, e.g. `(* n (fact (- n 1)))`.
- **Each module is a namespace (ADR-065).** A file's `(defmodule name …)`
  compiles the rest of the file into namespace `name`: every `def`/`defn`/
  `defmacro` defines `name/foo`, and a bare reference resolves first in the
  current namespace, then through the header's `(:use …)` imports, then root
  (the prelude). So the *same* short name in two modules is fine — `a/parse`
  and `b/parse` are distinct globals, and `nest run`/`nest test` no longer
  false-flag them. From *outside* a module (e.g. the REPL or `nest mcp` eval),
  reach a `defn` by its qualified name: `(life/step …)`, found via `apropos`.
  **The one exception is earmuffed `*foo*` names** — by convention they're
  *ambient* (dynamic/config vars) and stay bare/root, never namespaced, so a
  `(def *width* …)` is reachable as `*width*` everywhere and must be unique.
- **Importing a module**: inside `defmodule`, add a `(:use mod)` clause to refer
  `mod`'s public names **bare** (`(:use mod :refer [a b])` for a subset). A
  plain top-level `(require 'mod)` only *loads* `mod` — its names stay
  qualified (`mod/foo`). `(:require …)` is **not** a `defmodule` clause; only
  `:use` is (anything else in the header is silently ignored).
- **Not Clojure**: no `defprotocol`, no transients, no `loop` / `recur`
  (just plain recursion). Namespaces *do* exist now (ADR-065) but are
  `mod/name`-flat, not Clojure's `require :as` aliasing.
- **Not Scheme / CL**: no `setq`, no `cond`-with-`t`-catch-all (use `else`
  or `:else`).
- **`sort` on heterogeneous / non-numeric items uses *structural* order.**
  `(sort coll)` is `<` for numbers, lexicographic for vectors/lists, text order
  for strings/symbols/keywords (so `(sort [[1 0] [2 1]])` works, no comparator
  needed). For custom orderings use `(sort less? coll)` or `(sort-by key-fn coll)`.
- **`index-of` works on strings *and* on lists/vectors.** Strings → substring
  search; lists/vectors → linear element search (structural `=`). Returns `-1`
  if absent. The general "is `x` in `coll`?" predicate is `(includes? coll x)`
  — handles lists, vectors, strings (substring), and maps (looks at values; use
  `contains?` for keys).

## Module skeleton (what `nest new` scaffolds)

`nest new <name>` scaffolds this default `main`+`hello` pair. Other `--template`
options scaffold starter shapes you'd otherwise hand-write: `tui-loop` (a
tail-recursive animation loop, pairs with `nest run --for`), `hatch` (a stateful
gen_server-style process), `http-server` (a minimal web app), `editor` (a tiny
text editor on `ui-run`), and `gui` (a windowed `ui-run` app — see *Interactive
apps* above; needs a `--features gui` build).

```lisp
;; src/hello.blsp
(defmodule hello "A second module — main requires it and calls greeting.")

(defn greeting () "hello world")
```

```lisp
;; src/main.blsp
;; `(:use hello)` brings `hello`'s public names (here `greeting`) into scope
;; bare; without it you'd call `(hello/greeting)`. A plain `(require 'hello)`
;; only loads the file — it does not refer the names.
(defmodule main "The project's entry-point module (nest run -> main/main)."
  (:use hello))

(defn main ()
  "Entry point: print the project's greeting."
  (println (greeting)))
```

```lisp
;; tests/hello_test.blsp
;; `(:use hello)` brings `greeting` into scope; `(:use test)` the test macros.
(defmodule hello-test (:use hello) (:use test))

(describe "hello"
  (test "greeting works"   (assert= (greeting) "hello world"))
  (test "greeting is text" (is (string? (greeting)))))
```

`describe` groups tests; `test` defines one. `(assert= actual expected)` checks
structural equality with a diff on failure; `(is expr)` asserts truthy.
`(assert-error body…)` asserts a raise. For stochastic code, **property testing**:
`(check-property n gen pred)` draws `n` values from a `seed -> [value next-seed]`
generator (over the PRNG) and asserts `pred` on each — deterministic, and on a
counterexample it fails with the value + seed:

```clojure
(check-property 100 (fn (s) (rand-int s 1000)) (fn (x) (and (>= x 0) (< x 1000))))
```

`nest test` runs each test in its own green process. `nest run` invokes the
`main/main` entry by default (override in `project.blsp` via `:main`; e.g.
`:main app` runs `app/main`, `:main (app start)` runs `app/start`). The manifest
is **unevaluated data**, so write these as bare symbols — *not* `:main 'app`
(a leading quote misparses). Add
`--for DURATION` (e.g. `nest run --for 2s`) to run a loop/TUI for a bounded
time and exit cleanly.

## When in doubt

`std/prelude.blsp` is the canonical example of idiomatic Brood — almost
everything below the kernel is written there in the language itself; read it.
Deep references: `docs/language.md` (full reference), `docs/spec.md` (the
formal spec), `docs/pattern-matching.md` (the pattern grammar in detail).
