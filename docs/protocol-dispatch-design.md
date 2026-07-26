# Protocol dispatch & abilities — the polymorphism seam

> Status: **Slices 1–2 shipped.** `std/ability.blsp` (`defability`/`impl`/`defrecord*`,
> value-first nominal dispatch) + a checker arm + `tests/ability_test.blsp`. It unifies
> value polymorphism and drivers-as-values, detects cross-module impl conflicts, and
> dispatches records on a baked `module/name` identity.
>
> **Slice 2 (identity leak) — resolved pragmatically, no kernel change.** Verifying the
> constraints changed the plan: the `Value` layout is JIT-pinned and map ops match
> `Value::Map` with catch-alls, so a `Record` variant is a pervasive, risky change; and
> — the key realization — a record being **`≠` to a bare map is correct** (Elixir-struct
> semantics), so we do *not* want to hide the id from `=`, and a record *printing* with
> its id is informative, not a leak. The one genuinely harmful leak — an internal key
> reaching external JSON — is fixed in std (`json-encode` omits `:__id__`), and a clean
> `record?`/`record-id`/`fields` API means nothing outside `ability` touches `:__id__`.
> The only residual is cosmetic (`keys`/`count` include the id; use `fields`), deferred
> as optional polish behind a future hidden slot.
>
> **Still open:** checker nominal-awareness + monomorphization, sealed abilities,
> return-type dispatch, migrating/retiring `protocol`. The rest of this note captures the
> problem, measurements, the language survey, and the design space that led here.

## The goal

Make protocols *more used and more useful*. Concretely, the property we want is
the two-sided one — the **expression problem** stated the way a user would:

> Let an author specify a **behaviour / ability** without worrying about who will
> implement it, **and vice versa** — let a type gain abilities without its author
> having to know, up front, which abilities will ever consume it.

That "and vice versa" is the sharp part. It rules *out* declaration-site opt-in
(Java `implements`, Rust `impl`, Roc `implements` — and Brood's **current**
`:implements`), because those make the *type* author name the abilities in
advance. It favours either **retroactive registration** (anyone wires any type to
any ability) or **structural satisfaction** (satisfaction is *derived*, never
declared).

## Where Brood is today (measured, not assumed)

- **Protocols** (`std/protocol.blsp`, ADR-158): `defprotocol` / `defimpl`, open
  generic functions dispatching on `(type-of first-arg)`. Two registries —
  `*protocols*` (op specs, read by the checker/LSP) and `*impls*` (the dispatch
  table).
- **Behaviours** (`defbehaviour` / `(:implements …)`): a *module* contract. Value
  dispatch does **not** happen; a module satisfies it by defining functions. And
  `:implements` is a **checker-only annotation** — there is *no* runtime
  "module → contracts" table (prelude notes this explicitly).
- **Records are structural** (ADR-130): `defrecord` is sugar over a plain map — no
  new `Value`, no tag, no `point?`. So every record dispatches as `:map`.
- **Dispatch runtime, just completed:** `*impls*` nested to `{[proto op] → {kind →
  fn}}` with the `[proto op]` key emitted as a quoted literal (no per-call
  allocation); the generic fn calls the resolved impl directly (gensym'd
  temporaries, no `apply`); added `satisfies?`; the missing-impl error now lists
  the kinds that are implemented. Hot-reload-safe (reads the live global each
  call), node-local, checker-untouched.

**The tell:** in the whole of `std`, exactly one module — `std/editor/treesit`
— uses the protocol facility. That is the symptom to explain.

## The wall

`type-of` distinguishes only ~13 built-in kinds (`:int :string :vector :map …`).
Every *application* type is a structural map (record → `:map`) or a tagged vector
(`[:circle r]` → `:vector`). So a protocol can tell an `:int` from a `:string`
but **cannot tell one record from another** — both are `:map`. The single most
common reason to reach for a protocol — "make *my* type do the right thing with
the standard machinery" — is exactly the case Brood can't express. So nobody
reaches. Speed was never the blocker; **dispatch identity for user types** is.

## Two levers, and why they're coupled

- **Lever 1 — route stdlib cross-cutting ops through standard protocols** (`Encode`
  for JSON, `Show`, `Seq`, `Compare`). This is the conventional architecture
  (Clojure/Elixir expose core abstractions as protocols). Pure `std` change, ships
  now. **But** its *extension* value is capped by the same wall: a record already
  encodes/prints/seqs *as its map*, so all a user can newly extend is "wrap a
  built-in kind differently" — niche.
- **Lever 2 — give user types a dispatch identity.** This is the actual driver of
  "more used": without it, standard protocols serve only the stdlib.

They **chain, not compete.** Lever 1 provides *protocols worth extending* (a
vocabulary); Lever 2 lets app types extend them. The payoff is the product, not
the sum. Do Lever 1 alone and app devs still can't dispatch on their domain types
→ adoption stays put. Do Lever 2 with nothing to plug into → nothing to extend.

| Dimension | Lever 1 (protocols on `type-of`) | Lever 2 (dispatch identity) |
|---|---|---|
| Unlocks | stdlib ops across 13 kinds; records ride *as maps* | records dispatch **as themselves** |
| Drives app adoption? | barely | **yes** |
| Change surface | `std`-only | see below — *not* necessarily kernel |
| ADR-130 (records=maps) | untouched | preserved for data ops; adds dispatch-only identity |
| Reopens ADR-011? | no | yes (on safe, explicit terms) |
| Ships | now | after this note resolves |

## The module investigation (the surprising part)

"Brood has modules — aren't those the nominal identity?" Investigated in the
kernel. Findings:

- **Modules are compile-time namespaces, not runtime values.** The `Value` enum
  has no `Module` / `Record` / `Struct` / `Tag` variant. A module is a symbol
  prefix (`db/start`) over one flat global table. You cannot make a value point at
  its module the way an Elixir struct points at `__struct__: Date` — there is no
  module object to point at.
- **But `(current-ns)` yields the current *compilation* namespace as a symbol**,
  capturable at macro-expansion. So a value can carry its module *by name*.
- **Trap:** runtime `(current-ns)` reflects the *caller's* compile context, not the
  definition site. Capture must happen at expansion, baked as a literal.

### Consequence: per-record dispatch needs ZERO kernel change

It's pure macro sugar. A `defrecord` variant bakes a **namespaced identity**
(`module/name`) into each value at expansion; a `dispatch-identity` returns that
(else `type-of`); the protocol keys on it — a drop-in, since the shipped registry
already keys on an opaque `key`. Prototype (`geometry` module, two records):

```
circle area: 12.56636      rect area: 12          ; two records, one module, dispatched apart
ids: geometry/circle | geometry/rect              ; identity captured at expansion
ADR-011 plain map w/ :type not rerouted: :map     ; a bare {:type …} map is NOT hijacked
```

Refinements the prototype forced:

1. **Identity is the namespaced record-*name*, not the module.** A module holds
   many records (`geometry` → circle *and* rect); bare-module identity collides.
   Modules serve as the *namespace that disambiguates* nominal record names — the
   real substrate. (Elixir's one-struct-per-module, relaxed.)
2. **ADR-011 is threaded cleanly.** Only values built by the identity constructor
   dispatch nominally; a plain `{:type …}` map stays `:map`. Explicit,
   construction-time, zero inference — exactly the implicit version ADR-011
   rejected, made safe.
3. Openness survives: the registry keys on the `geometry/circle` symbol, so anyone
   can `defimpl Encode geometry/circle` from any module.

### The cost that decides it: the tag leaks

Because the identity is a **visible map field**, it leaks into every structural
view:

```
circle keys: (:__id__ :r)                          ; shows up in keys
(circle 2) = {:r 2}? false                          ; breaks structural = vs a bare map
JSON of circle: {"__id__":"geometry/circle","r":2}  ; leaks into serialization
```

That last line is the killer: the flagship use case for per-record dispatch is
*custom serialization*, and the naïve tag pollutes exactly that. Every generic
map consumer (`json-encode`, `pr-str`, `keys`, `=`) would have to learn to skip
`:__id__`. Elixir avoids this because `__struct__` is hidden from `==`-vs-map and
its encoders strip it.

**So: zero-kernel is feasible but leaky; a clean version needs a bounded kernel
carve-out** — a hidden identity slot on maps (or a real `Record` variant) that
`keys` / `=` / `pr-str` / `json-encode` / `type-of` ignore. That carve-out *is* an
ADR-130 amendment. The decision is a spectrum, not a binary:

| | **2-lite** (pure macro) | **2-clean** (minimal kernel) |
|---|---|---|
| Kernel change | none | hidden identity slot / `Record` variant |
| Ships | today | after an ADR-130 carve-out |
| `keys` / `=` / JSON | **leak `:__id__`** | clean |
| Serialization use case | undermined by the leak | works |
| Openness / ADR-011 | ✓ / ✓ | ✓ / ✓ |
| Good for | validating ergonomics + registry design | the real "encode my record my way" |

## The wider look — how other languages decouple the two sides

Filtered by our requirement (*two-sided* decoupling + coherence):

| System | "T has ability A" established by… | Two-sided? | Coherence | Resolved |
|---|---|---|---|---|
| CLOS generic fns | `defmethod` anywhere, multiple dispatch | ✓✓ | none (specificity + `call-next-method`) | runtime |
| Clojure protocols / `extend` | retroactive: any type → any protocol | ✓✓ (incl. foreign) | **none** — last load wins, silently | runtime (cached) |
| Clojure multimethods | arbitrary dispatch fn + `derive` | ✓✓ | `prefer-method` (manual) | runtime |
| Racket generics / `prop:` / units | author-declared *or* struct-property; units = 1st-class modules | ✓ | scoped-ish | runtime |
| Haskell typeclasses | `instance` (author *or* orphan) | ✓ (orphans discouraged) | **global uniqueness** (orphan rule) | compile-time, dict-passing |
| Rust traits | `impl`, orphan rule | partial (newtype workaround) | **enforced** | static or `dyn` |
| ML functors / signatures | consumer *applies* functor to structure | ✓ (explicit at use) | coherent by choice | compile-time, explicit witness |
| Scala `given`/`using`, OCaml modular implicits | scoped implicit values | ✓ | **scoped** (the dial's middle) | compile-time, inferred |
| **Go interfaces** | **structural — derived from method presence** | ✓✓ **+ no registration** | **N/A — nothing to conflict** | runtime (itab, cached) |

Two rows matter most because Brood is already shaped like them:

- **Go — structural, implicit satisfaction.** An interface is a set of method
  signatures; a type satisfies it iff it has the methods — no `implements`, no
  registration, ever. A *third* package can define an interface *after* the types
  and existing types satisfy it retroactively. There is **no coherence problem
  because there are no instances to conflict** — satisfaction is a derived fact.
  Cost: the method set is owned by the type's package (extend a foreign type by
  *wrapping* it), and structural matching can accept an unrelated type of the same
  shape.
- **ML modules/functors — explicit witnesses.** Total decoupling *and* coherence
  because the *consumer* picks the implementing structure. Un-Lispy and verbose —
  and Brood can't do it cleanly anyway, since modules aren't runtime values
  (nothing to pass) without reifying them (a big change).

## The fork this narrows to

Brood is already built for **Go-style structural satisfaction**: names resolve as
`module/op` over one flat table; `(current-ns)` captures a record's module
identity; `bound?` already answers "does this op resolve." So the Brood-native
shape is to **collapse `behaviour` and `protocol` into one `ability` concept that
is structurally satisfied**: an ability names ops; a record (carrying its
`module/name` identity) *satisfies* it iff those ops resolve for it —
`(bound? 'geometry/area)` — with no `defimpl`, no `:implements`, no registration.
That delivers every property asked for:

- ability author ⟂ implementors (an ability is a name + op list, never enumerates
  types);
- type author ⟂ abilities (a record just provides ops in its module);
- **coherent by construction** — nothing is registered, so nothing conflicts.

Honest trade vs. the registry route: structural satisfaction **loses retroactive
extension of a *foreign* record** (its ops live in its own module → write an
adapter, as Go makes you wrap), and can occasionally say "yes" by shape accident.

- **Structural / Go (current lean):** one `ability`, satisfied by op-resolution
  over `module/name`. Least machinery, coherent, maximally decoupled on both axes,
  unifies behaviours + protocols, uses only what Brood already has. Adapters for
  foreign retroactive extension.
- **Registry / Clojure (the prototype):** keep `defimpl` on record-name identity.
  Buys foreign retroactive extension, at the cost of coherence *and* the leaky-tag
  problem above.

## Invention space (open)

We are not obliged to pick an existing point. Seeds for a Brood-native synthesis:

- **Structural-first with an explicit override seam.** Default to Go-style
  structural satisfaction (coherent, zero-registration) for the common case, and
  provide a *narrow* explicit registry escape hatch for the orphan/foreign case
  (adapter-as-data). Neither Go nor Clojure does exactly this — it would give Go's
  coherence-for-free where it applies and Clojure's reach where you truly need it,
  with the escape hatch visibly opt-in so it never silently causes incoherence.
- **Identity without a leaking field.** Whatever the mechanism, the record's
  nominal identity must be invisible to `keys` / `=` / `pr-str` / `json-encode`.
  That is the one place a kernel carve-out earns its keep (hidden slot or `Record`
  variant).
- **Behaviours and protocols as one thing.** They already share the namespace as
  an identity source. An `ability` that is *sometimes* checked structurally (a
  module provides the ops — today's behaviour) and *sometimes* dispatched on a
  value's identity (today's protocol) may be two views of one concept.

### Constraints any candidate must satisfy

1. **Two-sided decoupling** — the stated goal; no declaration-site opt-in on the
   type.
2. **Preserve ADR-130** — records stay structural for `get` / `assoc` / `=`.
3. **Preserve the `type-of` contract** — 13 kinds; records still report `:map`.
   Dispatch identity is a *separate* notion layered on top.
4. **No tag leak** — identity invisible to structural/serialization views.
5. **Hot-reload-safe** — resolution reads live globals; nothing frozen at
   expansion that a later (re)definition should change.
6. **Node-local** — dispatch resolves against the running node's code; values
   crossing the wire carry no impl (Erlang's "code must be loaded on both nodes").
7. **Coherent, or explicitly-not** — conflicts are impossible by construction, or
   any incoherence is visibly opt-in, never silent.
8. **Minimal machinery** — Brood's ethos (small, structural, immutable). Prefer
   reusing `module/name` resolution, `current-ns`, `bound?`, and the shipped
   registry over inventing parallel systems.

## Already shipped vs. open

- **Shipped:** ADR-158 protocol facility; the protocol dispatch-runtime work (nested
  registry, inlined calls, `satisfies?`, richer missing-impl error). **Slice 1 of the
  unified facility:** `std/ability.blsp` — `defability`/`impl`/`defrecord*`, value-first
  nominal dispatch (record identity or `type-of`), drivers-as-values, provenance-tagged
  cross-module conflict detection, `satisfies?`, `:default`; the `defability`/`impl`
  checker arm (arity/missing/undeclared-op diagnostics under the noun "ability"); and
  `tests/ability_test.blsp`. `defbehaviour`/`defprotocol` are untouched and coexist.
- **Open (Slice 2+):** the kernel carve-out to stop `:__id__` leaking into
  `keys`/`=`/`json-encode`; checker nominal-awareness + monomorphization; sealed
  abilities (exhaustiveness + full static dispatch); return-type dispatch; and
  migrating/retiring `protocol` once `ability` proves out. `defbehaviour` stays — the
  module-as-implementor contract (Q3) is genuinely different from value dispatch.
