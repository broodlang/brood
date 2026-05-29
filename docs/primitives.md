# Native primitive kernel

The **complete set of functions implemented in Rust** (every `Value::Native`
registered in `crates/lisp/src/builtins.rs`). Everything else in the language —
`+ - * / < = map filter reduce defn -> …` — is written *in Brood*
(`std/prelude.blsp`) on top of these. Keeping this list small is a deliberate,
load-bearing choice (ADR-006 "write the language in the language", ADR-008
"Rust is a primitive kernel").

`%`-prefixed names are low-level primitives not meant to be called directly.

The **Arity** column below is now machine-enforced: each builtin declares an
`Arity` (`value.rs`) and the evaluator checks it once, at the single native call
gate (`eval::call_native`), before the primitive runs — so a wrong-count call is
a clean arity error (`type-of: expected 1 argument, got 0`) rather than a missing
arg silently becoming `nil`.

## Native primitive functions (91)

| Category | Primitive | Arity | Purpose |
|---|---|---|---|
| **Numeric** (arithmetic substrate) | `%add` `%sub` `%mul` `%div` | 2 | int-preserving arithmetic; `%div` is exact-int-or-float and errors on ÷0 |
| | `%lt` | 2 | numeric `<` → bool |
| | `%eq` | 2 | structural equality → bool |
| | `rem` | 2 | integer remainder (truncated, sign of dividend) — **irreducible**: deriving it via float division would lose precision past 2^53. `mod` (euclidean) and `quot` (truncated division) are Brood over it |
| | `floor` | 1 | floor toward −∞ → **int** (an int passes through) — the one Float→Int crossing the language can't bootstrap. `ceil`/`round`/`sqrt`/`pow` are Brood over it |
| **Pair / sequence** | `cons` | 2 | make a pair |
| | `first` `rest` | 1 | head / tail (nil, pair, or vector) — these *are* car/cdr; `empty?` is Brood over them + the length primitives |
| **Vector** (data type, O(1)) | `vector` | n | construct a vector |
| | `vector-ref` | 2 | index |
| | `vector-length` | 1 | length |
| **Map** (immutable; data type) | `hash-map` | n | construct a map from `k v k v …` args (the `{ }` literal's programmatic form); last-wins on dup keys |
| | `map-get` | 2–3 | value at a key, or the optional default (else nil) |
| | `map-assoc` | 3 | a fresh map with `key`→`val` added/updated |
| | `map-dissoc` | 2 | a fresh map with a key removed |
| | `map-pairs` | 1 | entries as a list of `[k v]` vectors, insertion order, one O(n) pass — the sole enumerator; `keys`/`vals`/`contains?`/`reduce-kv` are all Brood over it |
| **String** | `string-length` | 1 | char count |
| | `substring` | 3 | characters `[start, end)`, char-indexed |
| | `upper` | 1 | `s` upper-cased (Unicode-aware, e.g. `ß` → `SS`) |
| | `lower` | 1 | `s` lower-cased (Unicode-aware) |
| | `string->number` | 1 | strict parse → int, else float, else `nil` (`"3abc"` → `nil`, unlike `read-string`) |
| **Rope** (editor buffer text; immutable, char-indexed — ADR-045) | `string->rope` | 1 | a rope holding the characters of a string — the constructor |
| | `rope->string` | 1 | the full text of a rope as a string (the only way a rope's content crosses a process: ropes are process-local) |
| | `rope-length` | 1 | character count |
| | `rope-line-count` | 1 | line count (a trailing newline ends a line; `""` is 1 line) |
| | `rope-insert` | 3 | `(rope-insert r idx s)` → a **fresh** rope with `s` inserted at char `idx` |
| | `rope-delete` | 3 | `(rope-delete r start end)` → a **fresh** rope with chars `[start, end)` removed |
| | `rope-slice` | 3 | text of chars `[start, end)` as a string |
| | `rope-line` | 2 | text of line `n` (0-based), including its trailing newline — the viewport primitive |
| | `rope-char->line` | 2 | 0-based line index containing a char index |
| | `rope-line->char` | 2 | char index where a 0-based line begins |
| **Type reflection** | `type-of` | 1 | the runtime type tag as a keyword (`:int` `:string` …); the one irreducible reflective primitive. The tag predicates (`nil?` `pair?` `int?` `float?` `bool?` `string?` `symbol?` `keyword?` `vector?` `map?` `fn?`) are Brood wrappers over it, as are the in-language type checks |
| **Type checking** (advisory; see [types.md](types.md)) | `check` | 1 | run the advisory type checker over a *quoted* form: macro-expand it (like the real compile pass), then return a **list of warning strings** for provably-wrong primitive arguments (e.g. `(first 5)` → `"first: argument 1 expects nil \| pair \| vector, got int (5)"`), or `nil` when nothing is wrong. Advisory: never raises |
| | `check-file` | 1 | check every top-level form in the file at `path`, returning pre-formatted `"path:line:col: warning: …"` strings (or `nil` if clean). Reads but does **not** evaluate. Used by `(check-project)` for the `nest test` / `nest run` / `nest check` pre-flight |
| **Value ↔ text & I/O** | `str` | n | concatenate the *display* forms of args → string |
| | `pr-str` | 1 | *readable* form of a value → string |
| | `print` | n | write display forms to stdout → nil (`println`, which adds a newline, is Brood over it) |
| | `eprint` | n | write display forms to **stderr** → nil (mirrors `print`; `eprintln` is the Brood newline-adding wrapper) |
| | `stdout-tty?` | 0 | true when stdout is an interactive terminal (false when piped/captured) — gates colour output |
| **Time** | `now` | 0 | wall-clock milliseconds since the Unix epoch (integer); subtract two readings for elapsed time |
| **Memory** | `mem-bytes` | 0 | bytes currently allocated process-wide (from the counting global allocator) |
| | `mem-peak` | 0 | high-water mark of allocated bytes since process start |
| **Self-hosting hooks** | `eval` | 1 | evaluate a form in the global env |
| | `read-string` | 1 | parse one form from text |
| | `eval-string` | 1 | read + evaluate every form in a string (string analogue of `load`) |
| | `load` | 1 | read + evaluate a file |
| | `%builtin-module` | 1 | source of a baked-in std module by name, or nil (used by Brood `require`) |
| | `apply` | ≥2 | call a function with a spliced argument list |
| **Symbols** | `name` | 1 | a symbol/keyword's spelling as a string (no leading `:`) |
| | `symbol` | 1 | coerce a string / symbol / keyword to the matching symbol (intern as needed). Lenient inverse of `name`; strict `string->symbol` is a Brood wrapper |
| | `keyword` | 1 | coerce a string / symbol / keyword to the matching keyword (intern as needed). Mirrors `symbol`; they share an interner so `(= (name 'x) (name :x))` |
| **Filesystem** | `cwd` | 0 | current working directory |
| | `file-exists?` `dir?` | 1 | path exists / is a directory → bool |
| | `list-dir` | 1 | entry names directly under a directory (sorted) |
| | `make-dir` | 1 | create a directory and parents (`mkdir -p`) |
| | `spit` | 2 | write a string to a file (write-side of `load`) |
| | `slurp` | 1 | read a whole file into a string (read-side of `spit`; unlike `load`, does not evaluate) |
| | `file-mtime` | 1 | last-modified time as epoch-milliseconds, or nil if missing (cheap stat; pair with `load` for hot-reload) |
| **System** | `getenv` | 1 | environment-variable value, or nil if unset |
| | `run-process` | 2 | run an external program (`prog`, args list), inherit stdio → exit code |
| **Macro support** | `macroexpand-1` `macroexpand` | 1 | expand a form (one step / fully) |
| | `gensym` | 0–1 | a fresh, unique symbol (optional name prefix) |
| **Source positions** (editor tooling) | `form-pos` | 1 | a form's `[line col]` source position vector, or nil |
| | `current-file` | 0 | path of the file currently being loaded, or nil |
| | `source-location` | 1 | `[file line col]` of where `'name` was defined (`def`/`defn`/`defmacro`/`defdyn` site), or nil. Captured pre-expansion so macros' surface forms are located accurately (ADR-031) |
| | `parse-source` | 1 | parse a `.blsp` source string into a span-carrying CST node (`Atom`/`Cst`); the formatter and LSP read structure + positions from this rather than re-reading source. ADR-025 |
| **Introspection** (editor tooling) | `doc` | 1 | a function/macro's docstring, or nil |
| | `arglist` | 1 | a function/macro's parameter list (required, `&optional`, `& rest`), or nil |
| | `global-names` | 0 | every globally bound symbol, sorted by spelling (completion / doc generation) |
| | `bound?` | 1 | whether a symbol is bound in scope → bool |
| | `dynamic?` | 1 | whether a symbol names a dynamic variable (declared via `defdyn`) → bool |
| **Errors / control** | `throw` | 1 | raise a value as an error (non-local exit) |
| | `%try` | 2 | call a thunk; on raise, call the handler with the caught value |
| | `%isolate` | 1 | call a thunk against a private copy of the globals; roll back its `def`s afterward (used by `:isolated` tests) |
| **Processes** | `spawn` | ≥1 | run a function in a new process; returns its pid |
| | `send` | 2 | copy a message into a pid's mailbox |
| | `%receive` | 3 | selective-receive primitive (matcher fn, timeout-ms-or-nil, on-timeout thunk-or-nil); `receive` is a Brood macro over it |
| | `self` | 0 | this process's pid |
| | `ref` | 0 | a fresh, globally-unique reference token (`Value::Ref`); tags request↔reply |
| | `monitor` | 1 | watch a pid (local or remote); returns a monitor ref. Delivers `[:down ref pid reason]` on death (`:noproc` if already dead; `:noconnection` if a remote peer's link drops) |
| | `demonitor` | 1 | drop a monitor by its ref (best-effort; remote demonitor is fanned out to the holding peer) |
| | `register` | 2 | bind a local name → pid so peers can address it via `{:name n :node this-node}`. Returns the pid |
| | `whereis` | 1 | the local pid registered under `name`, or nil. Strictly local — does not query other nodes |
| | `spawn-count` | 0 | green processes spawned since program start |
| | `peak-threads` | 0 | high-water mark of spawned threads running concurrently (bounded by the CLI's `-j`) |
| | `worker-threads` | 0 | size of the scheduler's worker-thread pool (≈ nproc; `-j` overrides) |
| **Distributed nodes** ([docs](distribution.md), ADR-034) | `node-start` | 3 | name this runtime (`node`, `"host:port"`, `cookie`), start the acceptor; cookie is the HMAC key for handshake v2 (never on the wire). Returns the node name |
| | `connect` | 1 | dial `"name@host:port"`, complete the v2 handshake (magic+version, nonce-exchange, HMAC challenge-response). Returns the peer's node name |
| | `node-name` | 0 | this runtime's node name (`:nonode` until `node-start`) |
| | `nodes` | 0 | list of currently connected peer node names |
| | `monitor-node` | 1 | get `[:nodedown name]` when the link to node `name` drops (heartbeat timeout or clean close). Persistent — fires on each down |

**Why this set is irreducible:** every entry needs Rust — raw number ops, heap
construct/inspect, the type-tag *reflection* (`type-of`), I/O, value→text
conversion, the wall clock, the allocator counters, the `Ty`-lattice checker
pass, or a hook into `eval`/the reader. None of it can be written in Brood. Everything that *can* be is already
in the prelude — including the tag predicates (over `type-of`), the full
arithmetic/comparison families `+ - * / < <= > >= = not=` (over `%add`/`%lt`/`%eq`),
the whole math library `mod`/`quot`/`ceil`/`round`/`pow`/`sqrt`/`even?`/`odd?` +
variadic `min`/`max` (over `rem`/`floor`/`/`/`*`/`<` — `sqrt` is Newton's method),
the whole sequence library
(`range`/`take`/`drop`/`take-while`/`drop-while`/`some?`/`every?`/`find`/`zip`/
`partition`/`sort`/`sort-by` — a Brood merge sort), `empty?` (type dispatch over
the length primitives), `println` (over `print`), and the map surface
`get`/`assoc`/`dissoc`/`keys`/`vals`/`contains?`/`reduce-kv` (over `map-get`/
`map-assoc`/`map-dissoc`/`map-pairs`). Of the math library only **`floor`** (the Float→Int crossing) and
**`rem`** (exact integer remainder) need Rust — everything else is Brood over
them. The map literal `{ }` is read by the reader and evaluated like a vector
literal — no constructor call.

## Special forms (not primitives)

These are evaluation rules in `crates/lisp/src/eval/mod.rs`, not functions — they
control how their arguments are evaluated and cannot be passed as values:

```
quote  if  do  def  fn  lambda  let  let*  quasiquote  defmacro
```

`when`, `unless`, `cond`, `and`, and `or` are **prelude macros**, not special
forms (ADR-022). There is no `set!` and no `while`: data is immutable and there is
no local mutation — `def` (redefining a global) is the only mutation, and loops
are recursion or processes (ADR-026).

---

## Error handling (implemented)

Error signalling and handling, with a minimal kernel footprint — **two new
primitives, zero new special forms** — keeping the ergonomic layer in Brood.

| New | Where | What |
|---|---|---|
| `throw` | **primitive** (kernel) | `(throw v)` raises `v` as an error — a non-local exit. |
| `%try` | **primitive** (kernel) | `(%try thunk handler)` — call `thunk` (a 0-arg fn); if it raises, call `handler` with the caught value, else return the thunk's result. The low-level catch mechanism. |
| `try` / `catch` | **prelude macro** (Brood) | `(try body... (catch e handler...))` — sugar that wraps the body and handler in `fn`s and calls `%try`. |
| `error` | **prelude** (Brood) | `(error msg & parts)` ⇒ `(throw (str msg ...))` — the common "raise a message" case. |

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
