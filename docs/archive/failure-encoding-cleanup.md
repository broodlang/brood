# One encoding for expected failure — finishing what ADR-310 started

**Status: DONE (2026-11-06).** Scoped and mostly landed 2026-10-27. "What it would take" steps 1–3
(the docs, bedit's tutor lesson, and `supervisor/start-child`/`restart-child` and
`stream/next`, which now return a `failure`) landed then, step 4 on 2026-11-06 (ADR-617:
`failure/with` + `fields`; `http/request`, `fetch`, `post`, `request-on` and
`registry-get-json` answer with a failure — the table's claim that `request-on` already did
was wrong, it answered `:status 0` too), and step 5 the same day: the vector and range walks
stop on a failed accumulator, and so do the counted loops `range_fold_rewrite` writes for a
`fold` with a literal `fn` (the range loop had no guard either). The text below is the
scoping as written.

## The finding

Brood documents two conventions for an expected failure, and they contradict each other:

- **ADR-163** (`docs/decisions/adr-100-199.md:3899-3906`): "throw for bugs, return
  `[:ok v]` / `[:error e]` for expected alternatives". It is never marked superseded.
  `docs/brood-for-claude.md:264-267` still teaches it, and that file is baked into every
  `nest new` project, so every new project starts with the wrong rule.
- **ADR-310** (`adr-300-399.md:458`): a known failure is a returned **`failure`**, its own
  `Value` kind. It weighed tagged tuples and rejected them because they are "structurally at
  war with `->`" (`:478-481`). `docs/language.md` §Errors, ROADMAP and the `writing-brood`
  skill agree with it. `brood-for-claude.md:1132` agrees too, which contradicts the same
  file's line 264.

The pieces around `failure` are already in place:
- ADR-312 added `ok->`;
- ADR-315 added `with` (the failure `let`) and a fold that stops on a failed accumulator, and
  deleted `some->`;
- ADR-316 made the checker report a failure reaching a parameter that can't take one.

`resolver/resolve` and `tempo/new`/`parse`/`parse-span` were converted in `b6611b4b`.

## What's left in std (verified)

Most of the ~100 `[:ok`/`[:error` sites in `std/` are **process protocol**: `gen` replies,
`try` → `[:ok v]`/`[:raised e]` sent between processes, crash reports, kernel exit reasons.
Those are message data and **stay** (`docs/general-purpose-gaps.md:236-258` says the same).

| Function | Today | Change |
|---|---|---|
| `supervisor/start-child`, `supervisor/restart-child` (`proc/supervisor.blsp:561,579`) | pid \| `:not-found` \| `[:error msg]` when the `:start` thunk raised | the `[:error msg]` arm becomes a `failure` |
| `stream/next` (`stream.blsp:50`) | `[:ok v]` \| `:done` \| `[:err msg]` | only `[:err msg]` becomes a `failure`. `[:ok v]` stays, because `v` may be `nil` or `:done` |
| `http-collect` (private) → `http/request` | `{:status 0 :error …}`, a **third** convention, while `request-on` already returns a `failure` | `http/request` returns a `failure` |
| `registry-get-json` and callers in `package.blsp` (24 sites, all `defn-`) | `[:error status msg]` | `failure` with a status payload (needs gap 1) |

Leave alone:
- `treesit/parse-state`'s `[:incomplete r]`/`[:err m]`, a verdict about the user's text (data);
- `mcp.blsp`'s `[status payload]` pairs, which build a JSON-RPC response;
- `resolver.blsp`'s private search states.

`proc/names.blsp:67` already sends a `failure` inside a `gen` reply, so it can be done.

**Callers to update.** `start-child`/`restart-child` have 12 call sites, and only one
inspects the error (`tests/supervisor_test.blsp:420`). `stream/next` has 2 std matchers
(`stream.blsp:484,496`) and about 8 asserts in `tests/stream_test.blsp`. bedit uses none of
these tuple results.

## The gaps in `failure`

1. **No payload.** `Value::Failure(MapId)` is a CHAMP map with its own tag
   (`core/value.rs:801,943`). Storage, printing, equality (`core/heap/equality.rs:804,1049`),
   messaging (`process/message.rs:958`) and the wire format (`dist/wire.rs:638,864`) all handle
   any number of fields. But `%failure` takes one string (`builtins/errors.rs:151`), and the
   only reader is `%failure-message` behind `error-message`. A caller can't branch on *what
   kind* of failure it got (an HTTP 404 vs 500) without parsing the message. The fix is small
   and kernel-side: `(failure msg & parts)` keeps its shape, plus a way to attach a map of
   fields and read one back (for example `(failure-data f)`).
2. **The fold short-circuit covers lists only.** `%fold-loop` stops on a failed accumulator,
   but the vector and range paths keep running. They still return the failure, so the result
   is right; it just does wasted work, and side effects in the step function still happen.
3. **`docs/types.md` doesn't document `failure`.** It's mentioned three times in passing. The
   checker already models it as a tag (`types/check/annot.rs:589`), narrows on `failure?`
   (`walk.rs:1241`), and eight std sigs declare `(or T failure)`.

No threading form is needed: `ok->` and `with` already exist. No `ok?`/`unwrap`/`result`:
ADR-310 and ADR-312 rejected each by name, because they defeat checker narrowing.

## What it would take

1. **Docs, the same day.** Mark ADR-163's failure clause *superseded by ADR-310* (in the ADR
   and in its `docs/decisions.md:214` index line). Rewrite `brood-for-claude.md:264-267` to
   match §Errors. Fix ADR-310 clause 6 ("`keep` drops failures"): the implementation
   (`std/seq.blsp:647`) and `language.md:1683` say only `nil` is dropped, so record the
   reversal. Add a `failure` section to `docs/types.md`.
2. **bedit's tutor.** `src/tutor-lessons.blsp:510-512` teaches ADR-163's rule. Fix it, push,
   then `make smoke-bedit ARGS=--bump`.
3. **Convert** `supervisor/start-child`/`restart-child` and the `[:err]` arm of `stream/next`,
   and update their tests. Use `nest check` to find any remaining callers that match on
   tuples.
4. **Payload (gap 1)**, then `http/request` and `registry-get-json` on top of it.
5. **Optional.** Have the vector and range fold paths stop on a failure too (gap 2).

Steps 1–3 are Brood and docs. Step 4 adds one kernel accessor. None of it touches the GC,
JIT or scheduler, so CI's ordinary test run is enough; there's no stress or perf gate.
