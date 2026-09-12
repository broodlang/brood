# The bare namespace — every unqualified public name, and why it is one

**This file is a gate, not an inventory.** `tests/bare_names_test.blsp` compares it against
the runtime's live surface and fails when a bare name exists that is not listed here. Adding
one is then a deliberate act with a recorded reason, which is the whole point.

## Why it exists

Every public name is either qualified (`string/join`, `gen/call`) or **bare** — reachable
with no module prefix, from every program, forever. Bare names are the language's own
vocabulary, and the budget for them is small and shared: each one is a word users can no
longer use for their own top-level definition without shadowing something.

ROADMAP recorded the problem as a **flow, not a stock**:

> It went 268 → **264** across a day in which ADR-290/291 removed 18, because roughly
> fourteen arrived: of ten sampled, eight … did not exist that morning. Reduction work is
> cancelled by ordinary feature work at about the rate it is done, so the next audit will
> re-derive this same list unless **adding a bare name has to record a reason**.

Two audits had each removed names and each been undone by ordinary feature work, because
nothing sat between a new `defn` at the root and the namespace. That is what this is.

## What the gate does, and does not

- **Hard**: a bare public name absent from this file fails the test, naming it. To add one,
  add it here under the group it belongs to — and the reason to write in the commit is *why
  it should not be qualified*.
- **Soft**: a name listed here that is no longer live is reported only when the whole
  surface is loaded, because a lean build (no `brood/dev-tools`) genuinely has fewer
  modules, and a gate that reds on the feature set is a gate people learn to ignore.

The groups below are coarse on purpose: they carry the *kind* of justification, not prose
per name. `*earmuffed*` dynamics and operators are bare by convention and need no argument;
the `core` list is the one worth pushing back on, and the one to read before adding to it.

Counts are as of 2026-09-11: **265** bare public names.

## operator (13)

Operators — arithmetic, comparison and the reader's own punctuation. Bare by convention; a qualified `math/+` would be unreadable.
- `*`
- `*1`
- `*2`
- `*3`
- `+`
- `-`
- `->`
- `/`
- `<`
- `<=`
- `=`
- `>`
- `>=`

## dynamic (62)

Dynamic variables (`defdyn`) — earmuffed, so they cannot be mistaken for ordinary bindings, and bare because `binding` sites read better unqualified. The largest group and the least contentious; note how many are one subsystem's configuration (`*project-*`, `*test-*`, `*repl-*`).
- `*autoloading*`
- `*comment-kinds*`
- `*config-git-init*`
- `*config-registry*`
- `*config-registry-token*`
- `*debug-session*`
- `*err*`
- `*error-explanations*`
- `*faces*`
- `*features*`
- `*format-headers-extra*`
- `*format-pair-body*`
- `*http-max-head-bytes*`
- `*http-max-response-bytes*`
- `*lineedit-keymap*`
- `*load-path*`
- `*ns-package*`
- `*observe-keymap*`
- `*observe-timeout*`
- `*out*`
- `*parallel-batch*`
- `*print-length*`
- `*print-level*`
- `*print-string-length*`
- `*project-brood*`
- `*project-bundled-packages*`
- `*project-dependencies*`
- `*project-description*`
- `*project-dev-dependencies*`
- `*project-enhances*`
- `*project-format-paths*`
- `*project-kind*`
- `*project-main*`
- `*project-main-override*`
- `*project-name*`
- `*project-repository*`
- `*project-root*`
- `*project-source-paths*`
- `*project-templates*`
- `*project-test-paths*`
- `*project-version*`
- `*project-width*`
- `*protocols*`
- `*reload-diagnostics*`
- `*repl-cont-prompt*`
- `*repl-interruptible*`
- `*repl-prompt*`
- `*require-parent*`
- `*resolver-max-steps*`
- `*resolver-time-budget-ms*`
- `*show*`
- `*spy-sink*`
- `*term-display*`
- `*test-filter*`
- `*test-last-failed*`
- `*test-last-ran*`
- `*test-max-failures*`
- `*test-report-sink*`
- `*test-slow-timeout-ms*`
- `*test-timeout-ms*`
- `*test-trace*`
- `*test-wait-ms*`
- `*units*`

## predicate (40)

Predicates — `x?` type and shape tests. Bare because they read as English at a call site and are used everywhere.
- `any?`
- `bool?`
- `bound?`
- `bytes?`
- `contains?`
- `date?`
- `datetime?`
- `decimal?`
- `empty?`
- `every?`
- `failure?`
- `float?`
- `fn?`
- `includes?`
- `int?`
- `keyword?`
- `list?`
- `map?`
- `multimap?`
- `nil?`
- `number?`
- `pair?`
- `pid?`
- `pq?`
- `queue?`
- `range?`
- `ratio?`
- `record?`
- `ref?`
- `reserved-package-name?`
- `rope?`
- `satisfies?`
- `seqview?`
- `set?`
- `string?`
- `symbol?`
- `table?`
- `time-of-day?`
- `type-matches?`
- `vector?`

## core (150)

Core vocabulary — the language's own words: special-form companions, sequence and map operations, the process primitives, and the test/dev macros. **This is the group with a real budget.** Before adding here, ask whether the name belongs to a module instead: `string/`, `seq/`, `proc/`, `test/` all exist precisely so a name does not have to be bare.
- `*e`
- `->float`
- `->seq`
- `->string`
- `and`
- `append`
- `apply`
- `apropos`
- `arglist`
- `as->`
- `assoc`
- `assoc-in`
- `binding`
- `but-last`
- `bytes`
- `case`
- `check-allow`
- `comment`
- `comp`
- `compare`
- `compare-to`
- `complement`
- `cond`
- `cond->`
- `conj`
- `conj-onto`
- `cons`
- `constantly`
- `count`
- `dec`
- `def-`
- `defability`
- `defbehaviour`
- `defdyn`
- `defmacro`
- `defmethod`
- `defmodule`
- `defmulti`
- `defn`
- `defn-`
- `defonce`
- `defrecord`
- `deftype`
- `demonitor`
- `disj`
- `dissoc`
- `dissoc-in`
- `doc`
- `doc-search`
- `dolist`
- `doseq`
- `dotimes`
- `doto`
- `drop`
- `drop-while`
- `each`
- `error`
- `error-message`
- `exit`
- `failure`
- `fields`
- `filter`
- `first`
- `fold`
- `for`
- `gensym`
- `get`
- `get-in`
- `hash-map`
- `identity`
- `if-let`
- `impl`
- `inc`
- `index-of`
- `inspect`
- `into`
- `keys`
- `keyword`
- `last`
- `link`
- `list`
- `lookup-get`
- `lookup-keys`
- `macroexpand`
- `macroexpand-1`
- `map`
- `mapcat`
- `mapv`
- `match`
- `match*`
- `merge`
- `meta`
- `monitor`
- `multi-return-type`
- `not`
- `not=`
- `nth`
- `offload`
- `ok->`
- `or`
- `partial`
- `partition`
- `pr-str`
- `pr-str-bounded`
- `provide`
- `range`
- `read-line`
- `receive`
- `record-id`
- `reduce`
- `ref`
- `repeat`
- `require-one`
- `rest`
- `reverse`
- `second`
- `self`
- `send`
- `seq`
- `sig`
- `sig!`
- `sleep`
- `sort`
- `sort-by`
- `spawn`
- `spawn-link`
- `spawn-monitor`
- `spy`
- `str`
- `symbol`
- `take`
- `take-while`
- `tap`
- `then`
- `third`
- `throw`
- `try`
- `type-of`
- `unless`
- `unlink`
- `update`
- `update-in`
- `vals`
- `vec`
- `vector`
- `when`
- `when-let`
- `with`
- `with-err-str`
- `with-out-str`
