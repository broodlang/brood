# Releasing a Brood app as a single binary (`nest release`)

> Status: **implemented** (ADR-038, 2026-05-31). One command turns a project into
> one self-contained executable — no interpreter install, no project dir, no
> `.blsp` files on the target. After `make install`, **`nest release` needs no
> Rust toolchain**. Code-only (no runtime asset filesystem yet). Cross-targets
> (mac M-series, Intel mac, Windows, ARM) release from a local runtime cache —
> see "Targets and portability".

## TL;DR

```bash
nest release            # → ./<project-name>, a single executable (no Rust needed)
./<project-name>        # runs the project's :main, anywhere, with nothing else
```

`nest release` appends the project's source to the lean+gui `brood` runtime
**embedded in `nest`** (baked in at `make install` time — so releasing needs no
cargo/rustc). The result is an ordinary executable that, on startup, finds the
appended archive and boots `:main` instead of a REPL.

```
nest release [-o PATH] [--runtime PATH] [--target TRIPLE]…
  -o, --output PATH    output path (default: the manifest's :name); with several
                       --targets it's the stem, each binary gets a suffix
      --runtime PATH   base runtime to append to (default: the runtime embedded
                       in nest); only valid with at most one --target
      --target TRIPLE  repeatable; resolves a prebuilt runtime from the local
                       cache (~/.cache/brood/runtimes/<triple>/brood — see below)
```

With `--target`, output names get a friendly per-target suffix (and `.exe` for
Windows): `app-macos-arm64`, `app-macos-x86_64`, `app-linux-x86_64`,
`app-linux-musl-x86_64`, `app-windows-x86_64.exe`. One invocation can emit a
whole matrix:

```bash
nest release --target aarch64-apple-darwin \
             --target x86_64-apple-darwin \
             --target x86_64-unknown-linux-gnu
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

## The embedded lean+gui runtime (no Rust at release time)

`nest release` does **not** append to your dev `brood`, and it does **not** need
a compiler. `make install` builds one lean (stripped, no-LTO) runtime and
**bakes it into `nest`** (`crates/nest/build.rs` → `include_bytes!`); `nest
release` just appends your app to that embedded copy — pure file-ops, no
cargo/rustc. (Verified: it runs with an empty `PATH`.)

The runtime is **lean** — `--no-default-features` strips, so it never compiles in:

- the **test framework** (`test`),
- the **process observer** + GC debug builtins (`observer`, `gc-stats`,
  `gc-collect`, `gc-trace`, `runtime-collect`),
- the **MCP / doc / hot-reload** tooling (`mcp`, `docs`, `reload`),
- the interactive **REPL** (`repl`).

Kept (an app legitimately needs them): the whole prelude, `project` (it boots the
bundle), and the UI/editor toolkit — `display`, `keymap`, `layers`, `ui`, `pane`,
`buffer`, `sexp`, `regex`, `face`, `highlight`, **`lineedit`** (an editor's
minibuffer reuses it), plus `tcp`/`http`/`file`/`json`/`set`/`format`/`task`/
`hatch`/`supervisor`/`ansi`/`package` — **and the `gui` backend** (it's a single
variant that includes windowing, so `(gui-open)`/windowed apps just work).

The embedded runtime is built by `make install` under the `release-fast` cargo
profile — **stripped but not LTO'd** (parallel codegen, so the install builds in a
fraction of the time the fat-LTO profile takes; the trade-off is a larger binary,
since only fat LTO + one codegen unit dead-code-eliminates the big jit/gui/treesit
dep tree). So a `make install`-baked runtime — and the apps `nest release` ships
from it — trade size and a little runtime speed for a much faster install. The
from-source fallback below (and an explicit `cargo build --profile release-lean`)
still produce the fully fat-LTO'd `release-lean` runtime when you want the
smallest/fastest shippable binary.

`gc-stats`/`require 'test`/`require 'observer` etc. are therefore unavailable in a
shipped app — that's the point. If you want one back, ship it as a `.blsp` on the
load-path, or pass a fuller `--runtime`.

> One variant for now: every release carries the gui backend (a non-gui app pays
> ~4 MB it doesn't use). A future opt-in lean/terminal-only variant is the planned
> next step.

### Fallback: no embedded runtime

A plain `cargo build` of `nest` (not via `make install`) embeds nothing. There,
`nest release` falls back to **building** the lean+gui runtime once from the
workspace source (needs Rust + the brood tree), caching it under
`target/release-lean/`. So dev checkouts still work; only `make install` gives the
no-Rust release.

## Extending a shipped app at runtime (`init.blsp`)

A bundled binary is a full evaluator — `load`, `slurp`, `require`, and
`eval-string` all read the **real filesystem**, and `def` rebinds globals (live
hot reload). So a shipped app reads external `.blsp` to extend/reconfigure itself
exactly like an editor reading `~/.config/app/init.blsp`:

```lisp
(defn main ()
  (when (file-exists? (init-path))
    (load (init-path)))     ; user code redefines/extends the running runtime
  (app-loop (initial-state)))
```

The init file can `(require 'editor/layers)` (or any kept module), add editor/layers/keymaps/
modes, `def` new commands, and redefine existing functions — all against the live
runtime. Only the *stripped* modules above are unavailable to it.

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
  `std/tool/project.blsp`, which applies the embedded manifest, loads every embedded
  module, and invokes `:main` — passing the process's argv to the entry fn.

Rust supplies only mechanism (append/extract the archive, the three
`%bundle-*` primitives); the policy lives in Brood (ADR-006).

## Targets and portability

The base `brood` is an ordinary dynamically-linked ELF — it runs on any Linux
with a compatible-or-newer glibc.

For other OS/arch combinations (mac M-series, Intel mac, Windows, ARM Linux),
`--target <triple>` resolves a prebuilt lean runtime from the **local runtime
cache**:

```
$XDG_CACHE_HOME/brood/runtimes/<triple>/brood        (~/.cache fallback;
$XDG_CACHE_HOME/brood/runtimes/<triple>/brood.exe     Windows triples)
```

You populate the cache once per target — build the lean runtime **on (or for)
that machine** and copy the binary over:

```bash
# on the target machine (e.g. an M-series mac):
cargo build --profile release-lean -p cli --no-default-features --features brood/gui
# back on the release machine:
mkdir -p ~/.cache/brood/runtimes/aarch64-apple-darwin
scp mac:brood/target/release-lean/brood ~/.cache/brood/runtimes/aarch64-apple-darwin/brood
```

From then on `nest release --target aarch64-apple-darwin` appends to it with no
toolchain at all; the runtime only needs refreshing when `brood` itself changes
(the app archive is re-appended each release). A `--target` equal to the host's
own triple needs no cache entry — the runtime embedded in `nest` serves it.
Cross-*compiling* the runtime stays out of scope for `nest release` itself
(ADR-038); `--runtime PATH` remains the explicit one-off escape hatch.

For a drop-anywhere Linux binary, cache a musl runtime the same way (buildable
on the host — `rustup target add x86_64-unknown-linux-musl` first):

```bash
cargo build --profile release-lean -p cli --no-default-features \
  --features brood/gui --target x86_64-unknown-linux-musl
mkdir -p ~/.cache/brood/runtimes/x86_64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release-lean/brood \
   ~/.cache/brood/runtimes/x86_64-unknown-linux-musl/
nest release --target x86_64-unknown-linux-musl   # → app-linux-musl-x86_64
```

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
  `%bundle-module-names`; `%builtin-module` consults the bundle; `CORE_MODULES`
  vs `DEV_MODULES` (the latter `#[cfg(feature = "dev-tools")]`); GC debug builtins
  cfg-gated
- `crates/lisp/Cargo.toml` / `crates/cli/Cargo.toml` — the `dev-tools` feature
  (default on; `cli` forwards `brood/dev-tools`, off via `--no-default-features`)
- `Cargo.toml` — the `release-lean` profile (strip + LTO + 1 codegen unit)
- `std/tool/project.blsp` — `bundle-collect` (gather sources) + `run-bundle` (boot);
  no load-time `(:use test)` so a lean runtime can load it
- `crates/cli/src/main.rs` — `brood` boots the app when bundled
- `crates/nest/src/main.rs` — `nest release`; `resolve_runtime` (`--runtime` →
  runtime cache per `--target` → embedded → built fallback); `target_suffix` /
  `runtime_cache_path`; `EMBEDDED_RUNTIME` via `include_bytes!`
- `crates/nest/build.rs` — bakes `BROOD_EMBED_RUNTIME` into `nest` (empty if unset)
- `Makefile` (`install`) — builds the lean+gui runtime, then embeds it in `nest`
- `crates/cli/tests/release_bundle.rs` — end-to-end boot test
