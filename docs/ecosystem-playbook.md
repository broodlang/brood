# Ecosystem change playbook

How to make a change to the Brood language and roll it across the whole ecosystem —
the sibling repos (`hatch`, `hive`, `bedit`, `pong`, …), the registry packages, and the
live registry. This is the single source of truth for the process; follow it top to
bottom.

The repos live side by side under one parent directory:

```
broodlang/
  brood/            the language + toolchain (this repo — the hub)
  hatch/            web framework      (registry package)
  store/ s3/        storage + adaptors (registry packages)
  store-postgres/
  hive/             the package registry itself (deployed to fly.io)
  bedit/ pong/ …    apps that consume the language
```

The tools below live in `brood/scripts/ecosystem/` (put it on your `PATH`) and
`hive/bin/`.

---

## 1. Change the language (in `brood/`)

- Implement the change. **For a new or renamed kernel primitive, define its name once**
  as a `kw::` constant in `crates/lisp/src/core/keywords.rs` and reference the constant
  everywhere it is dispatched on — the PrimOp tables (`eval/compile/ir.rs`), the linmap
  map→table lowering (`eval/compile/inline.rs` + `macros.rs`, **both** sides, as coupled
  constant pairs), the JIT (`jit/rt.rs`), the checker, and error messages. A primitive
  name written as a bare string literal in more than one place is the trap: a missed one
  **silently disables an optimizer** with no error.
- **Bump the version and changelog it.** Edit `Cargo.toml` `version`, add a `## vX.Y.Z —
  DATE` section to `CHANGELOG.md`. A **breaking** change (a rename, a removed name) is a
  minor bump pre-1.0 (e.g. 0.5 → 0.6) so a consumer's `:brood ">= X"` can gate it.
- Record the decision (`docs/decisions.md` ADR) and a `docs/devlog.md` line.

**Verify (this machine runs only quick/targeted checks — never the full suite here):**

```
cargo build -p cli
nest check                              # zero warnings
cargo run -p cli -- --test tests/<the affected>_test.blsp
cargo check -p cli --no-default-features --features brood/jit   # the LEAN gate (see below)
```

The full suite and the cross-language benchmarks run on the other machine / in CI.

> **The lean gate matters.** `dev-tools` is a brood *default* feature, and a
> `cargo check --workspace` unifies features across members, so it compiles brood *with*
> dev-tools and misses a `#[cfg(feature = "dev-tools")]` slip (an ungated fn calling a
> gated one). The isolated `-p cli --no-default-features` check is what `make install`
> and hive's Docker build actually compile — CI now runs it, and so should you before a
> deploy.

Commit and push `brood`.

---

## 2. Roll the change across the sibling repos

If the change renames names consumers use, apply the same rename everywhere with the
**identifier-aware codemod** — never a plain `sed`, which corrupts substrings
(`map/get` sits inside `multimap/get`):

```
cd ../hatch && nest rename to-str ->string       # one rename, across this repo's sources
```

`nest rename` is pure Brood (`std/tool/codemod.blsp`) — it rewrites whole identifiers
only, so `map/get` never corrupts `multimap/get`. Run it per repo (or via `ws exec`).

Then drive every repo at once with the **workspace runner** (auto-discovers the sibling
Brood repos):

```
ws status                 # who is dirty / unpushed
ws check                  # nest check each project (needs the NEW brood/nest installed)
ws commit "chore: adopt <change> (brood vX.Y.Z)"
ws push
```

Install the new toolchain first so `ws check` checks against it:

```
cd brood && make install          # builds + installs brood + nest (lean runtime + dev-tools)
```

---

## 3. Republish the registry packages

Some siblings are **published registry packages** (`hatch`, `store`, `store-postgres`,
`s3`). Pushing their git repos is not enough — a consumer that resolves them by
`:version` gets the registry copy. Republish, **in dependency order** (a package before
anything that depends on it), with the safe release helper:

```
release-package ../store            # if it changed
release-package ../hatch            # depends on store
release-package ../store-postgres   # depends on store
release-package ../s3
```

`release-package` bumps only the project's **own** `:version` (never a dependency pin —
the mistake that once shipped a `store 0.2.6` dep that did not exist), commits, pushes,
and `nest publish`es. Releases are immutable, so a version already published is refused —
bump again if needed.

Then update any consumer that pins a bumped package (`:version` / `:ref`) and re-lock
(`nest fetch`).

---

## 4. Deploy the registry (`hive`)

`hive` is the registry **server**, deployed to fly.io (app `brood`, `brood.fly.dev`). Its
Docker image builds brood from a pinned `BROOD_REF`, so it must be moved to the brood
commit that carries the change — otherwise the new hive code builds on old brood and
breaks. The deploy script does the bump + deploy + health check:

```
cd hive && bin/deploy            # pins BROOD_REF to ../brood HEAD, fly deploy, checks /health
# or pin an explicit commit:  bin/deploy <brood-sha>
```

hive pins its own deps (`hatch`, `store-postgres`, …) by **git commit**, not registry
version (ADR-238, "hive never uses hive") — so bump those `:ref`s in `hive/project.blsp`
and `nest fetch` before deploying if their code changed.

The changelog page (`/changelog`) renders from the `CHANGELOG.md` baked into the image at
`BROOD_REF`, so it updates automatically with the deploy — nothing to hand-mirror.

---

## Checklist

1. `brood`: implement (name constants for primitives) · bump version · changelog · ADR/devlog · `nest check` · lean gate · commit + push.
2. `make install` the new toolchain.
3. `nest rename` the siblings · `ws check` · `ws commit` · `ws push`.
4. `release-package` each changed registry package in dep order · update + re-lock consumers.
5. `hive`: bump dep `:ref`s + `nest fetch` · `bin/deploy`.
