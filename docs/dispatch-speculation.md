# Speculative ability dispatch — scope

> **Status:** scoped, not started (2026-08-29). Supersedes the Tier-2 half of
> [ability-monomorphization.md](ability-monomorphization.md), which framed the problem as
> "build a checker→compiler channel". The channel is a *data source*, not the mechanism, and
> building it first is what made Tier 2 look large and dangerous.

## The principle this rests on

**A hint is unsound and free. A guard is sound and cheap. A hint must never decide
correctness.**

This is already the law in this runtime, one level down. Float-global unboxing observes at
tier time that a global held a `Float` and lowers the unboxed path — and the comment on
`unbox_float_global` (`eval/compile/jit_lower.rs`) states the rule exactly:

> Soundness is `as_f64`'s existing tag guard, **not** the tier-time observation: a global that
> is no longer a float … fails the guard and deopts to the VM. A stale guess costs a deopt; it
> can never miscompile.

Everything below is that same argument applied to a nominal identity instead of a `Tag`.

## Why this reframes the checker→compiler question

ADR-182's Tier 1 used a static proof **instead of** a guard: it baked the resolved impl into
the chunk, so being wrong was a wrong answer. That is what made it miscompile (ADR-294), and
it is why "get the checker's inferences to the compiler" read as a large, risky build — the
data would have been load-bearing for correctness.

Behind a guard the calculus inverts. A checker fact becomes one more *prior*, indistinguishable
in kind from a runtime observation: if it is right the site speculates immediately, if it is
wrong or stale it deopts once. **So the guard is the thing to build first, and every data
source after it is cheap and safe to add.** The two designs compose; they were never rivals.

## What already exists

| Piece | Where | Note |
|---|---|---|
| Per-call-site ids | `Node::Call { site: u32 }`, `Heap::vm_call_ics` | ADR-096; sites are already the unit of per-site memo |
| Site → source position | `CompiledArm::site_pos` | **Not** debug-gated — available in release. The join key any position-keyed table needs |
| Ability dispatch cache | `DispatchIcEntry`, `Heap::vm_dispatch` | ADR-172. 4-way, epoch-validated — but keyed **per op**, not per site |
| Tier-time type profile | slot-tag profile, `jit_runtime.rs` | One `Tag` per frame slot; the precedent for "observe, then lower optimistically" |
| Deopt | native outcome code, handled by `vm_run_bc`; `BAILED` after 16 | The fallback a failed guard needs |
| Callee inlining | `LeafInline` (`ir.rs`), ADR-210 | Epoch-validated derivation — precedent for "speculative derivation, invalidated by epoch" |

## What is missing

1. **An identity guard** in lowered code — "load the identity, compare to a constant, else
   deopt". Nothing today guards on a nominal id.
2. **Per-call-site identity observation.** The dispatch IC tells you an *op* is monomorphic
   across the whole program; speculation needs to know a *site* is.
3. **A way to feed static facts in as priors** — the channel, but now optional and unable to
   affect correctness.

## Phases

Each is independently shippable, and the order is chosen so that nothing is built on an
unguarded base.

### Phase 0 — one definition of dispatch identity ✅ (2026-08-29)

**Revised on contact, and the revision is the finding.** The phase was written as "emit the
guard in Cranelift". Two facts moved it:

1. **A native guard cannot be built yet.** `jit/rt.rs` has no map-read callback — `brood_rt_*`
   can read globals, the epoch, pairs and tables, but nothing reads a CHAMP field. A record's
   identity *is* a CHAMP field, so the guard needs a new rt callback before it needs any
   Cranelift.
2. **A guard is only as sound as the identity it compares.** And there was no shared
   definition to compare against: `%identity-of` is written in Brood, the compiler's
   `mono_arg_identity` **re-derived it by hand** under a comment reading *"mirrors
   `identity-of`"*, and the checker reasons about `:__id__` shapes separately beside them.
   Three expressions of one rule, none checked against another, with native code about to
   become a fourth. If a guard's notion of identity diverges from the dispatcher's, the guard
   *passes* and the wrong impl runs — silently.

So Phase 0 became the foundation the guard rests on rather than the guard itself:

- **`Heap::dispatch_identity`** (`core/heap/vm_cache.rs`, beside `vm_dispatch`) — the kernel's
  one definition, mirroring `%identity-of` including the case a naive `map_get(:__id__)` gets
  wrong: a map whose `:__id__` is `nil`/`false` is a plain map, not a record.
- **`kw::RECORD_ID`** — the `__id__` spelling now lives in `core/keywords.rs` with the other
  spellings that several layers independently recognise, so a fourth reader cannot invent a
  fifth spelling.
- **The compiler defers to it** instead of re-deriving (behaviour-identical: for a non-map the
  old expression was exactly this one).

*Gates, both sabotage-verified:*
`crates/lisp/tests/dispatch_identity_agrees.rs` asserts the kernel's answer equals the
language's across records, plain maps, falsy-`:__id__` maps and every non-map kind — dropping
the truthy check makes it fail naming both cases. And in `tests/ability_test.blsp`, two tests
pin the mapping `mono_arg_identity` actually depends on and which was previously asserted only
by comment: a record's identity round-trips to its own constructor
(`(reflect/eval (symbol (->string id)))`), and every registered record id names a bound
constructor — with a non-vacuity assertion on the registry size.

*Still to do for the guard proper, in Phase 2 where it has a consumer:* the rt callback and
the Cranelift emission. Writing them now would be dead code, and the epoch question below
decides their shape.

**The epoch finding, which rules out the simpler design.** An IR-level guard —
`(if (= (%identity-of x) :circle) (<impl> …) (<dynamic> …))` — is tempting: it needs no
Cranelift, works at every tier, and is sound as a conditional. It does not work, because the
identity is not the only thing that can go stale. Baking the impl also requires the resolution
to still be current, and `global_epoch` bumps on **every** `def`.

The difference is what each tier does when the epoch moves. Baked into **bytecode**, a
constant has no invalidation at all: the guard would have to compare a baked epoch at every
call and would fail permanently after the first unrelated `def`, leaving the site slow
forever. In the **JIT**, an epoch bump *invalidates the arm* (`jit_runtime.rs`: "a `def` that
rebound the name … bumps the epoch and invalidates the arm"), so the arm drops to the VM and
tiers again at the new epoch, re-resolving as it goes. Nothing stale survives and nothing is
permanently lost.

(Not to be confused with `LeafInline::epoch`, which is a *derivation* stamp: that derivation
is made once at arm-compile time — the only moment a `&Heap` can resolve the callee symbols —
and `jit_lower_inlined_arm` simply **refuses to lower** at any other epoch. It does not
re-derive. The invalidation above is what makes the whole scheme correct; the derivation stamp
is what keeps a stale splice from being lowered in the first place.)

**So the inlining prize is inherently a JIT-tier optimization**, and Phase 2 must be native.

### Phase 2a — the native guard, on the ids we already prove (do this next)

**The guard does not have to wait for profiling.** Phase 1 was sequenced before Phase 2 on the
assumption that speculation needs a profile to supply a candidate identity. It does not need
one *yet*: the syntactically-proven sites already carry a constant id today. Since ADR-294
they lower to `((%dispatch *impls* '[ability op] :id) args…)`, where `:id` is a compile-time
constant — the JIT can recognise that shape, resolve the impl at lower time (the epoch is
current then), and emit

```
guard: dispatch_identity(a0) == :id ?  direct call to the impl  :  deopt
```

which is a **known callee**, hence inlinable — the actual prize — with no profiling
infrastructure at all. It also gives the guard a real consumer, which is what makes it
testable; built in isolation it would be dead code.

What it needs: an rt callback over `Heap::dispatch_identity` (trivial now that one definition
exists — Phase 0), the Cranelift emission, and recognition of the constant-id dispatch shape
in lowering.

Phase 1 then stops being a prerequisite and becomes what it should be: **widening the source
of candidate ids** from "what syntax proves" to "what this site actually sees", which is where
`(map area shapes)` lives.

### Phase 1 — per-call-site identity profiling

Observe the identity actually seen at each ability-op call site and record it per site. Open
design question worth settling first: extend the existing per-site IC block, or a parallel
table. At tier time the question asked is "is this site monomorphic, and on what?"

*Gate:* a monomorphic site reads monomorphic; a polymorphic one does not.
*Risk:* medium — memory per site, and a megamorphic site must degrade rather than thrash. The
dispatch IC's round-robin eviction is the precedent for the latter.

### Phase 2 — speculate and inline (this is the win)

Guard on the profiled identity, then call the impl directly, and let the existing leaf inliner
splice its body. **This is the only phase that can move anything**, and the reason is not the
skipped `identity-of` — it is that a *known callee can be inlined* and a dynamic dispatch never
can.

*Gate:* the differential with the flag on, plus a deopt-rate check (`BROOD_DEOPT_TRACE`) so a
site that speculates wrong repeatedly is visible rather than silently slow.
*Risk:* medium-high. This is where a wrong guard becomes a wrong answer, so Phase 0's gate has
to be genuinely adversarial before this lands.

### Phase 3 — static facts as priors (the enrichment)

Three sources, weakest to strongest — and note the first two need **no channel at all**:

**(a) A sealed ability with exactly one member is a *proof*, not a hint.** `%sealed-members`
is already in the registry and already read by the checker (`sealed_members_ty`). Nothing else
can ever dispatch there. Free, and it needs no inference.

**(b) A declared `sig` on the enclosing function types its parameters directly.** `sig`s are
registered on the heap (`%register-sig`) and the compiler already reads heap registries
(`*impls*`, `*record-ids*`, `*op-ability*`). Reading a declared parameter type is a map lookup
— no `Ctx` walk, no file pass. With adoption now at 407 declarations this is the broadest
cheap source, and it grows every time someone writes a `sig`.

**(c) Full inferred types** — the actual channel. This is the expensive one, and the API shape
matters: `check::expr_ty_of` takes a *closed* expression with a `Ctx::default()`, so it returns
nothing useful for the variable case that Tier 2 exists for; the variable's type lives in the
enclosing `Ctx`. The usable entry point is file-scoped (`check_file`), which is far too heavy
to run during load. So (c) means **a side table**: `nest check` writes proven identities keyed
by source position, riding the existing result cache (ADR-129), and the loader joins them to
sites through `CompiledArm::site_pos`.

**Where (c) uniquely pays: cold start.** A profile has to warm up. A short-lived run — a
`nest check`, a one-shot script, a freshly spawned request handler — never gets there, and
`CLAUDE.md` already insists this arm is real work rather than the unwarmed case to be
discarded. A static prior lets a **cold** site speculate on its first call. That is the one
thing profiling structurally cannot do, and it is the strongest argument for the channel.

## Non-goals and traps

- **Never let a hint decide correctness.** If a phase can produce a wrong answer when the hint
  is wrong, it is mis-designed — go back to Phase 0.
- **Site ids are per-compilation**, so anything persisted must key on source position and join
  through `site_pos`, never on a raw site id.
- **Do not persist a table across a `std/` change.** The stdlib-image id trap (ADR-281) is the
  same failure: a stale artifact silently describing different source.
- **Both arms, short and long.** We do not benchmark in this repo, so the gates here are
  correctness (the differential) and deopt rate — not a number.

## Sequencing

`0 → 2a → 1 → 2 → 3(a) → 3(b) → 3(c)` (revised 2026-08-29 — 2a needs no profile, and gives
the guard the consumer it needs to be testable).

Phases 0–2 are the mechanism and deliver the win. **3(a) and 3(b) may capture most of the
static value without building a channel at all** — which is the recommendation: build the
channel last, if the profile turns out to leave something on the table, rather than first
because the data happens to exist.
