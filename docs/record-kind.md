# Records are maps with a magic key — leaks today, and a real kind later

**Status: IMPLEMENTED 2026-11-04 — option A, ADR-542.** Step 1 (the kernel kind and its
`%record` primitives), step 2 (`defrecord` constructs one; every `:__id__` probe is gone; node
links at protocol v12) and step 3 (the checker types a record by the `record` tag) are in; the
findings below describe the tree before them. Earlier status: **OPEN — scoped
2026-10-27.** It has two parts. Of the **leaks** (§2), `into` and
`merge` were **fixed 2026-10-27**: a record source is read through `seq`, and `merge` is seeded
with its first map. The `assoc :__id__` forge is still open (see §2). A **`Record` value kind**
(§4) would reverse ADR-130 and ADR-168 on purpose. It's a larger change, estimated at 50–70
files.

## The finding

A `defrecord` value is a map with a reserved `:__id__` key (`(point 3 4)` →
`{:__id__ :ns/point, :x 3, :y 4}`). The language treats it as a map in some places and as
nominal in others:

- `(type-of r)` is `:map`, and `(get r :__id__)` reaches the id;
- `=` is nominal: a record never equals a bare map with the same fields;
- `seq`/`count`/`keys`/`into`/`conj` hide the id (`Seqable`, `Conjable`, `%count-record`);
- the printer, equality, hashing, messaging and the wire format don't know records exist. The
  id is just one more field to them.

Each map operation decides for itself whether the id is there, so some of them get it wrong.

## 1. How it works today (verified)

Everything is prelude Brood. The only Rust support is the interned key (`core/keywords.rs:162`,
`core/value.rs:83-88`).

- **The macro** is in `std/prelude/seq.blsp:358-545`. The constructor is a map literal with
  `:__id__` consed onto the field keys (`:469`). Accessors expand to `(/get r :kw)`. Ids are
  registered in `*record-ids*` (`:418-422`) for constructor devirtualization.
- **Predicates** are in `std/prelude/abilities.blsp:3-39`: `%identity-of`, `record?`,
  `record-id`, and `fields` (which is `(dissoc x :__id__)`). Its comment says "nothing outside
  this module reads `:__id__`", but five other prelude files do.
- **Hiding the id:** `seq` (`core.blsp:204`), `count` (`predicates.blsp:370`), `keys` and the
  `get`-miss `Lookup` fallback (`map.blsp:51,178`), `into`/`conj` (`seq.blsp:278,311`).
- **The Rust paths that probe `:__id__`:**
  - the VM/JIT inline `get` and dispatch (`core/heap/vm_cache.rs:852-930`);
  - `count_inline` (`heap.rs:2220`);
  - `record_seq`/`record_counted` (`builtins/sequences.rs:509,700`);
  - `is_record`, which routes arithmetic and ordering to `Num`/`Ord`
    (`builtins/numeric.rs:575`);
  - JSON encode (`builtins/json.rs:91`);
  - constructor devirtualization (`eval/compile/inline.rs:88-177`);
  - LSP completion (`crates/lsp/src/completion.rs:372`).

  In all, 105 lines across 27 Rust files, 10 of them checker files.
- **The checker** has no Record tag. A record is `Tag::Map` with a shape whose `__id__` field
  is a keyword literal. A sealed ability's type is `%{__id__: (:a | :b …)}`
  (`types/check/annot.rs:381-527`, `protocol.rs:604`), and exhaustiveness checking keys on that
  field.

## 2. The leaks (confirmed 2026-10-27 with a one-file probe)

```clojure fragment
(defrecord point (x y))
(def p (point 1 2))
(record? (into {} p))                     ;=> true   — should be a plain map of the fields
(record-id (merge {:z 1} p))              ;=> :point — a plain map took on p's identity
(record-id (assoc {:x 1} :__id__ :a/b))   ;=> :a/b   — identity forged by hand
```

- **`(into {} r)` returns `r`.** `into`'s "pouring a map into an empty map returns it as-is"
  shortcut (`seq.blsp:279`) runs before any record check on `from`.
- **`(merge plain r)` makes `plain` an `r`.** The record branch is keyed on the *target*
  (`%map-get to :__id__`), so a plain target goes through `%map-into … (%map-pairs r)`, and
  `%map-pairs` includes `:__id__`.
- **`assoc :__id__` forges an identity.** `json.blsp:289-334` has to keep a decoded
  `"__id__"` key as a string, because otherwise JSON input could forge a record.

**Fixed 2026-10-27:**
- `into` reads a record *source* through `seq` (its fields);
- `merge` is seeded with its first map, so `(merge r m)` stays an `r`;
- the `abilities.blsp` docstring is corrected;
- `tests/record_test.blsp` has new cases, one of them across processes.

**Still open: the forge.** `assoc`/`update` with the `:__id__` key would have to raise. That
puts a check on `assoc`, one of the hottest paths, so it needs `make ab`/`make ab-vm` on the
test machine. A `Record` kind (§4) removes the forge by construction, with no check at all, so
the forge waits for that decision rather than paying a per-`assoc` cost now.

## 3. Why it's this way (ADR history)

- **ADR-130** (`adr-100-199.md:1994`): records are "maps, not a new Value kind".
- **ADR-168** (`:4261`, point 2): "ADR-130 survives. A record is still a structural map:
  `type-of` is `:map`, and `get`/`assoc`/`=` are unchanged. The identity is a dispatch-only
  notion … held in a reserved `:__id__` field." It rejected a Record variant because "the
  Value layout is JIT-pinned and map ops match `Value::Map` with catch-alls — pervasive,
  risky".
- **ADR-171** keeps the id in the default printed form on purpose.
- Later ADRs have already counted the cost. The `deftype` ADR (`adr-300-399.md:1962`) calls
  the nominal identity a cost, and `adr-200-299.md:5003` warns that `__id__` "freezes a
  private shape into a public contract".

Since then, `=` has become nominal and the seq view hides the id, so the "structural map"
premise in ADR-168 no longer holds. Records aren't structural maps any more, and the
representation is the one part that still treats them as if they were.

## 4. The options

**A. A `Record` value kind (the target).** `Value::Record { id: Symbol, fields: MapId }`
fits the 24-byte Value inline, the way `Pid { node, id }` already does, so it needs no new
slab. The variant goes at the **end** of the enum to keep the JIT's discriminant order, and
there are 24 of 32 tag slots in use.

- *Gains:*
  - no `:__id__` probes on the hot map paths (a missed `get` on a plain map does a second
    lookup today);
  - the forge and leak bugs can no longer happen at all;
  - `type-of` and ability dispatch become a tag check;
  - the checker gets a real atom instead of encoding nominality as a field.
- *Cost (estimated):* 50–70 files, about 25–30 of them Rust. The closest precedent is
  `Value::Failure` (ADR-310, `0fb46c62`), which touched 44 files, 22 of them Rust. A record
  needs more on top of that, because it must also support the map operations:
  - every `%map-*` primitive (`get`/`assoc`/`dissoc`/`pairs`/`count`/`into`) and `expect_map`
    (`builtins/args.rs:134`);
  - the VM/JIT `MapGet` inline paths and `jit/rt.rs` `map_get` (`:769,808,842`);
  - map patterns and destructuring;
  - GC trace, promote and freeze;
  - equality and hashing;
  - the printer;
  - `to_message` and the wire format, with a version bump;
  - about 20 checker sites, moving from the `__id__` shape to a Record tag with id literals.
- *Risk:* medium-high. The JIT, GC tracing and sealed-ability exhaustiveness are the delicate
  parts.

**B. Keep the map and hide the key in every map operation.** About 15 files, low–medium risk.
It fixes the leaks, but `type-of` stays `:map`, the checker's field encoding stays, and every
new map operation is one more place to forget. The two leaks above show that has already
happened twice.

**Recommendation.** Fix the §2 leaks now (that's B's useful half, and it's needed either
way). Plan A as its own piece of work, under a new ADR that supersedes ADR-130 and ADR-168
point 2, with the full proof on the test machine or in CI: `make test-both`, `make gcstress`,
a fuzz-differential pass, and `make ab`/`make ab-vm` for the `get` path.

## 5. What A would take, in order

1. The ADR, and a decision on `type-of` (`:record`, or the id itself; `:record` keeps
   `type-of`'s answers a closed set) and on the printed form (ADR-171).
2. **Kernel.** The variant, alloc, GC trace/promote/freeze, equality/hash, the printer, the
   codec and its version bump, the map primitives accepting both variants, and the VM/JIT
   inline `get` plus the `rt.rs` callbacks.
3. **Prelude.** `defrecord` constructs through a `%make-record` primitive. `record?`,
   `record-id` and `fields` become primitives. Delete the `:__id__` special cases in `seq`,
   `count`, `keys`, `into`, `conj` and the `Lookup` fallback. Constructor devirtualization
   reads the new form.
4. **Checker.** A Record tag carrying an id literal. Rebuild the sealed-ability type, the
   exhaustiveness checks and the display on it.
5. **Tests.** Delete the tests that pin `:__id__` (`tests/record_test.blsp:21-26,157`,
   `ability_test.blsp:67`, `show_test.blsp:26,38`, and the hand-written `{:__id__ …}`
   literals in `json_test.blsp:373,377` and `multimap_test.blsp:106`). The "does not leak"
   tests stay, and still pass.
6. **Downstream.** bedit has 49 `defrecord`s and 10 direct `(get e :__id__)` reads, all in
   tests; those become `record-id`. Then push and run `make smoke-bedit ARGS=--bump`.
7. **Docs.** `language.md` §Records, `brood-for-claude.md` §Polymorphism, `docs/types.md`
   (its compatibility contract for a new kind, `:665`), and the devlog.
