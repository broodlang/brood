# Packages: third-party Brood deps

> Status: **in progress** (ADR-037). Design captured here ahead of M2 because
> the decisions (manifest shape, cache layout, conflict policy) cross-cut
> project management and the upcoming editor plugin story. Landing in vertical
> slices — see [`roadmap.md`](roadmap.md):
>
> - **Slice 0 — done (2026-05-29):** manifest `:dependencies` parsing; the
>   `(project …)` form is now a *quoting macro* (bare symbols in manifests).
> - **Slice 1 — done (2026-05-29):** `:path` deps end-to-end. The `%sha256`
>   primitive + Brood tree-hashing, transitive resolution + conflict detection,
>   `project.lock.blsp` read/write, and `ensure-deps` wired into `project-setup`
>   (a path dep's `src/` joins `*load-path*`, so `(require 'dep)` finds it).
>   `std/package.blsp` is the new module; no git, no network. The `(fetch)` verb
>   exists; its `nest fetch` subcommand wiring lands with the other verbs.
> - **Slice 2 — next:** `:git` deps (`%git-resolve-ref`/`%git-clone`, the
>   `_deps/` cache, clone-then-checkout the pinned commit).
> - **Slice 3:** the `nest fetch`/`update`/`add`/`remove`/`tree` subcommands +
>   auto-fetch on every subcommand.
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

The `(project …)` form (`std/project.blsp`) gains an optional
`:dependencies` slot. The value is a vector of **dep entries**. Each entry
is a vector: `[name source-kind source-spec & opts]`.

Two source kinds at v1:

| Kind     | Shape                                              | Notes |
|----------|----------------------------------------------------|---|
| `:git`   | `[name :git URL :ref REF]`                         | `REF` is a tag or commit. Branches are accepted but advisory — `:ref "main"` re-resolves on every `nest update`. |
| `:path`  | `[name :path PATH]`                                | Filesystem path, relative to the manifest. Local dev/mirror; SHA-256'd at fetch time. |

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

## `*load-path*` integration

`project-setup` (in `std/project.blsp`) gains an `(ensure-deps)` step that:

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
| `nest remove <name>`                     | Strip from `:dependencies`, drop `_deps/<name>/`, re-resolve the lock. |
| `nest tree`                              | Print the resolved dep tree (root → direct → transitive). |
| `nest test` / `run` / `check` / `format` / `mcp` | Auto-fetch missing deps on first run (a no-op on the second). |

`nest fetch` is idempotent and side-effect-free when the cache is current.

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
- **Semver + constraint solver** — `:ref "^1.2.0"` and a resolver. Real
  pain has to materialise first; pinning by commit/tag works fine for a
  pre-1.0 ecosystem.
- **Signed packages** — a `:sig` opt with a Brood-flavoured key registry
  (akin to Maven's PGP or sigstore). Needs trust infrastructure that
  isn't this project's problem until packages are exchanged at scale.
- **Per-dep build / load-path overrides** — Cargo's `[patch]` /
  `[replace]` shape. Solved for now by `:path` sources.
- **MCP `packages.list` tool surface** — exposes the resolved dep tree to
  agents. Drops in cleanly once `std/package.blsp` is in.

## Implementation sketch (when it lands)

**Rust primitives** (`crates/lisp/src/builtins.rs`):

- `(%git-clone url dest ref commit)` — shell out to `git`: clone the ref
  shallowly into `dest`, then **check out the exact `commit`**. (A plain
  `clone --depth 1 --branch <ref>` only accepts a branch/tag name, but the
  lock file always pins a commit SHA — so cloning a pinned dep needs the
  clone-then-checkout shape, fetching the commit where the server allows it.)
  Returns `:ok` or throws.
- `(%git-resolve-ref url ref)` — `git ls-remote URL REF` → commit hash
  string, or nil if not found.
- `(%sha256 string)` — hash a byte string → hex string. The **only** hashing
  primitive: per-file hashing is `(%sha256 (slurp path))` and the canonical
  directory hash is a Brood tree-walk that combines per-file hashes (see
  [Reproducibility notes](#reproducibility-notes) below) — both live in
  `std/package.blsp`, not the kernel. Also hashes the lock manifest.
- `(%http-get url)` — GET → bytes. Lands now (small), used by future
  tarball sources.
- `(%rm-rf path)` — explicit because `nest update` overwrites cached deps.
  Bounded to paths under `_deps/`; refuses anything outside.

**Brood policy** (`std/package.blsp`, new module):

- `(read-lockfile root)` / `(write-lockfile root entries)`.
- `(resolve-deps manifest lock)` — the walk in [Resolution
  algorithm](#resolution-algorithm).
- `(ensure-cache root entries)` — the cache check + clone.
- `(ensure-deps)` — called from `(project-setup)`; the auto-fetch on
  every `nest` subcommand.
- The CLI verbs: `(fetch)` / `(update & opts)` / `(add name & opts)` /
  `(remove name)` / `(tree)`.

**Manifest extension** (`std/project.blsp`):

- `(project …)` recognises `:dependencies`. Stored in
  `*project-dependencies*`. Empty when omitted (back-compat with v1
  manifests).

**`nest`'s Rust shell** (`crates/nest/src/main.rs`):

- New subcommand arms: `fetch`, `update`, `add`, `remove`, `tree`. Each
  dispatches into `(require 'package) (<verb> …)`.

### Reproducibility notes

The directory content-hash is **Brood** over the single `%sha256` primitive,
not a directory-walking Rust primitive. It needs a canonical representation:
walk paths in sorted order, and for each file emit its relative path, a NUL,
and `(%sha256 (slurp path))`; `%sha256` the concatenation of those lines.
Approximates `git archive | sha256sum` but doesn't depend on git's behaviour.
Skips `_deps/` (a dep's nested `_deps/` is its own concern, not part of this
dep's content hash) and `.git/`.

```lisp
(defn sha256-file (p) (%sha256 (slurp p)))
(defn sha256-tree (dir)
  (%sha256 (join "" (map (fn (p) (str (rel dir p) "\0" (sha256-file p) "\n"))
                         (sort (tree-files dir))))))
```

(Source files are UTF-8 text, so `slurp`-as-string is exact for v1; a future
binary/tarball dep kind would want a bytes-level read, but that's deferred
with the `:tarball` source.)

## See also

- ADR-019 — Modules (the `(require)` resolver this package layer sits on)
- ADR-020 — Project model + test runner (`project.blsp`, `nest`)
- ADR-028 — The `brood`/`nest` split
- ADR-006 — Write the language in the language (why this is Brood policy)
- ADR-037 — This design's accept-the-decision record
