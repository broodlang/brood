# Error codes

Stable identifiers attached to built-in Brood errors at construction time.
Once shipped, **codes never get repurposed** — new errors get the next free
slot in their range. Agents and Brood code branch on `:code` (or `:kind`) for
programmatic handling; humans read `:message` and `:hint`.

The contract is described in [`archive/llm-native.md`](archive/llm-native.md) §4; the
machinery in [`error.rs`](../crates/lisp/src/error.rs) (`LispError`,
`error_codes`, `to_value_map`); the catch surface in [`prelude/control.blsp`](../std/prelude/control.blsp)
(`try`/`catch`); and the MCP projection in [`mcp.md`](mcp.md) (the agent sees
the same fields in `error.data` of a JSON-RPC failure and in `:error` of a
tool's `:value`/`:error` result).

## Catch shape

Every error the language raises is a map (ADR-501), so `(try expr (catch e …))` binds
this, and `catch` takes a pattern (`(catch {:kind :type} …)`):

```lisp fragment
{:kind <keyword>           ; :parse | :unbound | :arity | :type | :runtime | :interrupted
                           ;   | :contract | :error | :assert | :match
 :message <string>         ; the rendered text
 :code <string>            ; stable, "E00xx"; absent for user-thrown values
 :file <string>            ; when known (set by load / file runner)
 :line <int> :col <int>    ; when known (1-based)
 :hint <string>            ; optional, points at a likely fix
 :trace <list>}            ; the call stack at the raise, innermost first
```

A **user throw** (`(throw v)`) keeps the contract `(catch e e) → v` — the
caught value is whatever was thrown. Only errors the language raises get the
map.

## Numbering scheme

Codes are grouped by decade, broadly along [`ErrorKind`]:

| Range | Kind | Notes |
|---|---|---|
| `E000x` | parse / reader | `E0001` is the generic parse fail |
| `E001x` | unbound / scope | symbol lookup failures |
| `E002x` | arity | wrong number of args |
| `E003x` | type | wrong-kind operand |
| `E004x` | runtime | numeric faults, limits, interrupts |
| `E005x` | runtime | IO and subprocesses |
| `E006x` | runtime | distribution |
| `E007x` | runtime | messaging |
| `E008x` | contract | a refused `sig` |
| `E009x` | error map | what `error`, `assert` and a failed match raise; `E0099` is the catch-all |

[`ErrorKind`]: ../crates/lisp/src/error.rs

## Current codes

| Code | Kind | Raised by | Example trigger |
|---|---|---|---|
| `E0001` | `:parse` | reader, `LispError::parse(...)` | `)` (unexpected close), `(. .)` |
| `E0002` | `:parse` | reader, `err_incomplete` / `err_at_incomplete` — input ended mid-form or mid-string (vs a genuine syntax error). A REPL/editor matches this to mean "read another line" (ADR-049). | `(+ 1`, `[1 2`, `"unterminated` |
| `E0010` | `:unbound` | eval lookup, `LispError::unbound(...)` | `(no-such-fn)` |
| `E0020` | `:arity` | `bind_params`, `LispError::arity(...)` | `((fn (x) x))` |
| `E0030` | `:type` | `LispError::wrong_type(...)` / `type_err(...)` | `(first 5)` |
| `E0040` | `:runtime` | `%div` / `rem` (with a `:hint`) | `(/ 1 0)`, `(math/rem 1 0)` |
| `E0041` | `:runtime` | `math/floor` of a non-finite float (integers never overflow — they promote to a bignum, so `(* 9223372036854775807 2)` and `(math/floor 1e20)` both succeed) | `(math/floor (* 1e308 10.0))` |
| `E0042` | `:runtime` | index out of range (`string/substring`, `string/char-at`, bytes and rope indexing) | `(string/substring "hi" 0 99)`, `(string/char-at "hi" 9)` |
| `E0043` | `:runtime` | allocation crossed the soft memory limit; the eval safepoint raises (catchable) instead of OOMing the host. Off by default outside a memory-capped container, whose cap sets it; set via `BROOD_MEM_LIMIT` | a runaway `(cons …)`/`(string/repeat …)` loop under a limit |
| `E0044` | `:runtime` | runaway *non-tail* recursion: the VM's call-frame cap (`MAX_BC_FRAMES`, 1048576 frames — `recursion too deep: exceeded the VM's 1048576-frame non-tail-call limit`), the tree-walker's frame cap (262144 frames — `exceeded the tree-walker's 262144-frame non-tail-call limit`), or, for recursion that re-enters the evaluator through native code (`eval`, a callback), the native-stack byte budget (tune via `BROOD_STACK_BUDGET`). Clean and catchable either way — never a SIGSEGV | `(defn boom (n) (+ 1 (boom (+ n 1)))) (boom 0)` |
| `E0045` | `:runtime` | this process's live heap stayed over its own `(proc/flag :max-heap n)` limit after a collection; raised in that process only (uncaught, it retires just the offender) | `(spawn (do (proc/flag :max-heap 1000000) (grow-forever)))` |
| `E0046` | `:runtime` | this process's mailbox grew past its `(proc/flag :max-mailbox n)` bound (ADR-307); raised in the flooded process only, senders never blocked, nothing dropped | flood a process that set `(proc/flag :max-mailbox 8)` |
| `E0047` | `:interrupted` | the running process was interrupted at a safepoint (ADR-491): its quit key arrived while it was busy (`proc/set-quit-key`), another process called `proc/interrupt`, or its interrupt window's ceiling passed (`proc/interruptible ms thunk`). Raised only inside an open window, and sticky until it closes: a catch inside the window is interrupted again at its next safepoint | `(proc/interruptible 0 (fn () (spin)))` → `[:interrupted {:kind :interrupted …}]` |
| `E0048` | `:runtime` | a process marked `(proc/flag :responsive :refuse)` called a native that waits on the outside world (`os/cmd`, `os/run-process`, `os/clipboard-get`, `os/clipboard-image`), refused before the wait (ADR-517). The message names the call and its command | `(proc/flag :responsive :refuse) (os/cmd "true" [])` → `{:code "E0048" …}` |
| `E0050` | `:runtime` | file IO (`reflect/load`, `file/slurp`, `file/spit`, `file/mkdir`, `file/ls`, `file/cwd`, `reflect/check-file`, `reflect/check-file-structured`) | `(file/slurp "/no/such/file")` |
| `E0051` | `:runtime` | `os/run-process` couldn't start the subprocess (with a `:hint` about PATH) | `(os/run-process "nope" [])` |
| `E0060` | `:runtime` | distribution layer: `node/start` / `node/connect` failed, or a `send` to an unreachable node under `(proc/flag :send-errors true)` | `(node/start "a" "no.such.host:1")` |
| `E0070` | `:runtime` | `send` saw a message value nested past `MAX_MESSAGE_DEPTH` — a million levels since ADR-394, a sanity bound rather than a stack limit (with a `:hint` about chunking) | a recursively self-referential structure |
| `E0080` | `:contract` | a runtime contract refused an argument or a result (`BROOD_CONTRACTS`, `sig!`; raised by `%contract-raise`). The caught value is the contract map — `:blame :function :argument :expected :got :message` — with `:code`, `:file`/`:line`/`:col` and `:trace` added, as a built-in error has them. A map a program `throw`s itself is not touched, even one of kind `:contract` | `(sig! f (int -> int))` then `(f "x")` |
| `E0090` | `:error` | `(error part…)` (ADR-501; raised by `%raise`). The caught value is `{:kind :error :message …}` with `:code`, `:file`/`:line`/`:col` and `:trace` added | `(error "bad index: " 7)` |
| `E0091` | `:assert` | `(assert test …)` found `test` falsy: `{:kind :assert :message "assert failed: <form>[: parts]"}`, enriched the same way | `(assert (> 0 1))` |
| `E0092` | `:match` | no clause of a `match`/`case`, refutable `let` or pattern `fn` matched: `{:kind :match :message … :context :value :patterns}` — `:context` names the form (`:match`, `:let`, `:fn`, a function name), `:value` what was matched, `:patterns` what was tried | `(match 3 (1 :one))` |
| `E0099` | `:runtime` | `LispError::runtime(...)` (catch-all) | uncoded runtime raises |

`E0099` is the catch-all assigned by `LispError::runtime(...)` — every
runtime raise picks it up unless the site overrides via
`.with_code(error_codes::SOMETHING)`. As specific raises get their own
codes, they slot into the ranges above and the message becomes more
diagnostic.
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
| `(/ x 0)` / `(math/rem x 0)` | `guard the denominator: (when (not (= y 0)) (/ x y))` |
| Unbound symbol in a green process | `this fired inside a spawned process — if it happens only under fan-out load, the scheduler may be racing prelude lookups; try -j 1 …` |
| `os/run-process` failure | `check that the program is on PATH and the args are well-formed` |
| Message too deep (`E0070`) | `messages cross processes by deep copy — flatten or chunk the data (e.g. send a list of items rather than one nested tree)` |

The scheduler-race hint is conditional on two things, both in
`eval::unbound_error`: the error is raised inside a green (spawned) process
(`process::in_green_process()`), **and** the missing name is *bare*
(unqualified). The root thread (REPL / file runner / `nest mcp` dispatcher)
doesn't get it, because the race is a multi-thread scheduling phenomenon. A
*qualified* miss (`mod/name`) doesn't get it either: the race only loses bare
internal names (`fold`/`acc`/`%eq`), so a qualified miss is never that race —
it's a genuinely-absent module global. The common qualified case is a
`send`-ed closure's free global that exists only on the *sending* node:
closures travel with their captured locals but free globals late-bind on the
receiver, so a sender-only global arrives `unbound symbol: <ns>/<name>` there,
and the race story would misdirect. Documented from
[`archive/claude-demo-findings.md`](archive/claude-demo-findings.md)'s blocker §1.

## Branching on `:code` vs `:kind`

`:kind` is for **categorical** matching ("any runtime error retries; any
type error is a real bug"). `:code` is for **specific** matching ("the
scheduler race specifically — try `-j 1`"). Prefer the broader one unless
you need the precision.

```lisp fragment
;; Categorical: handle any unbound symbol uniformly.
(try
  (do-work)
  (catch (and e {:kind :unbound}) (do-handle-missing-dep e))
  (catch (and e {:kind :runtime}) (log-and-retry e)))
  ;; anything else, a type error say, matches no clause and is raised again

;; Specific: only this exact failure can be retried, not all runtimes.
(try
  (do-fan-out-work)
  (catch {:code "E0099"} (retry-with-single-thread)))
```
