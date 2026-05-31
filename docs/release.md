# Releasing a Brood app as a single binary (`nest release`)

> Status: **implemented** (ADR-038, 2026-05-31). One command turns a project into
> one self-contained executable — no interpreter install, no project dir, no
> `.blsp` files on the target. Code-only (no runtime asset filesystem yet),
> Linux-first.

## TL;DR

```bash
nest release            # → ./<project-name>, a single executable
./<project-name>        # runs the project's :main, anywhere, with nothing else
```

`nest release` appends the project's source to a copy of the prebuilt `brood`
runtime. The result is an ordinary executable that, on startup, finds the
appended archive and boots `:main` instead of starting a REPL.

```
nest release [-o PATH] [--runtime PATH] [--target TRIPLE]
  -o, --output PATH    output path (default: the manifest's :name)
      --runtime PATH   base `brood` to append to (default: the `brood` beside
                       `nest`, else target/release/brood)
      --target TRIPLE   informational; cross-targets need --runtime (see below)
```

## What's in the binary

The appended archive carries, all baked in:

- `project.blsp` — the manifest (so `:main`, `:source-paths`, etc. are known)
- every `src/**/*.blsp` module
- every resolved **dependency** source (`_deps/`), so a `:path`/`:git`-dep app is
  fully self-contained

`tests/` is **excluded** — a release ships the app, not its tests.

The Brood standard library is **not** in the archive: the prelude and all `std/`
modules are already compiled into `brood` itself (`include_str!` +
`EMBEDDED_MODULES`). A release ships only your own code on top of that runtime.

It is **code-only**: runtime file reads (`(slurp "data.txt")`, `(list-dir …)`)
still go to the real filesystem on the target — the bundle is not a virtual FS.
If you need data files, ship them alongside for now.

## How it works

```
[ base `brood` binary ][ archive ][ 20-byte footer ]
```

- **Footer** (read last-bytes-first): magic `BRDBNDL1` + `u32` format version +
  `u64` archive length. Appended trailing bytes don't disturb the ELF/PE/Mach-O
  loader, so the binary still runs normally — this is the classic
  self-extracting-archive trick.
- On startup `brood` reads its own path via `std::env::current_exe()`, checks for
  the footer, and if present **mounts** the archive (`crates/lisp/src/bundle.rs`).
- A mounted bundle is just *more embedded modules*: the `%builtin-module`
  primitive consults the bundle after the baked-in std modules, so `require` and
  `(:use …)` resolve an app's own modules through the **existing** module path —
  no load-path-on-disk needed. Modules are keyed by filename **stem** (`foo.blsp`
  → `foo`), exactly the name `require` searches for.
- Boot policy is Brood: `brood` calls `(project/run-bundle argv)` in
  `std/project.blsp`, which applies the embedded manifest, loads every embedded
  module, and invokes `:main` — passing the process's argv to the entry fn.

Rust supplies only mechanism (append/extract the archive, the three
`%bundle-*` primitives); the policy lives in Brood (ADR-006).

## Targets and portability

The base `brood` is an ordinary dynamically-linked ELF — it runs on any Linux
with a compatible-or-newer glibc. For a drop-anywhere Linux binary, build the
runtime against musl and pass it as the base:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
nest release --runtime target/x86_64-unknown-linux-musl/release/brood
```

A different OS/arch (macOS, Windows, ARM) needs a `brood` built for that target;
build it there (or cross-compile) and pass it with `--runtime`. Cross-compiling
the runtime is out of scope for `nest release` itself (ADR-038).

**macOS note:** appending bytes invalidates an existing code signature; re-sign
the produced binary (`codesign`) before distributing.

## Re-releasing is safe

`nest release` strips any existing footer off the base before appending, so
releasing *from* an already-released binary (e.g. `--runtime ./myapp`) replaces
the payload rather than nesting a second archive.

## Implementation map

- `crates/lisp/src/bundle.rs` — wire format, `current_exe` mount, `strip_existing`,
  `write_release` (+ unit tests)
- `crates/lisp/src/builtins.rs` — `%bundled?`, `%bundle-manifest`,
  `%bundle-module-names`; `%builtin-module` extended to consult the bundle
- `std/project.blsp` — `bundle-collect` (gather sources) + `run-bundle` (boot)
- `crates/cli/src/main.rs` — `brood` boots the app when bundled
- `crates/nest/src/main.rs` — the `nest release` subcommand
- `crates/cli/tests/release_bundle.rs` — end-to-end boot test
