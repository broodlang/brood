# Packages: third-party Brood deps

> Status: **done** (ADR-037; v1 scope). Design captured here ahead of M2 because
> the decisions (manifest shape, cache layout, conflict policy) cross-cut
> project management and the upcoming editor plugin story. Landed in vertical
> slices — see [`ROADMAP.md`](../ROADMAP.md):
>
> - **Slice 0 — done (2026-05-29):** manifest `:dependencies` parsing; the
>   `(project …)` form is now a *quoting macro* (bare symbols in manifests).
> - **Slice 1 — done (2026-05-29):** `:path` deps end-to-end. A hashing
>   primitive (then `%sha256`, since generalised to `%digest`) + Brood
>   tree-hashing, transitive resolution + conflict detection,
>   `project.lock.blsp` read/write, and `ensure-deps` wired into `project-setup`
>   (a path dep's `src/` joins `*load-path*`, so `(require 'dep)` finds it).
>   `std/tool/package.blsp` is the new module; no git, no network. The `(fetch)` verb
>   exists; its `nest fetch` subcommand wiring lands with the other verbs.
> - **Slice 2 — done (2026-05-30):** `:git` deps end-to-end. The
>   `%git-resolve-ref` (`ls-remote` a tag/branch/commit → SHA), `%git-clone`
>   (init + shallow-fetch the pinned commit + detached checkout), and `%rm-rf`
>   (bounded to `_deps/`) primitives; the `_deps/<name>/` cache with a
>   `.brood-pkg.blsp` metadata stamp; commit reuse from the lock so a re-resolve
>   is network-free on a cache hit; the `:git`/`:commit` lock fields; and the
>   "direct beats transitive" conflict rule.
> - **Slice 3 — done (2026-05-30):** the `nest fetch`/`update`/`add`/`remove`/
>   `tree` subcommands, `add`/`remove` editing the manifest over the
>   comment-preserving CST, and auto-fetch via `ensure-deps` on every
>   project-aware subcommand.
> - **v2 — done (2026-07-24, ADR-147):** **`:tarball` deps** — a `.tar.gz`
>   artifact + a **mandatory `:sha256`**, downloaded (via `std/net`'s byte-faithful
>   `http-get`, or read from a `file://` path), verified, and strip-extracted into
>   `_deps/` by the new `%untar-gz` primitive; and a **git-backed registry** — the
>   index is just a git repo of `packages/<name>.blsp` metadata (no hosted server,
>   keeping ADR-037's "no central infrastructure"). `nest publish` appends the
>   project's version entry; `nest search` greps the index; a `[name :version "^1.2"]`
>   dep names a **semver range**, resolved to a concrete published version by the
>   PubGrub resolver (ADR-209, added after v2). See *The registry (v2)* below.
>
> Still deferred by design (ADR-011): tarball sources *in* registry entries (registry
> entries point to git today) and signed packages. See *Future work* below.
>
> Four decisions refined the original sketch when implementation began — they
> are folded into the relevant sections below and summarised in ADR-037's
> *Implementation refinements*.

Brood's module system (ADR-019) already resolves `(require 'foo)` through
`*load-path*`, with embedded std modules baked into the binary. Packages
fill the missing piece: **where does the source for `foo` come from when
it isn't yours and it isn't stdlib?**

The answer in this design is **Git** — repositories pinned by commit (or
tag) in the project manifest, cached under the project root, with a
lock file for bit-for-bit reproducibility. No registry, no semver solver,
no install scripts. Closest sibling design: Go modules in the pre-MVS era.

## What it looks like

A project that depends on two external packages and one internal sibling:

```lisp
;; project.blsp
(project
  :name    "my-editor"
  :version "0.1.0"
  :main    (main main)
  :dependencies
  [[parser :git "https://github.com/foo/brood-parser.git" :ref "v1.2.0"]
   [pretty :git "https://github.com/bar/brood-pretty.git" :ref "abc1234"]
   [shared :path "../shared"]])
```

`(project …)` is a **macro that treats its arguments as literal data** — it
quotes them and hands them to `project--apply` — so dep names (`parser`,
`pretty`, `shared`) and the `:main` pair are written as **bare symbols**, no
leading `'`. A manifest is pure static data; nothing in it is ever evaluated.

```bash
nest fetch          # download what's missing, write project.lock.blsp
nest test           # auto-runs fetch first
nest add curl :git "https://github.com/baz/brood-curl.git" :ref "v0.3.0"
nest update parser  # re-resolve parser's ref (a moving tag, for example)
nest tree           # print the resolved dep graph
nest remove pretty  # strip from :dependencies and from _deps/
```

After `fetch`, the tree:

```
my-editor/
  project.blsp
  project.lock.blsp        ← committed; pins commit + SHA-256
  .gitignore               ← contains _deps/
  src/
  tests/
  _deps/                   ← gitignored, regenerable from the lock file
    parser/
      .brood-pkg.blsp      ← url, ref, commit, fetched-at, sha256
      project.blsp
      src/
      ...
    pretty/
      ...
```

Inside any project source, `(require 'parser)` resolves through
`*load-path*` exactly as today — the only change is that `_deps/*/src/`
have been added to it.

## Manifest model

The `(project …)` form (`std/tool/project.blsp`) gains an optional
`:dependencies` slot. The value is a vector of **dep entries**. Each entry
is a vector: `[name source-kind source-spec & opts]`.

Four source kinds:

| Kind       | Shape                                    | Notes |
|------------|------------------------------------------|---|
| `:git`     | `[name :git URL :ref REF]`               | `REF` is a tag or commit. Branches are accepted but advisory — `:ref "main"` re-resolves on every `nest update`. |
| `:path`    | `[name :path PATH]`                      | Filesystem path, relative to the manifest. Local dev/mirror; SHA-256'd at fetch time. |
| `:tarball` | `[name :tarball URL :sha256 HEX]`        | A `.tar.gz` artifact (http/https, or `file://` for a local/offline one). `:sha256` is **mandatory** — the integrity pin standing in for git's commit; a mismatch is a loud error. Extracted into `_deps/<name>/`, stripping the single wrapper directory. (v2, ADR-147.) |
| `:version` | `[name :version "^1.2"]`                 | A **registry** dep naming a **semver range** (`^1.2`, `~> 1.3`, `>= 1.2, < 2.0`, `= 1.2.3`, or a bare `1.2.3` = exact). The PubGrub resolver picks the newest published version satisfying it (and every transitive range), downloads + sha-verifies + extracts it, and locks the concrete version. (Ranges: ADR-209; registry: v2, ADR-147.) |

`name` is the **local symbol** the dep will be available as inside
`(require …)`. It need not match the package's own `:name` — the manifest
binds: the user *chooses* the require-name for each dep in their project,
just like Cargo's `[dependencies] foo = { package = "...", … }` rename.
(A future `:rename` opt could make this explicit; for v1 the first slot
*is* the rename.)

Reserved opts for future use (parsed-but-rejected in v1, so the manifest
shape stays forward-compatible):

- `:branch BRANCH` — track a branch (re-resolves on `nest update`).
- `:dir SUBDIR` — the dep's source lives in `SUBDIR/` of the repo, not at the root.
- `:features [a b]` — pass build-feature flags through to the dep.

## Lock file

`project.lock.blsp` is **generated**, **committed**, and **read-only** to
the user. It's plain Brood data — same reader/printer the rest of the
language uses — so a diff in a PR is human-reviewable:

```lisp
;; project.lock.blsp — generated by `nest fetch`. Do not edit by hand.
(lock
  :version 1
  :brood-version "0.1.0"
  :dependencies
  [[parser
    :git    "https://github.com/foo/brood-parser.git"
    :ref    "v1.2.0"
    :commit "abc1234567890abcdef1234567890abcdef123456"
    :sha256 "deadbeefcafe..."
    :deps   []]
   [pretty
    :git    "https://github.com/bar/brood-pretty.git"
    :ref    "abc1234"
    :commit "abc1234567890abcdef..."
    :sha256 "..."
    :deps   [[ansi :git "https://github.com/quux/brood-ansi.git" :ref "v0.1.0"]]]
   [shared
    :path   "../shared"
    :sha256 "..."                         ; tree hash at fetch time
    :deps   []]
   ;; Transitive — depth-first; resolved at root, not nested.
   [ansi
    :git    "https://github.com/quux/brood-ansi.git"
    :ref    "v0.1.0"
    :commit "..."
    :sha256 "..."
    :deps   []]])
```

Two invariants:

1. **Manifest-consistent.** Every direct dep in the manifest appears here,
   with the resolved commit (for `:git`) or with the tree hash recorded at
   fetch time (for `:path`). A manifest edit that changes a `:ref` makes
   the lock file stale; `nest fetch` notices and re-resolves only that dep.
2. **Transitively closed.** Every dep this project transitively uses
   appears at the top level. Nesting is deliberately avoided — flat is
   easier to diff, easier to override, and easier to detect conflicts in.

The `:deps` slot on each row records the dep's own direct dependencies —
purely for traceability (`nest tree` and "why is X here?"). Transitive
resolution is at the root.

> **Slice 1 note.** The current implementation stores `:deps` as a vector of
> the dep's direct-dependency *names* (symbols), not the full sub-entries shown
> above. That's enough to reconstruct the graph against the flat root list; the
> richer sub-entry form lands with `nest tree` (Slice 3). Two other slice-1
> simplifications: a dep's source dir is assumed to be `<dep>/src` (it doesn't
> yet read the dep's own `:source-paths`), and a `:path` dep's `resolved-path`
> is left un-normalised (`app/../greeter` — the OS resolves it; cosmetic).

## Resolution algorithm

```
fn fetch(project_root):
    manifest = read(project_root / "project.blsp")
    lock     = try_read(project_root / "project.lock.blsp") or empty
    resolved = {}                                   # name → resolved entry

    queue = manifest[:dependencies]
    while queue not empty:
        dep = queue.pop_front()
        if dep.name in resolved:
            check_compatible(resolved[dep.name], dep)   # see "conflicts"
            continue
        entry = resolve(dep, lock)                  # see below
        resolved[dep.name] = entry
        queue.extend(read_subdeps(entry))           # depth-first

    write_lockfile(project_root, resolved)
    ensure_cache(project_root, resolved)            # _deps/<name>/

fn resolve(dep, lock):
    if dep.kind == :path:
        absp  = absolute(dep.path)
        hash  = sha256_tree(absp)
        return {…dep, sha256: hash, deps: read_subdeps_of(absp)}

    locked = lock.get(dep.name)
    if locked and locked.git == dep.git and locked.ref == dep.ref:
        return locked                               # already pinned
    commit = git_resolve_ref(dep.git, dep.ref)     # ls-remote
    return {…dep, commit, sha256: TBD, deps: TBD}  # filled by ensure_cache

fn ensure_cache(project_root, resolved):
    for entry in resolved.values():
        target = project_root / "_deps" / entry.name
        if cache_matches(target, entry):            # .brood-pkg.blsp metadata
            continue
        rm -rf target
        git_clone(entry.git, target, entry.ref, entry.commit)  # clone ref, checkout commit
        sha    = sha256_tree(target)
        entry.sha256 = sha
        write_pkg_meta(target / ".brood-pkg.blsp", entry)
```

`read_subdeps` is just "read the dep's `project.blsp`, return its
`:dependencies`". The depth-first walk keeps the topology straightforward
and gives nice trace output for `nest tree`.

> **Implementation note (Slice 2).** The sketch shows `resolve` returning
> `deps: TBD` and a separate `ensure_cache` pass filling them. In the
> implementation the clone is **folded into `resolve`** for `:git` deps — a dep's
> own `project.blsp` only exists on disk *after* the clone, and the walk needs its
> `:dependencies` immediately to queue them. So `package--resolve-git` clones (on
> a cache miss), then reads the dep's manifest for sub-deps in the same step,
> exactly as `:path` resolution already does. A **cache hit** (the
> `.brood-pkg.blsp` records the wanted commit) skips the clone *and* the tree-hash,
> reusing the locked SHA — so `ensure-deps`, which runs on every project-aware
> `nest` subcommand, doesn't re-hash every dep file on each invocation.
>
> "Direct beats transitive" falls out of the queue order: the root manifest's
> deps are enqueued first, so each is resolved before any transitive request for
> the same name. When a duplicate name surfaces and it's a direct dep, the root's
> pin already won (the transitive request is dropped silently); two *transitive*
> deps that disagree is the loud error below.

## Conflicts

If two deps require the same `name` at different refs, that's an **error**.
The message names both pinning sites and tells the user to add an explicit
override in the root manifest:

```
nest fetch: conflicting dependency `ansi`
  required by you at v0.1.0
  required by pretty at v0.2.1
fix: pin `ansi` explicitly in your project.blsp's :dependencies — it wins.
```

The root manifest's direct dep always wins over a transitive dep at a
different ref. This is the **MVS-without-the-solver** rule (Go's approach):
direct beats transitive; nothing else gets clever. For two transitive deps
at different refs without a direct pin, it's an error — the user resolves
it by adding a direct pin to their root manifest.

This is intentionally less powerful than Cargo's `[patch]` or npm's
peer-dep nudging. For a pre-1.0 ecosystem with no registry yet, "you
resolved it by hand once and committed the lock file" is *plenty*.

### Namespace collisions (ADR-070)

A *version* conflict (above) is two requests for the same dependency. A
**namespace collision** is different: two *unrelated* providers that each ship a
module of the same name. Because namespaces aren't package-rooted yet, every
module lands in the one flat global table under its `(defmodule …)` name — so two
providers of `util` would clobber. `require` loads whichever `util.blsp` is first
on `*load-path*`; the other never loads, and any code depending on the loser
silently binds the *wrong* `util`:

```
$ nest run
error: package: module name collision — 'b' is provided by both your project
  and dependency 'foo'; rename one (namespaces aren't package-rooted yet — ADR-070)
```

So the package manager **detects and rejects** it at resolution time
(`fetch`/`add`/the auto-fetch on every subcommand), naming both providers. Note
the providers checked include **your own project's modules**, not just deps — a
dep shadowing a module you wrote is the same bug. A provider's namespaces are read
from each source file's `(defmodule …)` name (the name that actually clobbers),
not the filename.

The fix is to rename one. The structural cure — **package-rooted namespaces**
(`foo/b` instead of `b`, collisions *impossible*) plus author `:exports` and
import aliases `[mod :as m]` — is the recorded future direction (ADR-070 *Future
direction*), deferred until the editor's multi-author plugin ecosystem makes it
pay. It's a loader-level change that won't churn package source, so deferring it
is nearly free.

## `*load-path*` integration

`project-setup` (in `std/tool/project.blsp`) gains an `(ensure-deps)` step that:

1. Reads `project.lock.blsp` (failing if it doesn't exist but `:dependencies`
   does — the user needs to run `nest fetch`).
2. Verifies each `_deps/<name>/` exists and `.brood-pkg.blsp` matches the
   lock; if not, kicks off `fetch` automatically.
3. Extends `*load-path*` with each dep's source dir
   (`_deps/<name>/src/` by default; overridable via the dep's own
   `project.blsp` `:source-paths`).

A **`:path` dep loads *in place*** — its `<path>/src/` is added to
`*load-path*` directly; it is **not** copied into `_deps/`. So `_deps/` only
exists once a git dep is fetched, and edits to a path-dep's source tree are
live (the intended local-dev workflow — see [Hot reload + dev
workflow](#hot-reload--dev-workflow)). The dep is still tree-hashed into the
lock file for change detection.

The existing `(require 'foo)` machinery resolves through the extended
path. No special "package require" surface — packages are just modules on
the load path. (This is the same reason an internal `(require 'main)`
works: `src/` was already on the path.)

## Subcommand surface

Each is a one-liner from the Rust shell into Brood policy:

| Command                                  | Effect |
|------------------------------------------|---|
| `nest fetch`                             | Ensure every dep is present; re-resolve any whose lockfile entry is stale. |
| `nest update`                            | Re-resolve every dep's ref (re-running `ls-remote` for moving refs). |
| `nest update <name>`                     | Same, but only for one dep. |
| `nest add <name> :git URL :ref REF`      | Append to `:dependencies` (preserving the manifest's formatting via the existing `parse-source` / formatter), then `fetch`. |
| `nest add <name> :path PATH`             | Path-dep variant of `add`. |
| `nest add <name> :tarball URL :sha256 HEX` | Tarball-dep variant of `add` (v2). |
| `nest remove <name>`                     | Strip from `:dependencies`, drop `_deps/<name>/`, re-resolve the lock. |
| `nest tree`                              | Print the resolved dep tree (root → direct → transitive). |
| `nest publish [<index>]`                 | Append this project's version entry to the registry index (v2, ADR-147). |
| `nest search <term> [<index>]`           | Search the registry index by name/description (v2, ADR-147). |
| `nest test` / `run` / `check` / `format` / `mcp` | Auto-fetch missing deps on first run (a no-op on the second). |

`nest fetch` is idempotent and side-effect-free when the cache is current.

## The registry (v2, ADR-147)

The registry is deliberately **not** a hosted service — it is **a git repository of
metadata**, keeping ADR-037's "no central infrastructure to host or pay for" while
adding discovery and named/versioned resolution (the crates.io-index / Go-proxy
model). Layout: one file per package, `packages/<name>.blsp`, holding a vector of
published version entries (newest last):

```lisp
[{:version "1.0.0" :git "https://github.com/you/foo" :ref "v1.0.0" :description "…"}
 {:version "1.1.0" :git "https://github.com/you/foo" :ref "v1.1.0" :description "…"}]
```

The index location is the user config's `:registry`
(`~/.config/brood/config.blsp`), default `https://github.com/broodlang/registry`,
overridable per command by passing an index path/URL. A **URL** index is cloned into
`_deps/.registry-<hash>/`; a **local path** index is read (and, for `publish`,
written) in place — the dev / self-hosted / offline path.

- **`nest publish`** reads `:name`/`:version`/`:description`/`:repository` from
  `project.blsp`, appends the entry to `packages/<name>.blsp` in a **local** index
  checkout, and stops there — it does **not** auto-commit (you own the index repo:
  review, commit, `git push`). A version already published is refused; publishing to
  a URL is refused (clone it first).
- **`nest search <term>`** greps every package's name and latest description.
- **A `[name :version "^1.2"]` dep** names a **semver range** (ADR-209). The
  PubGrub resolver asks the registry which versions exist and what each
  release requires, picks the newest that satisfies every constraint across the
  transitive closure, then downloads + sha-verifies + extracts it and locks the
  concrete version. A fully-covering lock is reused network-free; a range nothing
  satisfies is a loud conflict naming the package and every requirer.

To publish, a package sets two manifest fields: `:repository` (its git URL) and
`:description`. The published `:ref` is `v<version>` by convention.

## Concurrent manifest edits are safe

`nest add` / `nest remove` edit `project.blsp` as a read-modify-write: read the
source, splice an entry in or out, write it back. Done naively that **loses an
update** — two processes both read the original, both splice, and the second write
erases the first, while *both* report success. Measured before the fix: three
concurrent `nest add`s landed between one and three of them.

The edit now goes through `package--edit-manifest!`, a **compare-and-swap**: the
write only lands if the file still holds exactly what was read, and when it doesn't,
the edit is *recomputed against the new content* and retried (bounded, 8 attempts,
then a clear error saying nothing was written). Concurrent adds and removes all land,
in some order, and the manifest is always valid.

The CAS is the right shape here specifically because the "modify" step is Brood code
— splicing an entry into source text — and so cannot run inside a locked primitive.
The primitive is `%file-swap`, which supplies the two properties the retry rests on:

- **Serialisation** — a blocking exclusive `flock`, held only for the duration of the
  call, so it cannot leak and the OS releases it if the process dies (no stale-lock
  recovery to get wrong). The lock is a *separate* file, never the manifest: the
  manifest is replaced by `rename`, and a lock on a since-unlinked inode would
  exclude nobody.
- **Crash-atomicity** — the new contents are written to a temp file and `rename`d
  over the manifest, so a crash mid-edit leaves the old file intact. A half-written
  manifest is exactly the "project no longer parses" failure worth avoiding.

The lock file lives with the project's other derived state (its cache dir, keyed by
project root — `/tmp` when there is no `HOME`), **not** in the project tree: nothing
stray appears next to your source, and the lock's inode stays stable across manifest
rewrites.

A failed `add` still rolls its own edit back, and that rollback is a CAS too — it
only reverts if the manifest still holds what the failed command wrote, so it cannot
stomp a concurrent editor. If it can't, it says so rather than overwriting.

Read-only commands were never affected: concurrent `nest test` / `nest check` runs are
safe, including their shared on-disk check cache and `--failed` record.

## Cache layout & gitignore

The cache is **per project** at `_deps/`. It is **not** shared across
projects. Pros: hermetic; reproducible across machines; no race between
parallel `nest fetch` invocations. Cons: more disk. Acceptable for v1.

`_deps/` is `.gitignore`'d. `nest new` adds it to the scaffolded
`.gitignore`. `project.lock.blsp` is **committed** — that's where
reproducibility lives.

Each dep's directory contains a `.brood-pkg.blsp` with:

```lisp
(brood-pkg
  :git    "https://github.com/foo/brood-parser.git"
  :ref    "v1.2.0"
  :commit "abc1234..."
  :sha256 "deadbeef..."
  :fetched-at 1716922800000)   ; ms since epoch — for `nest tree` display
```

This is the cache's source of truth; comparing it to the lock entry tells
`ensure_cache` whether the directory is up-to-date.

## Hot reload + dev workflow

Brood's `def`-based hot reload (ADR-013) is unchanged by packages. Deps
load like any other module; re-`(require)`ing them with `(reload)` (a
forced re-load via `eval-string` of the source) makes a redefinition
visible to running processes. This means **a dep can be hot-edited
in-place** in `_deps/<name>/src/`:

- Useful for "what would happen if I patched this dep?" experimentation.
- Lost on the next `nest fetch` (the cache is reset to the locked tree).
- For sustained local development on a dep, prefer `:path` source — the
  fetcher SHA-256s on each fetch but doesn't re-clone, so edits in the
  path-deps source tree are preserved.

## Trust / security model

**No install scripts.** Packages are pure Brood source. They run only when
`(require)`d, through the same evaluator as user code. There is no
package-defined hook that runs at fetch time, no privileged context
during install. This closes the npm-style supply-chain attack class
**by construction**.

**No native code.** A package can't ship a `cargo` crate that gets compiled
on install. The runtime is a fixed binary; packages are source over it.
If a future package wants native acceleration, the standard
"`cargo`-distributed crate + Brood wrapper" path applies — the native
piece comes from crates.io, the Brood wrapper from a Brood package.
Cleanly separates concerns; users opt into native crates the same way
they would in any Rust project.

**Reproducibility.** SHA-256 in the lock file pins the exact bytes.
Re-running `nest fetch` against the same lock file produces a
byte-identical `_deps/` tree.

**Provenance.** Trust flows from the URL. `nest tree --remotes` (future)
could list each dep's origin URL prominently for review. Signed
packages are deferred; Git commit hashes are pseudo-signatures over the
content (matches Go's stance: if you trust the URL, the lock file pins
the content).

**Eval still runs `require`d code.** A malicious package, once
`(require)`d, can do anything Brood can — `run-process`, `spit`, network
I/O via future primitives. **Don't `(require)` untrusted code**, same as
`import` in Python or `require` in npm. The package manager doesn't (and
shouldn't) sandbox.

## Comparison

Why this shape, in three side-by-sides:

| Concern            | Brood (this design)        | Go modules         | Cargo            | npm                |
|--------------------|----------------------------|--------------------|------------------|--------------------|
| Identity           | Git URL = name             | Git URL = name     | crates.io name   | npm name           |
| Constraint solver  | None                       | MVS (since Go 1.11)| SAT-ish          | SAT solver         |
| Lock file          | `project.lock.blsp` (committed) | `go.sum`      | `Cargo.lock`     | `package-lock.json`|
| Cache              | Project-local              | `$GOPATH/pkg/mod` (global) | `~/.cargo/registry` (global) | `node_modules` (project) |
| Install scripts    | **No**                     | No                 | No (build.rs is sandboxed-ish) | Yes (the disaster) |
| Registry needed    | No                         | No                 | Yes (crates.io)  | Yes (npm)          |

Brood lands closest to Go's pre-MVS era: name = URL, direct refs, lock
file, no registry. Simpler than even Go-today because there's no
constraint solver. The reasonable next stop after Brood is Cargo's level
of sophistication, but that requires a registry and a solver — both
out-of-scope for v1.

## Future work (explicitly deferred)

- **Registry** — a brood-hex or brood-archive equivalent. Adds discovery
  (`nest search`), human-readable names independent of URLs, and curated
  metadata. None of this is needed before there are enough packages to
  curate.
- **Tarball / HTTP source kind** — `[name :tarball URL :sha256 HASH]`.
  The `%http-get` primitive lands now so the Rust kernel doesn't have to
  change later; the source-kind dispatch is gated until a real use case.
- **Semver + constraint solver** — ✅ shipped for registry deps (ADR-209):
  `[name :version "^1.2"]` names a range, resolved to a concrete published
  version by a PubGrub (CDCL) newest-compatible solver (`std/resolver.blsp`)
  that prefers the locked versions (adding one dep keeps the rest pinned). A
  `:brood "<constraint>"` gate refuses an incompatible runtime. Still open:
  semver ranges over `:git` tags (registry-only for now), and PubGrub-grade
  conflict-error derivation.
- **Signed packages** — a `:sig` opt with a Brood-flavoured key registry
  (akin to Maven's PGP or sigstore). Needs trust infrastructure that
  isn't this project's problem until packages are exchanged at scale.
- **Per-dep build / load-path overrides** — Cargo's `[patch]` /
  `[replace]` shape. Solved for now by `:path` sources.
- **MCP `packages.list` tool surface** — exposes the resolved dep tree to
  agents. Drops in cleanly once `std/tool/package.blsp` is in.

## Implementation sketch (when it lands)

**Rust primitives** (`crates/lisp/src/builtins/io.rs`):

- `(%git-clone url dest ref commit)` — shell out to `git`: clone the ref
  shallowly into `dest`, then **check out the exact `commit`**. (A plain
  `clone --depth 1 --branch <ref>` only accepts a branch/tag name, but the
  lock file always pins a commit SHA — so cloning a pinned dep needs the
  clone-then-checkout shape, fetching the commit where the server allows it.)
  Returns `:ok` or throws.
- `(%git-resolve-ref url ref)` — `git ls-remote URL REF` → commit hash
  string, or nil if not found.
- `(%digest algo bytes)` — hash a byte sequence → a bytes digest. The **only**
  hashing primitive (with `%hmac`); `std/hash.blsp` is Brood over it, exposing
  `hash/sha256` (hex over a string) and `hash/sha256-bytes`. Per-file hashing is
  `(hash/sha256-bytes (slurp-bytes path))` — byte-level, so a binary asset hashes
  correctly — and the canonical directory hash is a Brood tree-walk combining
  per-file hashes (see [Reproducibility notes](#reproducibility-notes) below); both
  live in `std/tool/package.blsp`, not the kernel. Also hashes the lock manifest.
- `(%http-get url)` — GET → bytes. **Deferred** with the `:tarball` source kind
  (ADR-011): it has no caller until then, so it isn't added yet. When a tarball
  dep lands, the kernel gains this one primitive and the source-kind dispatch in
  `std/tool/package.blsp` opens up — no other reshaping.
- `(%rm-rf path)` — explicit because `nest update` overwrites cached deps.
  Bounded to paths under `_deps/`; refuses anything outside.

**Brood policy** (`std/tool/package.blsp`, new module):

- `(read-lockfile root)` / `(write-lockfile root entries)`.
- `(resolve-deps manifest lock)` — the walk in [Resolution
  algorithm](#resolution-algorithm).
- `(ensure-cache root entries)` — the cache check + clone.
- `(ensure-deps)` — called from `(project-setup)`; the auto-fetch on
  every `nest` subcommand.
- The CLI verbs: `(fetch)` / `(update & opts)` / `(add name & opts)` /
  `(remove name)` / `(tree)`.

**Manifest extension** (`std/tool/project.blsp`):

- `(project …)` recognises `:dependencies`. Stored in
  `*project-dependencies*`. Empty when omitted (back-compat with v1
  manifests).

**`nest`'s Rust shell** (`crates/nest/src/main.rs`):

- New subcommand arms: `fetch`, `update`, `add`, `remove`, `tree`. Each
  dispatches into `(require 'package) (<verb> …)`.

### Reproducibility notes

The directory content-hash is **Brood** over the single `%digest` primitive (via
`std/hash.blsp`), not a directory-walking Rust primitive. It needs a canonical
representation: walk paths in sorted order, and for each file emit its relative
path, a NUL, and that file's hash; then hash the concatenation of those lines.
Approximates `git archive | sha256sum` but doesn't depend on git's behaviour.
Skips `_deps/` (a dep's nested `_deps/` is its own concern, not part of this
dep's content hash) and `.git/`.

As implemented in `std/tool/package.blsp`:

```lisp
(defn package--sha256-file (path) (hash/sha256-bytes (slurp-bytes path)))

(defn package-tree-hash (dir)
  (hash/sha256 (join ""
                 (map (fn (rel) (str rel "\0" (package--sha256-file (path-join dir rel)) "\n"))
                   (package--tree-files dir)))))
```

Per-file hashing reads **bytes** (`slurp-bytes` + `hash/sha256-bytes`), so a dep
containing a binary asset (image, font, …) hashes correctly — the earlier
`slurp`-as-string form threw on any non-UTF-8 file. For a text file the hash is
identical (its UTF-8 bytes *are* the file bytes), so existing lock hashes did not
churn when this changed.

## See also

- ADR-019 — Modules (the `(require)` resolver this package layer sits on)
- ADR-020 — Project model + test runner (`project.blsp`, `nest`)
- ADR-028 — The `brood`/`nest` split
- ADR-006 — Write the language in the language (why this is Brood policy)
- ADR-037 — This design's accept-the-decision record
