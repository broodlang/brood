# mylisp language specification

**Version:** 0.1 · **Status:** draft, tracking the implementation.

This is the normative description of mylisp as currently implemented. Where it
and the code disagree, that is a bug in one of them — please file/fix. The
companion [language.md](language.md) is the friendlier tutorial-style reference;
this document aims to be precise.

mylisp is a dynamically-typed, lexically-scoped **Lisp-1** with proper tail
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

integer = [ "+" | "-" ] digit { digit } ;
float   = ? a token, beginning with a digit/sign/dot, that parses as an IEEE-754
            f64 and is not an integer ? ;
number  = integer | float ;

string  = '"' { stringchar | escape } '"' ;
escape  = "\" ( "n" | "t" | "r" | "0" | "\" | '"' | ? any char ? ) ;

keyword = ":" symbolchar { symbolchar } ;
boolean = "true" | "false" ;
nil     = "nil" ;
symbol  = symbolchar { symbolchar } ;   (* any token that is none of the above *)
```

`symbolchar` is any non-delimiter character. A token is classified in this
order: `nil`/`true`/`false`, then integer, then float (only if it lexically
looks numeric), then keyword (leading `:`), otherwise a symbol. Thus `+`, `-`,
`->`, `empty?`, `%add`, `string-length` are all symbols.

## 3. Syntax (reader grammar)

The reader turns text into values (§4).

```ebnf
program = { form } ;
form    = number | string | keyword | boolean | nil | symbol
        | list | vector | reader-macro ;
list    = "(" { form } ")" ;
vector  = "[" { form } "]" ;

reader-macro = "'"  form        (* (quote form)            *)
             | "`"  form        (* (quasiquote form)       *)
             | "~"  form        (* (unquote form)          *)
             | "~@" form ;      (* (unquote-splicing form) *)
```

`{ }` is reserved for map literals and is currently a read error. The empty list
`()` reads as `nil` (§4).

## 4. Data model

A value is exactly one of:

| Kind | Notes |
|---|---|
| **nil** | the empty value; also the empty list. |
| **boolean** | `true`, `false`. |
| **integer** | 64-bit signed. |
| **float** | IEEE-754 double. |
| **string** | immutable sequence of characters. |
| **symbol** | an interned name. |
| **keyword** | an interned, self-evaluating name (`:k`). |
| **pair** | a cons cell `(a . b)`. Proper lists are pairs chained to a final `nil`. |
| **vector** | a fixed sequence of values. |
| **function** | a closure (`fn`) or a primitive. |

Lists are not a distinct type: a "list" is either `nil` or a pair whose chain of
`rest`s ends in `nil`.

## 5. Evaluation

Evaluation maps a (form, environment) pair to a value, or raises an error (§10).

1. **nil, boolean, integer, float, string, keyword, function** evaluate to
   themselves.
2. A **symbol** evaluates to the value bound to it, looked up per §6; an unbound
   symbol raises an error.
3. A **vector** `[e₁ … eₙ]` evaluates to a new vector of the evaluated elements,
   left to right.
4. A **pair** `(h a₁ … aₙ)` is a *combination*:
   - If `h` is a symbol naming a **special form** (§7), the form's own rule
     applies (it decides which arguments are evaluated).
   - Otherwise `h` is evaluated to a function `f`, then `a₁ … aₙ` are evaluated
     left to right, then `f` is **applied** to those arguments. Applying a
     closure binds its parameters (§7, `fn`) in a fresh environment whose parent
     is the closure's captured environment, and evaluates the body (an implicit
     `do`). Applying a non-function raises an error.

### 5.1 Tail position and tail calls

The implementation guarantees **proper tail calls**: a call in tail position
uses O(1) interpreter stack. The tail positions are:

- the last form of a `do`/`when`/`unless`/`let` body, and of a function body;
- both branches chosen by `if`; the chosen branch of `cond`;
- the last operand of `and`/`or`;
- the body that any of the above ultimately reduces to.

Consequently, recursion is the idiomatic and safe way to loop.

## 6. Scoping and namespaces

mylisp is a **Lisp-1**: there is a single namespace. The operator position of a
combination is resolved with the same lookup as any other variable reference, so
functions are first-class values bound like any other (`(def + …)`, `(map f xs)`).
A local binding may therefore shadow a global function of the same name.

Scoping is **lexical**. An environment is a frame of bindings with an optional
parent. Lookup searches the current frame, then its parent, and so on; the
outermost frame is the **global environment**. A closure captures the
environment in which it was created. (Dynamically-scoped variables are planned
but not yet implemented — see §11.)

`def` always binds in the global environment. `set!` mutates the nearest
existing binding and errors if none exists. `let` introduces a child frame.

## 7. Special forms

Special forms are reserved symbols recognised in operator position. `body...`
denotes zero or more forms evaluated as an implicit `do`.

| Form | Semantics |
|---|---|
| `(quote x)` | `x`, unevaluated. Reader shorthand: `'x`. |
| `(if t a b?)` | Evaluate `t`; if truthy (§8) evaluate `a`, else `b` (or `nil`). `a`/`b` are in tail position. |
| `(when t body...)` | If `t` is truthy, evaluate `body`; else `nil`. |
| `(unless t body...)` | If `t` is falsy, evaluate `body`; else `nil`. |
| `(cond t₁ e₁ t₂ e₂ …)` | Even number of forms. Evaluate tests left to right; the first truthy test's `eᵢ` is the result (tail position). `else` or `:else` as a test always matches. No match ⇒ `nil`. |
| `(do body...)` | Evaluate in order; result is the last (tail position), or `nil` if empty. |
| `(def name v?)` | Evaluate `v` (or `nil`) and bind `name` globally. Result: `name`. |
| `(set! name v)` | Evaluate `v`, assign to the nearest existing binding of `name`; error if unbound. |
| `(fn [params] body...)` | A closure capturing the current environment. `lambda` is an alias. |
| `(let [n₁ v₁ …] body...)` | Sequential bindings in a new child frame (each `vᵢ` sees the previous bindings). `let*` is an alias. |
| `(and a₁ …)` | Left to right; returns the first falsy value, else the last (tail position). Empty ⇒ `true`. |
| `(or a₁ …)` | Left to right; returns the first truthy value, else the last (tail position). Empty ⇒ `nil`. |
| `(while t body...)` | While `t` is truthy, evaluate `body` for effect. Returns `nil`. |
| `(quasiquote tmpl)` | Build a value from a template (§7.2). Reader shorthand: `` `tmpl ``. |
| `(defmacro name [params] body...)` | Define a macro bound to `name` globally (§7.3). |

### 7.2 Quasiquote

`` `tmpl `` returns `tmpl` as a literal, except that `~x` (`(unquote x)`) is
replaced by the value of `x`, and `~@xs` (`(unquote-splicing xs)`) splices the
elements of the sequence `xs` into the surrounding list/vector. Unquoting works
inside both lists and vectors. Nested quasiquote is not level-tracked in v0.1:
unquotes resolve at the first enclosing quasiquote.

### 7.3 Macros

A macro is invoked in operator position on its **unevaluated** argument forms;
the value it returns is then evaluated in its place (and is itself subject to
further macro expansion and tail-call treatment). Macros are ordinary closures
tagged as macros, so a macro body is just mylisp code that computes a form —
typically with quasiquote. `gensym` yields fresh symbols for
hygiene-by-convention. `macroexpand-1`/`macroexpand` expand without evaluating.
Macros are resolved after special forms and before function application, so a
special-form name cannot be shadowed by a macro.

### 7.4 Parameter lists

A parameter list is written as a **list** `(a b)` (idiomatic — code is lists,
ADR-010) or a vector `[a b]` (accepted). It has three sections; each is optional,
and they appear in this order. The grammar is kept deliberately small —
simplicity for the user is the priority (ADR-011).

```ebnf
param-list = "(" spec ")" | "[" spec "]" ;

spec       = { required } [ "&optional" optional { optional } ] [ "&" symbol ] ;

required   = symbol ;
optional   = symbol | "(" symbol default ")" ;
default    = form ;   (* evaluated only when the argument is omitted *)
```

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

```clojure
(a b)                      ; exactly two
(a b & more)               ; two or more; `more` is the extras as a list
(a &optional b (c 9))      ; (f 1) => a=1 b=nil c=9 ;  (f 1 2 3) => a=1 b=2 c=3
```

**Status.** Implemented: `required`, `&optional` (with defaults), and `& rest` —
all in the closure calling convention, so `fn`, `lambda`, and `defn` share them.
(Argument binding is core mechanism, hence the kernel rather than macro sugar.)

**Deferred (designed, not in v1) — keyword arguments.** Named, order-independent
arguments (`&key (width 80) ...`, called `:width 100`) were designed and are a
natural fit for the eventual editor command API. They are *deferred for
simplicity*: they make the user learn keyword pairs, order-independence, and
mixing rules. They are purely additive — adding them later needs no migration of
existing code. Supplied-p flags and required-keyword markers are likewise
out of scope. See `docs/devlog.md` for the design discussion.

## 8. Truthiness and equality

**Truthiness.** Only `nil` and `false` are falsy. Every other value — including
`0`, `0.0`, `""`, and empty collections — is truthy.

**Equality** (`=`, built on the `%eq` primitive) is structural for `nil`,
booleans, numbers (within a type: `(= 1 1.0)` is `false`), strings, symbols,
keywords, pairs, and vectors. Functions compare by identity. `=` is variadic and
holds iff every adjacent pair is equal.

## 9. The kernel / library split

Almost the entire language is written in mylisp (`std/prelude.lisp`). Rust
supplies only an **irreducible primitive kernel**. This split is a deliberate,
load-bearing design choice (see `CLAUDE.md` and `docs/decisions.md`).

**Primitives (Rust):**
`%add %sub %mul %div %lt %eq mod rem` ·
`cons first rest empty?` ·
`vector vector-ref vector-length` · `string-length` ·
the type-tag predicates `nil? pair? int? float? bool? string? symbol? keyword?
vector? fn?` ·
`str pr-str print println` ·
`eval read-string load apply` ·
`macroexpand macroexpand-1 gensym`.

`%`-prefixed names are low-level and not intended for direct use.

**Derived (mylisp, in the prelude):**
the `defn` macro and the `->`/`->>` threading macros; `not + - * / inc dec < <=
> >= = not= number? list? car cdr list second third fold reduce map filter
reverse append count length nth identity zero? positive? negative? abs max min
sum product`, plus the helpers `chain?`, `append-two`, `nth-list`,
`thread-first-step`, `thread-last-step`.

## 10. Errors

Evaluation either yields a value or raises an error carrying a kind (`parse`,
`unbound`, `arity`, `type`, `runtime`) and a message. In the REPL an error
aborts the current form and prints a message; the session continues. There is no
in-language error handling (`try`/`catch`) yet — see §11.

## 11. Not yet specified (planned)

The following are on the roadmap and intentionally absent from this version:

- **Dynamic variables** (`defdyn` / `binding`).
- **In-language error handling** (`try`/`catch`, `throw`).
- **Map literals** `{ }` and map operations.
- **Modules / namespaces** beyond the single global environment.
- A **tracing GC** (the current `Rc` model leaks reference cycles).

See [roadmap.md](roadmap.md) for sequencing.
