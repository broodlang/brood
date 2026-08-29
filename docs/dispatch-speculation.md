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

### Phase 0 — the identity guard (enabling; no win on its own)

Emit, in lowered code: read the value's dispatch identity, compare against a constant keyword,
deopt on mismatch. Two shapes, because `%identity-of` has two cases — a record-shaped map
answers with its `:__id__` field, everything else with its `type-of` keyword.

*Gate:* extend `crates/cli/tests/mono_differential.rs`; sabotage the guard and confirm it
fails. *Risk:* low — this is `as_f64` one level up, and it changes nothing until Phase 2 uses
it.

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

`0 → 1 → 2 → 3(a) → 3(b) → 3(c)`.

Phases 0–2 are the mechanism and deliver the win. **3(a) and 3(b) may capture most of the
static value without building a channel at all** — which is the recommendation: build the
channel last, if the profile turns out to leave something on the table, rather than first
because the data happens to exist.
