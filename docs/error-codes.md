# Error codes

Stable identifiers attached to built-in Brood errors at construction time.
Once shipped, **codes never get repurposed** — new errors get the next free
slot in their range. Agents and Brood code branch on `:code` (or `:kind`) for
programmatic handling; humans read `:message` and `:hint`.

The contract is described in [`llm-native.md`](llm-native.md) §4; the
machinery in [`error.rs`](../crates/lisp/src/error.rs) (`LispError`,
`error_codes`, `to_value_map`); the catch surface in [`prelude.blsp`](../std/prelude.blsp)
(`try`/`catch`); and the MCP projection in [`mcp.md`](mcp.md) (the agent sees
the same fields in `error.data` of a JSON-RPC failure and in `:error` of a
tool's `:value`/`:error` result).

## Catch shape

When `catch` rebinds a kernel error (`(try expr (catch e …))`), `e` is a map:

```lisp
{:kind <keyword>           ; :parse | :unbound | :arity | :type | :runtime | :user
 :message <string>         ; the rendered text
 :code <string>            ; stable, "E00xx"; absent for user-thrown values
 :file <string>            ; when known (set by load / file runner)
 :line <int> :col <int>    ; when known (1-based)
 :hint <string>}           ; optional, points at a likely fix
```

A **user throw** (`(throw v)`) keeps the contract `(catch e e) → v` — the
caught value is whatever was thrown. Only kernel-raised errors get the
wrapper map.

## Numbering scheme

Codes are grouped by [`ErrorKind`]:

| Range | Kind | Notes |
|---|---|---|
| `E00xx` | parse / reader | `E0001` is the generic parse fail |
| `E01xx` | unbound / scope | symbol lookup failures |
| `E02xx` | arity | wrong number of args |
| `E03xx` | type | wrong-kind operand |
| `E04xx` | runtime | division, overflow, IO, … |

[`ErrorKind`]: ../crates/lisp/src/error.rs

## Current codes

| Code | Kind | Raised by | Example trigger |
|---|---|---|---|
| `E0001` | `:parse` | reader, `LispError::parse(...)` | `(unclosed` |
| `E0010` | `:unbound` | eval lookup, `LispError::unbound(...)` | `(no-such-fn)` |
| `E0020` | `:arity` | `bind_params`, `LispError::arity(...)` | `((fn (x) x))` |
| `E0030` | `:type` | `LispError::wrong_type(...)` / `type_err(...)` | `(first 5)` |
| `E0040` | `:runtime` | `%div` / `rem` (with a `:hint`) | `(/ 1 0)`, `(rem 1 0)` |
| `E0041` | `:runtime` | checked arithmetic overflow; `floor` of non-finite or out-of-i64 float | `(* 9223372036854775807 2)`, `(floor 1e20)` |
| `E0042` | `:runtime` | index out of range (`vector-ref`, `substring`) | `(vector-ref [1 2 3] 7)`, `(substring "hi" 0 99)` |
| `E0043` | `:runtime` | allocation crossed the soft memory limit; the eval safepoint raises (catchable) instead of OOMing the host. Off by default; set via `BROOD_MEM_LIMIT` | a runaway `(cons …)`/`(string-repeat …)` loop under a limit |
| `E0044` | `:runtime` | evaluation used more stack than the byte budget (runaway *non-tail* recursion); raised before the coroutine stack overflows into an uncatchable SIGSEGV. Tune via `BROOD_STACK_BUDGET` | `(defn boom (n) (+ 1 (boom (+ n 1)))) (boom 0)` |
| `E0050` | `:runtime` | file IO (`load`, `slurp`, `spit`, `make-dir`, `list-dir`, `cwd`, `check-file`, `check-file-structured`) | `(slurp "/no/such/file")` |
| `E0051` | `:runtime` | `run-process` couldn't start the subprocess (with a `:hint` about PATH) | `(run-process "nope" [])` |
| `E0060` | `:runtime` | distribution layer: `node-start` / `connect` failed | `(connect "bad@host")` |
| `E0070` | `:runtime` | `send` saw a message value nested past `MAX_MESSAGE_DEPTH` (with a `:hint` about chunking) | a recursively self-referential structure |
| `E0099` | `:runtime` | `LispError::runtime(...)` (catch-all) | uncoded runtime raises |

`E0099` is the catch-all assigned by `LispError::runtime(...)` — every
runtime raise picks it up unless the site overrides via
`.with_code(error_codes::SOMETHING)`. As specific raises get their own
codes, they slot into the `E04xx` / `E05xx` / `E06xx` / `E07xx` ranges
(integer-shaped failures in `E004x`, IO/process in `E005x`, distribution
in `E006x`, messaging in `E007x`) and the message becomes more diagnostic.
Reserve any new code only when you also intend to attach a `:hint` — the
diagnostic value of a code without one is small.

## Adding a new code

1. Pick the next free slot in the right range.
2. Add a `pub const` to [`error_codes`](../crates/lisp/src/error.rs)
   alongside the existing ones — the name uppercases the kind.
3. Tag the raise site with `.with_code(error_codes::YOUR_NEW)`.
4. Add a row to the table above. Don't reuse a retired code.
5. If the error has a common fix, attach a `:hint` at construction via
   `.with_hint("…")`.

## Hints

`:hint` is set by the raise site when there's a common fix worth surfacing.
A hint always names an actionable next step, not just a description:

| Hint context | Message |
|---|---|
| `(/ x 0)` / `(rem x 0)` | `guard the denominator: (when (not= y 0) (/ x y))` |
| Unbound symbol in a green process | `this fired inside a spawned process — if it happens only under fan-out load, the scheduler may be racing prelude lookups; try -j 1 …` |
| `run-process` failure | `check that the program is on PATH and the args are well-formed` |
| Message too deep (`E0070`) | `messages cross processes by deep copy — flatten or chunk the data (e.g. send a list of items rather than one nested tree)` |

The scheduler-race hint is conditional: it attaches only when the unbound
error is raised inside a green (spawned) process — checked via
`process::in_green_process()` in `eval::unbound_error`. The root thread
(REPL / file runner / `nest mcp` dispatcher) doesn't get it, because the
race is a multi-thread scheduling phenomenon. Documented from
[`claude-demo-findings.md`](claude-demo-findings.md)'s blocker §1.

## Branching on `:code` vs `:kind`

`:kind` is for **categorical** matching ("any runtime error retries; any
type error is a real bug"). `:code` is for **specific** matching ("the
scheduler race specifically — try `-j 1`"). Prefer the broader one unless
you need the precision.

```lisp
;; Categorical: handle any unbound symbol uniformly.
(try
  (do-work)
  (catch e
    (case (get e :kind)
      :unbound (do-handle-missing-dep e)
      :type    (raise e)            ; real bug, surface it
      :else    (log-and-retry e))))

;; Specific: only this exact failure can be retried, not all runtimes.
(try
  (do-fan-out-work)
  (catch e
    (if (= (get e :code) "E0099")
      (retry-with-single-thread)
      (raise e))))
```
