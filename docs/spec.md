# Brood language specification

**Tracks:** Brood 0.50.0 · **Status:** draft, tracking the implementation.

This is the normative description of Brood as currently implemented. Where it
and the code disagree, that is a bug in one of them — please file/fix. The
companion [language.md](language.md) is the friendlier tutorial-style reference;
this document aims to be precise.

Brood is a dynamically-typed, lexically-scoped **Lisp-1** with proper tail
calls. "Lisp-1" means functions and variables share a single namespace
(§6).

---

## 1. Notation

Grammar is given in EBNF: `{ x }` is zero-or-more, `[ x ]` is optional, `|` is
alternation, `"x"` is literal text.

---

## 2. Lexical structure

Source is read as a sequence of Unicode characters.

```ebnf
whitespace = ? Unicode whitespace ? ;
comment    = ";" { ? any char except newline ? } newline ;
delimiter  = whitespace | "(" | ")" | "[" | "]" | "{" | "}" | '"' | ";" | "'" ;
```

Whitespace and comments separate tokens and are otherwise insignificant. The
comma `,` is treated as whitespace (Clojure-style); unquote is written `~`.

A **token** is the maximal run of non-delimiter characters, except for strings
and the reader-macro characters below.

```ebnf
token   = number | string | keyword | boolean | nil | symbol ;

integer = [ "+" | "-" ] digit { digit }
        | [ "+" | "-" ] ( "0x" | "0o" | "0b" ) radixdigit { radixdigit } ;   (* ADR-334 *)
float   = ? a token, beginning with a digit/sign/dot, that parses as an IEEE-754
            f64 and is not an integer ? ;
ratio   = integer "/" digit { digit } ;   (* 1/2 — always reduced, ADR-196 *)
decimal = ? an integer or float token ending in "M" ? ;   (* 1.50M *)
number  = integer | float | ratio | decimal ;

string  = '"' { stringchar | escape } '"' ;
escape  = "\" ( "n" | "t" | "r" | "e" | "0" | "\" | '"'
               | "x" hex hex | "u{" hex { hex } "}" ) ;

keyword = ":" symbolchar { symbolchar } ;
boolean = "true" | "false" ;
nil     = "nil" ;
symbol  = symbolchar { symbolchar } ;   (* any token that is none of the above *)
```

`symbolchar` is any non-delimiter character. A token is classified in this
order: `nil`/`true`/`false`, then integer, then float (only if it lexically
looks numeric), then keyword (leading `:`), otherwise a symbol. Thus `+`, `-`,
`->`, `empty?`, `%add`, `string/upper` are all symbols.

## 3. Syntax (reader grammar)

The reader turns text into values (§4).

```ebnf
program = { form } ;
form    = number | string | keyword | boolean | nil | symbol
        | list | vector | map | set | bytes | reader-macro ;
list    = "(" { form } ")" ;
vector  = "[" { form } "]" ;
map     = "{" { form form } "}" ;   (* alternating key/value forms *)
set     = "#{" { form } "}" ;
bytes   = '#b"' { stringchar | escape } '"' ;

reader-macro = "'"  form        (* (quote form)            *)
             | "^"  form        (* (%pin form) — a pattern PIN, §7.4.1 *)
             | "`"  form        (* (quasiquote form)       *)
             | "~"  form        (* (unquote form)          *)
             | "~@" form ;      (* (unquote-splicing form) *)
```

A map literal `{ }` holds an even number of forms (key, value, key, value, …);
an odd count is a read error. The empty list `()` reads as `nil` (§4).

## 4. Data model

A value is exactly one of:

| Kind | Notes |
|---|---|
| **nil** | the empty value; also the empty list. |
| **boolean** | `true`, `false`. |
| **integer** | 64-bit signed, with **automatic bignum promotion**: arithmetic is overflow-checked, and a result outside `i64` promotes to arbitrary precision rather than wrapping (demoting again when it fits). So the integer type is unbounded in practice. |
| **float** | IEEE-754 double. |
| **string** | immutable sequence of characters. |
| **symbol** | an interned name. |
| **keyword** | an interned, self-evaluating name (`:k`). |
| **pair** | a cons cell `(a . b)`. Proper lists are pairs chained to a final `nil`. |
| **vector** | a fixed sequence of values. |
| **map** | immutable key→value associations (`{ }`); iteration order is hash-derived (CHAMP, ADR-040), **not** insertion order; any value as a structurally-compared key. |
| **function** | a closure (`fn`) or a primitive. |
| **record** | a nominal value: an identity plus its fields, built by a `defrecord` constructor and printed `#point{:x 1, :y 2}`. Its own kind (`type-of` is `:record`, `map?` is false), never `=` to a map; `get`, keyword calls, `assoc`, `update`, `dissoc` and destructuring act on its fields and keep the record (ADR-542). |
| **failure** | an expected failure returned as a value, carrying `:message` and any fields (ADR-310, ADR-617). |
| **set** | immutable collection of distinct elements (`#{ }`); its own kind — never `=` to a map (ADR-060). |
| **bytes** | immutable byte sequence (`#b"…"`), the binary counterpart of `string`; addressed by byte, and the subject of bit-syntax patterns (ADR-140). |
| **decimal** | exact arbitrary-precision base-10 (`1.50M`), for money — values a float cannot hold. |
| **ratio** | exact rational (`1/2`), always reduced with a positive denominator; `/` on integers is exact (`(/ 1 2)` → `1/2`), and a denominator of 1 demotes to an integer (ADR-196). |
| **rope** | immutable, char-indexed editor buffer text, backed by a rope structure (ADR-045). |
| **pid** | a process identifier, carrying its node's identity. |
| **ref** | a globally-unique reference token; tags a request to its reply. |
| **table** | a shared, identity-mutable key→value store — **the one mutable kind** (see below). |

Lists are not a distinct type: a "list" is either `nil` or a pair whose chain of
`rest`s ends in `nil`.

**All values are immutable, with exactly one exception.** No operation mutates an
existing value; there are no data-mutation primitives (no `set-car!`,
`vector-set!`, `string-set!`, and no mutable reference cell of any kind — no
atoms, no Clojure-style refs or agents, no transients; Brood's **ref** kind above
is an immutable unique *token*, not a cell).
Constructors and updates (`cons`, `assoc`, `conj`, `append`, …) return a fresh
value and leave their arguments unchanged. The only *binding* mutation in the
language is `def` rebinding a global (§6) — never the contents of a value
(ADR-026).

The exception is **table**, the shared mutable table (ADR-107): a shared key→value store behind
an opaque handle, mutated in place and shared by *identity*, so a handle sent to
another process names the same store. It deep-clones keys and values on the way in
and out, so no two processes ever alias stored data — which is what keeps it
compatible with share-nothing concurrency. Every other kind is immutable, and
per-process state normally lives in a process loop's arguments instead.

## 5. Evaluation

Evaluation maps a (form, environment) pair to a value, or raises an error (§10).

1. **nil, boolean, integer, float, string, keyword, function** evaluate to
   themselves — as does every other kind that is not a symbol, pair, or vector
   (**bytes**, **decimal**, **rope**, **pid**, **ref**, **table**).
2. A **symbol** evaluates to the value bound to it, looked up per §6; an unbound
   symbol raises an error.
3. A **vector** `[e₁ … eₙ]` evaluates to a new vector of the evaluated elements,
   left to right. A **map literal** `{k₁ v₁ …}` and a **set literal** `#{e₁ … eₙ}`
   likewise evaluate their forms left to right and build a fresh map / set.
4. A **pair** `(h a₁ … aₙ)` is a *combination*:
   - If `h` is a symbol naming a **special form** (§7), the form's own rule
     applies (it decides which arguments are evaluated).
   - Otherwise `h` is evaluated to a callable `f`, then `a₁ … aₙ` are evaluated
     left to right, then `f` is **applied** to those arguments. Applying a
     closure binds its parameters (§7, `fn`) in a fresh environment whose parent
     is the closure's captured environment, and evaluates the body (an implicit
     `do`).
   - A **keyword is callable** as an accessor: `(:k m)` is `(get m :k)`, so it can
     be passed to a higher-order function (`(map people :name)`) (ADR-165).
     Nothing else data-like is — `({:a 1} :a)`, `([10 20] 1)` and `(#{1} 1)` all
     raise `cannot call non-function`, with a hint. A callable map would be a
     second spelling of `get`, and a callable vector/set answers by
     index-or-membership, an ambiguity Brood refuses.
   - Applying anything else raises an error.

### 5.1 Tail position and tail calls

The implementation guarantees **proper tail calls**: a call in tail position
uses O(1) interpreter stack. The tail positions are:

- the last form of a `do`/`when`/`let` body, and of a function body;
- both branches chosen by `if`; the chosen branch of `cond`;
- the last operand of `and`/`or`;
- the body that any of the above ultimately reduces to.

Consequently, recursion is the idiomatic and safe way to loop.

## 6. Scoping and namespaces

Brood is a **Lisp-1**: there is a single namespace. The operator position of a
combination is resolved with the same lookup as any other variable reference, so
functions are first-class values bound like any other (`(map xs f)`).
A local binding may therefore shadow a global function of the same name.

Scoping is **lexical**. An environment is a frame of bindings with an optional
parent. Lookup searches the current frame, then its parent, and so on; the
outermost frame is the **global environment**. A closure captures the
environment in which it was created. (Dynamically-scoped variables are also
implemented — `defdyn`/`binding`, per-process — see §11.)

`def` always binds in the global environment. **It is the only mutation in the
language** — rebinding a global, which is what enables live redefinition / hot
reload (ADR-026). There is no local mutation: a `let`/`fn` binding, once made,
never changes, and data is immutable. `let` introduces a child frame.

A file may open a **namespace** with `(defmodule ns …)`; each `def` in it binds
`ns/name`, and a bare reference resolves *current namespace → `(:use …)` imports →
root*. The exception is an **ambient** name — one declared with `defdyn` — which is
never namespaced, so a `def` of it from any namespace rebinds the single root
binding. Ambient status is a declaration, not a spelling: an `*earmuffed*` name
that was never declared is namespaced like any other (ADR-151).

## 7. Special forms

Special forms are reserved symbols recognised in operator position. `body...`
denotes zero or more forms evaluated as an implicit `do`.

Only `quote`, `if`, `do`, `def`, `fn`, `let`, `letrec`, and `quasiquote` are
**true core special forms** (the evaluator's own rules, in `eval.rs`) — eight
in all. `defmacro`, `when`, `cond`, `and`, and `or` are **prelude macros**
— they expand to the core forms and so can be shadowed or passed over like any
binding; they are tabled here only so the whole surface reads in one place.
`defmacro` lowers to `(def name (%make-macro (fn …)))`: a macro is just a closure
the expander calls, and `%make-macro` is the one primitive that tags it as such.

| Form | Semantics |
|---|---|
| `(quote x)` | `x`, unevaluated. Reader shorthand: `'x`. |
| `(if t a b?)` | Evaluate `t`; if truthy (§8) evaluate `a`, else `b` (or `nil`). `a`/`b` are in tail position. |
| `(when t body...)` | If `t` is truthy, evaluate `body`; else `nil`. |
| `(cond t₁ e₁ t₂ e₂ …)` | Even number of forms. Evaluate tests left to right; the first truthy test's `eᵢ` is the result (tail position). `else`, which must be the last test, always matches; a constant test (`:else`, `true`, `nil`, a number) raises at expansion (ADR-518). No match ⇒ `nil`. |
| `(do body...)` | Evaluate in order; result is the last (tail position), or `nil` if empty. |
| `(def name v?)` | Evaluate `v` (or `nil`) and bind `name` globally (redefinable — the language's only mutation). Result: `name`. |
| `(fn (params) body...)` | A closure capturing the current environment. The parameter list is a **list**, not a vector (ADR-149) — a vector there is an error. |
| `(let (n₁ v₁ …) body...)` | Sequential bindings in a new child frame (each `vᵢ` sees the previous bindings). The binding container is a **flat list**, not a vector and not Scheme's double-parens (ADR-149); a vector *inside* it is still destructuring — `(let ([x y] p) …)`. |
| `(letrec (n₁ v₁ …) body...)` | Mutually recursive bindings in a new child frame — every `nᵢ` is visible in every `vⱼ` (and to itself). Each name is pre-bound to `nil` before any `vⱼ` evaluates, so the form is for mutually recursive **functions** (their bodies fire at call time, by which point the real value is bound). Binding targets must be plain symbols. |
| `(and a₁ …)` | Left to right; returns the first falsy value, else the last (tail position). Empty ⇒ `true`. |
| `(or a₁ …)` | Left to right; returns the first truthy value, else the last (tail position). Empty ⇒ `nil`. |
| `(quasiquote tmpl)` | Build a value from a template (§7.2). Reader shorthand: `` `tmpl ``. |
| `(defmacro name (params) body...)` | Define a macro bound to `name` globally (§7.3). A prelude macro over `%make-macro`, not a core special form. |

### 7.2 Quasiquote

`` `tmpl `` returns `tmpl` as a literal, except that `~x` (`(unquote x)`) is
replaced by the value of `x`, and `~@xs` (`(unquote-splicing xs)`) splices the
elements of the sequence `xs` into the surrounding list/vector. Unquoting works
inside both lists and vectors.

Quasiquote has **levels** (ADR-503). The template being built is level 1; a nested
`` ` `` raises the level by one and each `~`/`~@` lowers it by one. A hole that brings
the level to 0 is evaluated (or spliced); anything at level 1 or deeper is rebuilt as
written, including its `quasiquote`, `unquote` and `unquote-splicing` markers, so
`` `(a `(b ~(+ 1 2))) `` is `(a (quasiquote (b (unquote (+ 1 2)))))` and, with `x`
bound to `y`, `` `(a `(b ~~x)) `` is `(a (quasiquote (b (unquote y))))`. An `x#`
auto-gensym is replaced only at level 1. A `` ` `` inside an `~unquote` that reaches
level 0 is ordinary code, starting a template of its own.

### 7.3 Macros

A macro is invoked in operator position on its **unevaluated** argument forms;
the value it returns is then evaluated in its place (and is itself subject to
further macro expansion and tail-call treatment). Macros are ordinary closures
tagged as macros, so a macro body is just Brood code that computes a form —
typically with quasiquote. A binder a template writes is renamed fresh per expansion,
at every binding site of the pattern grammar and in the standard binding macros, so
it cannot capture the caller's code (ADR-500); `x#` (the auto-gensym) and
`reflect/gensym` name a fresh symbol by hand, for a binder of the macro author's own
binding macro, and `~'name` asks for the capture. `reflect/macroexpand-1` and
`reflect/macroexpand` expand without evaluating. An expander's error is placed at the
macro call, or at the sub-form `reflect/error-at` names (ADR-618).
Macros are resolved after special forms and before function application, so a
special-form name cannot be shadowed by a macro.

### 7.4 Parameter lists

A parameter list is written as a **list** `(a b)` — code is lists (ADR-010). A
**vector** `[a b]` in this position is an error, not an accepted alias (ADR-149):
tolerating it made Clojure's `(defn f [x y] …)` and multi-arity
`(defn g ([x] …) ([x y] …))` reinterpret rather than fail. A vector *inside* the
list is a destructuring pattern (§7.5). It has three sections; each is optional,
and they appear in this order. The grammar is kept deliberately small —
simplicity for the user is the priority (ADR-011).

```ebnf
param-list = "(" spec ")" ;   (* a vector here is an error, ADR-149 *)

spec       = { required } [ "&optional" optional { optional } ] [ "&" pattern ] ;

required   = pattern ;
optional   = symbol | "(" pattern default ")" ;
default    = form ;   (* evaluated only when the argument is omitted *)
```

A `pattern` is a plain symbol or any pattern `match` takes (§7.5); a symbol is the
pattern that matches anything. A clause head is the same `spec`, followed in the body by
an optional `:when guard`, and a multi-clause `fn` tries its clauses in order, refusing
one an earlier clause shadows (ADR-619).

**Binding** happens at call time in a fresh function scope, **left to right**, so
a later `default` may reference an earlier parameter.

1. **required** — each binds to the next positional argument. Fewer positional
   arguments than required parameters is an arity error.
2. **&optional** — bound in order from the remaining positional arguments. An
   omitted optional gets its `default` (a bare symbol ⇒ `nil`).
3. **& rest** — binds to a list of all arguments past the required and
   `&optional` positionals, or `nil` if none.

**Arity:** too few required is always an error. With no `&` rest, too many
arguments is an error (the strict default — allowing up to required + number of
optionals). A `&` rest makes a trailing surplus legal.

**Examples**

```clojure fragment
(a b)                      ; exactly two
(a b & more)               ; two or more; `more` is the extras as a list
(a &optional b (c 9))      ; (f 1) => a=1 b=nil c=9 ;  (f 1 2 3) => a=1 b=2 c=3
```

**Status.** Implemented: `required`, `&optional` (with defaults), and `& rest` —
all in the closure calling convention, so `fn` and `defn` share them.
(Argument binding is core mechanism, hence the kernel rather than macro sugar.)

**No keyword arguments.** Brood has no `&key` and will not (ADR-163): three or more
optional parameters take an options map, destructured with `{:keys [...]}`.

### 7.4.1 Patterns and the pin `^`

One pattern grammar serves `match`, a refutable `let` binding, `fn`/`defn` clause
heads, and `receive` clauses (full grammar: `docs/pattern-matching.md`). A bare
symbol **binds**; to match against an existing value, **pin** it with `^`:
`^expr` (read as `(%pin expr)`) matches the current value of `expr`.

The pin is `^`, not `~`. A pin used to be spelled `~expr` — literally
`(unquote expr)` — so inside a macro's `` ` `` template the quasiquote walker
consumed it first and a pinned pattern could not be emitted by a macro at all,
which is precisely what wrapping the request/reply idiom
(`(receive ([:reply ^tag v] …))`) requires. `^` leaves `~` to
quasiquote alone (ADR-150). `~expr` in pattern position is now an error naming the
fix; Brood has no metadata, so `^` is unambiguous.

## 8. Truthiness and equality

**Truthiness.** Only `nil` and `false` are falsy. Every other value — including
`0`, `0.0`, `""`, and empty vectors, maps and sets — is truthy. The empty list reads
as `nil` (§3), so it is falsy (ADR-620).

**Equality** (`=`, built on the `%eq` primitive) is structural for `nil`,
booleans, numbers (within a type: `(= 1 1.0)` is `false`), strings, symbols,
keywords, pairs, vectors, maps, sets and bytes; two records are equal when their ids and
fields are, and a record is never `=` to a map. Functions compare by identity. `=` is
variadic and holds iff every adjacent pair is equal.

## 9. The kernel / library split

Almost the entire language is written in Brood (`std/prelude/*.blsp`). Rust
supplies only an **irreducible primitive kernel**. This split is a deliberate,
load-bearing design choice (see `CLAUDE.md` and `docs/decisions.md`).

**Primitives (Rust)** — the irreducible kernel; the full annotated set is in
[primitives.md](primitives.md). By area:

- arithmetic substrate `%add %sub %mul %div %lt %eq %rem %quot`
- the float→int crossing `math/floor`, and the floating-point functions (`%f64-sqrt %sin %exp` …)
- pairs/vectors `cons first rest empty? vector %vector-ref %vector-length`
- maps `%hash-map %map-get %map-assoc %map-dissoc %map-pairs %map-count`
- strings `count string/substring string/upper string/lower string/->number`
- reflection/checking `type-of %check`; value↔text & IO `str pr-str %render %write-out %write-err`
- self-hosting `reflect/eval reflect/eval-string reflect/load %read-string %builtin-module apply`; macros `reflect/macroexpand-1 reflect/gensym`
- filesystem/system `file/cwd file/exists? file/dir? file/ls file/mkdir file/spit file/slurp %getenv %run-process`
- symbols/tooling `%form-pos %current-file reflect/doc reflect/arglist reflect/global-names bound?`
- time/memory `%now %mem-bytes %mem-peak`; errors/control `throw %raise %try %isolate`
- processes `%spawn send %receive self ref monitor demonitor %spawn-count %peak-threads %worker-threads`

`%`-prefixed names are low-level and not intended for direct use. Note that the
type-tag predicates (`nil? pair? int? …`), `math/mod`, `io/puts`, `require-one`,
and the `receive`/`try`/`match` surfaces are **not** primitives — they are Brood,
written over the kernel above.

**Derived (Brood, in the prelude):**
the macros `defn`, `when`/`cond`/`and`/`or`, `->`, `match`/`%match`,
`receive`, and `try`/`catch`; `error`; the full arithmetic/comparison family
(`+ - * / < <= > >= = inc dec` — plus ~~`not=`~~, deprecated since 0.19.1 in favour of
`(not (= a b))` — and, in the `math` module, `math/mod`, `math/abs`, `math/max`,
`math/min`, `math/sum`, `math/product`, `math/ceil`, `math/round`, `math/pow` and
`math/sqrt`); the type-tag predicates (`nil? pair? int? float?
bool? string? symbol? keyword? vector? map? fn?`, over `type-of`); the map surface
(`get assoc dissoc keys vals contains?`); the sequence library (`range take drop
any? every? sort`, and the `seq` module: `seq/take-while`, `seq/filter`, `seq/find`,
`seq/zip`, `seq/partition`, `seq/sort-by` …) and the list helpers
(`list second third fold reduce map reverse append count nth last` …); plus the
pattern-match compiler itself.

## 10. Errors

Evaluation either yields a value or raises. Every error the language raises is a map,
`{:kind :message …}`, with a `:code`, a position when known, and a `:trace` (ADR-501; the
codes are in [error-codes.md](error-codes.md)). In the REPL an error aborts the current
form and prints a message; the session continues.

**Raising and handling.** `(throw v)` raises `v` as it is. `(error part...)` raises an
error map of kind `:error` whose message is the concatenated parts; `(assert test part...)`
raises kind `:assert`, and a pattern nothing matched raises kind `:match`. `(try body...
(catch pattern [:when guard] handler...)... [(finally cleanup...)])` evaluates the body
and, if anything raises, runs the first `catch` clause whose pattern matches; when none
does, the error is raised again from where it was first raised. `(catch e …)` catches
everything. `finally` runs on every path and its value is discarded (ADR-306). `throw`,
`%raise` and the low-level `%try` are primitives; `try`/`catch` and `error` are
macros/functions in the prelude.

**Expected failure** is a value, not a raise: `(failure part...)` and `(failure/with fields
part...)` build one, `failure?` tests it, `error-message` and `fields` read it (ADR-310,
ADR-617). A failure is truthy.

## 11. Not yet specified (planned)

The features this section once listed have all shipped: **dynamic variables**
(`defdyn` / `binding`), **map literals** `{ }` and their operations (CHAMP
tries), **modules / namespaces** (`defmodule`), a **per-process tracing GC**
(ADR-035 and its successors), **rest-parameter notation in `(sig …)`** (`&` /
`&optional`, ADR-127), **records** (`defrecord`, a value kind of their own, ADR-542),
**fusing lazy seq-views** (`seq/lmap`/`seq/lfilter`/`seq/lkeep`/`seq/lreject` threaded
with `->`, ADR-111), a **first-class set** kind with the `#{…}` literal
(ADR-060 — `type-of` reports `:set` and a set is never `=` to a map),
**callable keywords** (ADR-165/167) and **abilities** — open generic functions
with nominal dispatch (ADR-168) — are all part of the language today and
specified in the sections above.

The following are still on the roadmap and intentionally absent from this
version:

- **Unbounded lazy streams** (`iterate` and infinite producers). The fusing
  lazy seq-views above cover finite pipelines; tail-recursive accumulators and
  processes cover the rest — an unbounded generator is deferred until a concrete
  feature needs one ([`deferred.md`](deferred.md) #2).
- **Return-type dispatch and monomorphization for abilities** — additive
  refinements of ADR-168, both post-1.0.

See [ROADMAP.md](../ROADMAP.md) for sequencing.
