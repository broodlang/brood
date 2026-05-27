# Native primitive kernel

The **complete set of functions implemented in Rust** (every `Value::Native`
registered in `crates/lisp/src/builtins.rs`). Everything else in the language —
`+ - * / < = map filter reduce defn -> …` — is written *in mylisp*
(`std/prelude.lisp`) on top of these. Keeping this list small is a deliberate,
load-bearing choice (ADR-006 "write the language in the language", ADR-008
"Rust is a primitive kernel").

`%`-prefixed names are low-level primitives not meant to be called directly.

## Native primitive functions (44)

| Category | Primitive | Arity | Purpose |
|---|---|---|---|
| **Numeric** (arithmetic substrate) | `%add` `%sub` `%mul` `%div` | 2 | int-preserving arithmetic; `%div` is exact-int-or-float and errors on ÷0 |
| | `%lt` | 2 | numeric `<` → bool |
| | `%eq` | 2 | structural equality → bool |
| | `mod` `rem` | 2 | integer modulo / remainder |
| **Pair / sequence** | `cons` | 2 | make a pair |
| | `first` `rest` | 1 | head / tail (nil, pair, or vector) |
| | `empty?` | 1 | empty collection? (nil / string / vector / pair) |
| **Vector** (data type, O(1)) | `vector` | n | construct a vector |
| | `vector-ref` | 2 | index |
| | `vector-length` | 1 | length |
| **String** | `string-length` | 1 | char count |
| **Type tags** (not expressible in-language) | `nil?` `pair?` `int?` `float?` `bool?` `string?` `symbol?` `keyword?` `vector?` `fn?` | 1 | tag test → bool |
| **Value ↔ text & I/O** | `str` | n | concatenate the *display* forms of args → string |
| | `pr-str` | 1 | *readable* form of a value → string |
| | `print` `println` | n | write display forms to stdout → nil |
| **Self-hosting hooks** | `eval` | 1 | evaluate a form in the global env |
| | `read-string` | 1 | parse one form from text |
| | `load` | 1 | read + evaluate a file |
| | `require` | 1 | load an embedded std-library module by name (e.g. `'test`) |
| | `apply` | ≥2 | call a function with a spliced argument list |
| **Macro support** | `macroexpand-1` `macroexpand` | 1 | expand a form (one step / fully) |
| | `gensym` | 0–1 | a fresh, unique symbol (optional name prefix) |
| **Errors / control** | `throw` | 1 | raise a value as an error (non-local exit) |
| | `%try` | 2 | call a thunk; on raise, call the handler with the caught value |
| **Processes** | `spawn` | ≥1 | run a function in a new process; returns its pid |
| | `send` | 2 | copy a message into a pid's mailbox |
| | `receive` | 0 | take the next message from this process's mailbox (blocking) |
| | `self` | 0 | this process's pid |

**Why this set is irreducible:** every entry needs Rust — raw number ops, heap
construct/inspect, type-tag tests, I/O, value→text conversion, or a hook into
`eval`/the reader. None of it can be written in mylisp. Everything that *can* be
is already in the prelude.

## Special forms (not primitives)

These are evaluation rules in `crates/lisp/src/eval.rs`, not functions — they
control how their arguments are evaluated and cannot be passed as values:

```
quote  if  when  unless  cond  do  def  set!  fn  lambda
let  let*  and  or  while  quasiquote  defmacro
```

---

## Error handling (implemented)

Error signalling and handling, with a minimal kernel footprint — **two new
primitives, zero new special forms** — keeping the ergonomic layer in mylisp.

| New | Where | What |
|---|---|---|
| `throw` | **primitive** (kernel) | `(throw v)` raises `v` as an error — a non-local exit. |
| `%try` | **primitive** (kernel) | `(%try thunk handler)` — call `thunk` (a 0-arg fn); if it raises, call `handler` with the caught value, else return the thunk's result. The low-level catch mechanism. |
| `try` / `catch` | **prelude macro** (mylisp) | `(try body... (catch e handler...))` — sugar that wraps the body and handler in `fn`s and calls `%try`. |
| `error` | **prelude** (mylisp) | `(error msg & parts)` ⇒ `(throw (str msg ...))` — the common "raise a message" case. |

Net kernel growth: **+2 primitives (`throw`, `%try`), and zero new special forms.**
The `try`/`catch` *syntax* is a macro written in the language — keeping the
evaluator's special-form set unchanged, per "the language must be as small as
possible." Two functions are a smaller addition to the *language* than one
special form, because special forms are core evaluator semantics while
primitives are just Rust-implemented functions.

### Supporting change

`LispError` gains an optional payload so a thrown value can ride along the error:

```rust
struct LispError { kind: ErrorKind, message: String, payload: Option<Value> }
```

`throw` sets `payload`; built-in errors (e.g. `%div` ÷0, arity, type) leave it
`None`.

### `try` / `catch` semantics

```clojure
(try
  (risky-thing)
  (catch e
    (println "failed:" e)
    :recovered))
```

- Evaluate the body forms in order; the value of the last is the result.
- If a body form raises, bind `e` to the **caught value** and evaluate the
  handler forms; the value of the last handler is the result.
- The `catch` clause is the last form of the `try`.
- (No `finally` in v1 — can add later.)

It desugars to the `%try` primitive:

```clojure
(try a b (catch e h))
;; expands to:
(%try (fn () a b) (fn (e) h))
```

### What `catch` binds

For `(throw v)`, `e` is the thrown value `v`. For a built-in error (e.g. `%div`
÷0, arity, type), `e` is the error's **message string** (e.g. `"runtime error:
division by zero"`). `error` throws a string, so `e` is that string too.

This was a deliberate choice for simplicity (ADR-011): it loses the structured
`kind`, but is trivial to use. A structured error value (carrying `kind` +
`message` + payload) can replace the message-string fallback once map literals
exist — a backward-compatible refinement.
