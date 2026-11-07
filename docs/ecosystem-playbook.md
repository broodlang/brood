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

The tools are all Brood: the `nest` subcommands `nest rename`, `nest ws`, and
`nest publish --bump`, plus `hive/bin/deploy.blsp`.

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
- Record the decision (an ADR in the newest `docs/decisions/adr-*.md`, indexed in
  `docs/decisions.md`) and a `docs/devlog.md` line. A renamed public name goes in the
  rename ledger too (`crates/lisp/src/renames.rs`).

**Verify (tests never run on the development desktop, see `CLAUDE.md`; run these on the
test machine):**

```
cargo build -p cli
nest check                              # zero warnings
cargo run -p cli -- --test tests/<the affected>_test.blsp
cargo check -p cli --no-default-features --features brood/jit   # the LEAN gate (see below)
```

The full suite and the cross-language benchmarks run on the test machine / in CI.

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
only, so `map/get` never corrupts `multimap/get`. Run it per repo.

Splitting a module is `nest move FROM.blsp TO.blsp NAME…`: each definition leaves with its `sig`,
the comments above it and any `check-allow`, lands at the end of TO (made, `:use`-ing what FROM
does, when missing), and every reference follows it — `from/NAME` in code and in quoted data (a
keymap's `'from/cmd-x`), and a bare NAME in a file that `:use`s FROM. The moved code reaches what
it left behind as `from/HELPER`; the tool says so, and warns when the two modules would then
reach each other (move those helpers too). The flags of `nest rename` compose
(`--refs-only --in-quote`), and neither command rewrites a nested checkout (an agent's worktree).

Then drive every repo at once with the **workspace runner** (auto-discovers the sibling
Brood repos):

```
nest ws status                 # who is dirty / unpushed
nest ws check                  # nest check each project (needs the NEW brood/nest installed)
nest ws commit "chore: adopt <change> (brood vX.Y.Z)"
nest ws push
```

Install the new toolchain first so `nest ws check` checks against it:

```
cd brood && make install          # builds + installs brood + nest (lean runtime + dev-tools)
```

A rename wave ends in `bedit`: CI's `downstream-bedit` job builds it at `BEDIT_REF`. Fix and
push bedit, then `make smoke-bedit ARGS=--bump` in `brood` moves the pin to bedit's pushed HEAD;
commit the pin with the change.

---

## 3. Republish the registry packages

Some siblings are **published registry packages** (`hatch`, `store`, `store-postgres`,
`s3`). Pushing their git repos is not enough — a consumer that resolves them by
`:version` gets the registry copy. Republish, **in dependency order** (a package before
anything that depends on it), with the safe release helper:

```
cd ../store && nest publish --bump patch            # if it changed
cd ../hatch && nest publish --bump patch            # depends on store
cd ../store-postgres && nest publish --bump patch   # depends on store
cd ../s3 && nest publish --bump patch
```

`nest publish --bump` bumps only the project's **own** `:version` (never a dependency pin —
the mistake that once shipped a `store 0.2.6` dep that did not exist), commits, and
`nest publish`es; it does not push, so `nest ws push` afterwards. Releases are immutable, so a version already published is refused —
bump again if needed.

Then update any consumer that pins a bumped package (`:version` / `:ref`) and re-lock
(`nest fetch`).

---

## 4. Deploy the registry (`hive`)

`hive` is the registry **server**, deployed to fly.io (app `brood`, `brood.fly.dev`). Its
Docker image builds brood from a pinned `BROOD_REF`, so it must be moved to the brood
commit that carries the change — otherwise the new hive code builds on old brood and
breaks. The deploy script (pure Brood over `os/run-process`) does the bump + deploy + health check:

```
cd hive && brood bin/deploy.blsp     # pins BROOD_REF to ../brood HEAD, fly deploy, checks /health
```

hive pins its own deps (`hatch`, `store-postgres`, …) by **git commit**, not registry
version (ADR-238, "hive never uses hive") — so bump those `:ref`s in `hive/project.blsp`
and `nest fetch` before deploying if their code changed.

The changelog page (`/changelog`) renders from the `CHANGELOG.md` baked into the image at
`BROOD_REF`, so it updates automatically with the deploy — nothing to hand-mirror.

---

## Checklist

1. `brood`: implement (name constants for primitives) · bump version · changelog · ADR/devlog · rename ledger · `nest check` · lean gate · commit + push.
2. `make install` the new toolchain.
3. `nest rename` the siblings · `nest ws check` · `nest ws commit` · `nest ws push` · `make smoke-bedit ARGS=--bump`.
4. `nest publish --bump` each changed registry package in dep order · update + re-lock consumers.
5. `hive`: bump dep `:ref`s + `nest fetch` · `brood bin/deploy.blsp`.
