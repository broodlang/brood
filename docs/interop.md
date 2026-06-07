# Native interop: WASM components, shipped and built with packages

> **Status:** design note (2026-05-30). Backs **ADR-071** (proposed). Nothing
> here is implemented yet. This is the long-term answer to "how does a Brood
> package use code from another ecosystem (or ship a perf-critical native
> kernel) without forking the Rust kernel?" The short answer: **a package may
> ship a WebAssembly component; the package manager builds/fetches it and pins
> it in the lock file; the runtime instantiates it sandboxed and surfaces its
> exports through a Brood wrapper module.**

## The hard guarantee: zero kernel recompilation

This is the load-bearing requirement, so state it plainly. The `wasmtime` host
is compiled into the kernel **exactly once**. After that, a native extension is
**hash-pinned `.wasm` data, never kernel code** — so:

- **Adding** a native capability never rebuilds the runtime. A package drops a
  `.wasm` on the load path; `require` instantiates it. No PR against the core, no
  `cargo build`, no new `brood` binary.
- **Updating or removing** one is a manifest/lock edit, not a recompile.
- The **same shipped `brood` binary** runs every present and future extension —
  including extensions written *after* that binary was built, in languages the
  kernel has never heard of.

Contrast today's only native path: a crate baked into the kernel, where every
native capability *is* a recompile and a new binary. That coupling is exactly
what this eliminates. The one-time cost of embedding `wasmtime` buys an
open-ended, recompile-free extension surface; the steady state is **zero kernel
rebuilds, forever, for any number of native extensions.**

**"Built when we pull the package" ≠ kernel rebuild.** A native extension *is*
compiled from source — but at **`nest fetch` time, for that package only**, and
the output is a `.wasm` cached under `_deps/`. The `brood` kernel binary is never
touched. This is exactly Elixir's split: `mix deps.compile` builds a dependency's
Rustler NIF from source when you fetch it; the BEAM itself is not recompiled. We
take the same shape — build the *package's* artifact on fetch, never the runtime.

## The problem ADR-037 left open

ADR-037 (packages) deliberately closed the native-code door:

> **No install scripts.** A package that wants to ship native code does it the
> standard Rust way (a separate `cargo` crate, baked into the kernel); the
> Brood side just `require`s a wrapper. The npm-style supply-chain attack
> surface stays closed by construction.

That keeps the supply chain safe, but it means **every native capability is
kernel-blessed**: to add one you recompile the runtime, the artifact is tied to
one kernel build, and a third party can't ship native code at all without a PR
against the core. As the editor (M2+) starts inviting plugins — syntax
highlighters, language servers, format/encode/codec helpers, a regex engine, a
crypto routine — that's a wall. We need native extensions that:

1. **ship and version *with the package*** (declared in its manifest, pinned in
   the lock file), not with the kernel;
2. are **cross-kernel and cross-platform** — the same artifact works on any
   machine/runtime that fetches the package, and survives a pre-1.0 kernel that
   reshapes `Value`/the GC under it;
3. keep ADR-037's **supply-chain door shut** — fetching a dep, even one with
   native code, must not run arbitrary host code, and a buggy/hostile extension
   must not be able to crash or corrupt the runtime;
4. **don't break the runtime's invariants** — the moving GC, per-process heaps,
   immutability, and the no-worker-pinning scheduling rule.

## Why WASM (not a native dynamic-library ABI)

A native `.so`/`dylib` plugin ABI (the literal Rustler/NIF analog) fails every
one of those:

| | native `.so` plugin | **WASM component** |
|---|---|---|
| **cross-kernel** | tied to the kernel's internal `Value`/GC layout *and* the host target triple — needs an (arch × kernel-version) build matrix, breaks on every pre-1.0 kernel reshape | one portable artifact; the only ABI is the **WIT interface**, decoupled from kernel internals — a kernel upgrade never rebuilds plugins |
| **safe** | a bug segfaults the *whole runtime*; destroys the actor model's fault isolation; can scribble the Brood heap | **linear-memory sandbox** — cannot touch the host heap or segfault the runtime; a trap is a catchable Brood error, isolated to the calling process |
| **supply chain** | trusts the artifact with full host privileges | **deny-by-default capabilities** (WASI) — no fs/net/clock/env unless the manifest grants it and the host wires the import |
| **schedulable** | a runaway call pins a worker forever; uninterruptible | **fuel / epoch metering** — a call is bounded and preemptible, fits ADR-043 backstops and the offload pool |
| **polyglot** | effectively Rust/C only | any language targeting WASM — Rust, C/C++, Zig, TinyGo, AssemblyScript |

WASM is the *modern, safe* shape the user asked for, and — crucially — it is the
only option that lets native code ship per-package **without** reopening the
supply-chain hole ADR-037 sealed. The sandbox is what makes "run third-party
native code" compatible with "don't trust third-party native code."

`wasmtime` is the host engine. It joins `boxcar` (RUNTIME code region) and
`ropey` (buffers) as a runtime crate that *removes real complexity* and is
**infrastructure, not Lisp-callable behaviour** — squarely inside the
dependency rule in `CLAUDE.md`. The Lisp-visible policy (manifest parsing, the
per-package API) still lives in Brood (ADR-006).

## The boundary: marshal, never share handles

A moving, generational GC (ADR-054/055) relocates LOCAL handles; per-process
heaps are `Send`-isolated; values are immutable. So the WASM boundary **must
marshal** — you can never hand a guest a raw `Value` handle and expect it to
survive a safepoint. Brood already has exactly one serialization boundary for
this: the **`Message` enum** (`process/message.rs`) — the off-heap, heapless
representation that `send` copies across per-process heaps
(`to_message`/`from_message`). Reuse it as the interop ABI:

- **Values cross as `Message`-shaped data**, lowered to / lifted from WIT types.
  Int/float/string/keyword/bool/vector/map all have a natural WIT mapping; the
  guest sees a typed interface, the host marshals to `Value` on return. This
  reuses the freeze/copy round-trip the dist layer and `send` already exercise —
  so interop inherits the same tested marshalling, not a second one.
- **Large byte payloads ride the blob heap (ADR-041).** A `Value::Blob` is
  immutable, refcounted, cross-process-shareable bytes — the right vehicle for
  "give the codec these 4 MB and get bytes back." Copy in/out of linear memory
  for v1; investigate a zero-copy read-mapping later.
- **Stateful guest objects are opaque resources.** A parser, a DB connection, a
  compiled regex — anything the guest *owns* — is a WASM Component Model
  `resource`, surfaced on the Brood side as an **opaque resource handle behind
  primitives** (the same shape as the rope, ADR-045). It is GC-tracked as an
  opaque root; dropping it runs the guest destructor.

### A WASM instance is *mutable state* — so model it the way Brood already does

WASM linear memory is mutable. That collides head-on with Brood immutability, and
the resolution is the rule the language *already* uses for all mutable state
(`CLAUDE.md`): genuine mutable state is expressed **only** as (a) a process
holding it in its loop, or (b) a Rust-backed opaque resource handle behind
primitives — **never a mutable `Value`**. A WASM instance is exactly case (b):
an opaque, non-shareable handle. It is **not** a `Value` you can put in a map or
`send` to another process. If two processes need the same extension, each holds
its own instance, or one process owns the instance and others talk to it by
message — the standard "mutable state = a process" pattern. No new concept; WASM
instances slot into the existing one.

## Scheduling: a WASM call never pins a worker

A guest call is synchronous and CPU-bound — left alone it would hold its worker
(the scheduler pins each process to one worker, no migration). It routes through
the existing blocking story (`handoff-blocking-io.md`):

- **Short, bounded calls** run inline with a **fuel cap**; exceeding it traps to
  a catchable Brood error (an extension can't wedge the scheduler).
- **Long calls** run on the **Phase-3 blocking offload pool** — `(blocking (fn
  () (ext/parse …)))` runs the guest off the worker pool, fuel-metered, and
  **delivers the result to the caller's mailbox**; the process parks in
  `receive`. Same deliver-to-mailbox principle as TCP (ADR-062), GUI input
  (ADR-059), and dist. Thousands of processes can each be mid-extension-call
  without starving the pool.

`wasmtime`'s epoch interruption is what makes this enforceable, and it's the same
budget lever ADR-043 already pulls.

## Package-manager integration (the ADR-037 extension)

This is the part that makes it "compiled with the code." The artifact is treated
as **just more package data** — hash-pinned in the lock file exactly like source
— and the runtime never trusts it (the sandbox does that). That framing keeps
ADR-037's supply-chain invariant fully intact, and even *strengthens* it: today's
native path trusts crates.io and bakes opaque code into the kernel; this path
hash-pins the artifact *and* sandboxes it.

### Manifest: a `:native` clause

A package that ships native code declares it in *its own* `project.blsp` with a
`:native` slot. The **primary** form is build-from-source-on-fetch (the Rustler
model); a prebuilt artifact is an optional optimization.

| Kind | Shape | Notes |
|---|---|---|
| `:wasm-build` (**primary**) | `[:wasm-build :crate SUBDIR :target wasm32-wasip2 :toolchain cargo]` | Build **from source** at fetch time with a *declared* toolchain + target. The consumer needs the wasm toolchain installed (as a Rustler consumer needs Rust). This is what "built entirely when we pull the package" means. |
| `:wasm-artifact` (optimization) | `[:wasm-artifact PATH-or-URL :sha256 HASH]` | A **prebuilt** `.wasm` — committed in the repo or fetched by URL — so a consumer without the toolchain (or who wants fast/offline installs) skips the build. The Elixir ecosystem's `rustler_precompiled` is exactly this escape hatch over build-on-fetch; we anticipate the same need. |

Plus a capability grant list (deny-by-default):

```lisp
(project brood-fastjson                          ; the dep's OWN manifest
  :native [fastjson
           :wasm-build  (:crate "native" :target "wasm32-wasip2" :toolchain cargo)
           :interface   "native/fastjson.wit"    ; the WIT contract the wrapper binds against
           :capabilities []])                     ; pure compute — no fs/net/clock
```

### The "no install scripts" line stays — reframed, not broken

ADR-037 banned install scripts to close the npm `postinstall` hole: *arbitrary
host code at install time*. Building on fetch sounds like it reopens that — it
doesn't, because of *what* runs and *what the output can do*:

- **The build is a declared toolchain invocation, not an arbitrary hook.** The
  manifest names a toolchain + target (`cargo build --target wasm32-wasip2`); the
  package manager runs *that*, not a `postinstall` shell script of the package's
  choosing. There is no place for "run this arbitrary command on the host at
  install." (This is also exactly the Rustler trust model — `mix deps.compile`
  runs `cargo`, declaratively, not a free-form hook.)
- **The output is sandboxed regardless.** Whatever the build produces is a
  `.wasm` that runs under deny-by-default capabilities — it can't touch the host
  fs/net/heap unless granted. So even a *malicious* build output is contained,
  which is strictly stronger than today's "bake an opaque crate into the kernel
  with full host privileges."
- **`:wasm-artifact` runs nothing at install** — a hash-checked download, the
  same trust model as fetching source, reproducible bit-for-bit via the lock.

So the rule becomes: **no arbitrary host code at install; a declared
build-toolchain or a hash-pinned prebuilt artifact only, and everything executes
sandboxed.** The honest cost (shared with Rustler): build-on-fetch needs the wasm
toolchain present and pays compile time — which is exactly why `:wasm-artifact`
(the `rustler_precompiled` analog) exists as the escape hatch.

### Lock file records the artifact

`project.lock.blsp` gains a `:native` field per dep — the `.wasm` hash, plus
build provenance when built from source:

```lisp
[fastjson
 :git    "https://…/brood-fastjson" :ref "v1.0.0" :commit "…" :sha256 "…"
 :native (:wasm :sha256 "deadbeef…"            ; the artifact hash — the "compiled with the code" pin
                :interface-hash "…"            ; the WIT the host binds against
                :built-from :artifact)         ; or :source + {:toolchain "cargo 1.x" :target "wasm32-wasip2"}
 :deps   []]
```

Same reproducibility guarantee as source: a clean machine fetching this lock
gets a byte-identical `.wasm`. The compiled artifact is locked *alongside* the
source it came from.

### Cache + load

- The `.wasm` lands under `_deps/<name>/native/` — per-project, gitignored,
  hermetic, consistent with ADR-037's rejection of a global cache.
- **Loading is `require`.** When a package with a `:native` clause is `require`d,
  its Brood wrapper module instantiates the component and binds its exports. **No
  change to `require`/load semantics** — the native side is just a primitive the
  wrapper calls.

## Wrapping: the Rustler analog

Rustler's ergonomics come from two halves: a Rust crate with `#[rustler::nif]`
functions + `rustler::init!`, and an Elixir module that `use Rustler` — declaring
each NIF as a stub the loader replaces. Brood mirrors this:

- **Guest side (any WASM language).** The extension exports functions over a
  **WIT interface** — the typed contract (the Component Model's role, replacing
  Rustler's `init!` registration). In Rust that's a `#[component]` exporting the
  `world` declared in the package's `.wit`.
- **Brood side (the wrapper module).** A macro — `use-native` — is the `use
  Rustler` analog. Given the package's component handle + its WIT interface, it
  binds **every exported function as a Brood function in the namespace**,
  marshalling args/results across the boundary, so callers see ordinary Brood
  functions:

  ```lisp
  (ns fastjson)
  ;; binds `parse`, `stringify`, … from the instantiated component as
  ;; namespace functions — the `use Rustler` moment, but driven by the WIT.
  (use-native 'fastjson)

  ;; consumers just:
  (require 'fastjson)
  (fastjson/parse "{\"a\":1}")        ; => {:a 1}  — a normal Brood call
  ```

The wrapper is the right place for *policy* (ADR-006): validate/shape inputs,
provide idiomatic defaults, wrap opaque guest resources in a Brood API — the same
role `std/net/tcp.blsp` plays over the net primitives. `use-native` itself is a thin
macro over the `%wasm-*` primitives below; the WIT interface is what lets it
generate the bindings instead of the author hand-writing a stub per function (the
advantage over Rustler's manual stub list).

## Kernel surface (small, à la ADR-037's `%git-clone`/`%sha256`)

Mechanism in Rust; everything else in Brood.

- Embed `wasmtime` (component model) in the `brood` lib crate.
- `%wasm-instantiate path caps → handle` — load + instantiate a component with a
  capability set; returns an opaque resource handle.
- `%wasm-call handle export args → value` — marshal `Message ↔ WIT`, fuel-capped.
- `%wasm-build src target toolchain → path` — drive the declared toolchain on
  fetch (shells out, like `%git-clone` shells out to `git`). May just be Brood
  over a generic subprocess primitive rather than a dedicated one.
- Resource-drop wiring so a dropped Brood handle runs the guest destructor.

Manifest `:native` parsing, build orchestration, lock-file fields, cache layout,
capability grants, and each package's wrapper API — **all Brood**
(`std/tool/package.blsp` + the package's own module).

## Relationship to today's native path

This **complements** kernel builtins; it doesn't replace them. The split is by
*who owns the code*, and it maps cleanly onto recompilation:

- **In the kernel (recompile to change):** the `wasmtime` host itself, plus the
  blessed, universally-needed, performance-critical primitives the runtime can't
  bootstrap — rope ops, the GC, core math. These ship *with* the binary because
  they *are* the binary.
- **In packages (zero recompile, built on fetch):** every third-party,
  optional, or ecosystem native capability. A WASM extension is package data,
  compiled when the package is pulled, never coupled to the kernel build.

So the recompile boundary is exactly the kernel/package boundary. Some current
kernel builtins *could* later migrate to WASM components if that proves cleaner,
but nothing forces it.

## Open questions (answer on implementation)

- **Component Model + WIT vs. core WASM + a hand-rolled ABI.** Recommend the
  Component Model (typed WIT interfaces) as the long-term ABI — it's the modern,
  safe, self-describing shape and aligns the interop ABI with a real interface
  language. Cost: it's younger/heavier than core WASM. Revisit if wasmtime's
  component support proves too green.
- **Async guests.** WASI 0.3 async + wasmtime async vs. the offload pool — how
  an extension that itself wants to do async IO composes with deliver-to-mailbox.
- **Zero-copy blobs.** Can a `Value::Blob`'s bytes be read-mapped into linear
  memory without a copy? Material for codec/parse throughput.
- **Build hermeticity.** Do we sandbox the `:wasm-build` toolchain run, or trust
  the declared toolchain? v1: trust declared toolchain, *prefer prebuilt
  artifacts* so most consumers never build.
- **Capability UX.** Deny-by-default + an explicit grant list ships first; a
  richer per-extension capability/permission UI (the editor will want "this
  highlighter can't read my keys") is deferred (ADR-011).
- **Cross-node extensions.** A WASM instance is local mutable state (not a
  `Value`), so it does **not** travel in a `send`/closure-ship. Cross-node use is
  "talk to the process that owns the instance" — already the rule. Worth a line
  in `distribution.md` when implemented.

## References

ADR-037 (packages — the manifest/lock/cache this extends, and the "no install
scripts" line this reframes), ADR-041 (blob heap — large byte payloads),
ADR-045 (opaque immutable resource handle — the rope precedent for the WASM
handle), ADR-043 (resource backstops — fuel/epoch), ADR-059/062 +
`handoff-blocking-io.md` (deliver-to-mailbox; the offload pool a long guest call
uses), ADR-054/055 (moving/generational GC — why the boundary marshals), ADR-006
(write the language in the language — wrapper + policy in Brood), `CLAUDE.md`
(runtime-crate rule; "mutable state = a process or an opaque handle").
