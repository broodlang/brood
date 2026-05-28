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
| `E0099` | `:runtime` | `LispError::runtime(...)` (catch-all) | `(/ 1 0)` |

`E0099` is the catch-all assigned by `LispError::runtime(...)` — every
runtime raise picks it up by default. As specific raises get their own codes
(e.g. a dedicated `E0040` for "division by zero", `E0050` for IO failures),
they slot into the `E04xx` range and the message becomes more diagnostic.

## Adding a new code

1. Pick the next free slot in the right range.
2. Add a `pub const` to [`error_codes`](../crates/lisp/src/error.rs)
   alongside the existing ones — the name uppercases the kind.
3. Tag the raise site with `.with_code(error_codes::YOUR_NEW)`.
4. Add a row to the table above. Don't reuse a retired code.
5. If the error has a common fix, attach a `:hint` at construction via
   `.with_hint("…")`.

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
