# Convenience wrapper around the Cargo workspace. Cargo remains the source of
# truth — these targets just give short, memorable commands. Pass extra args
# with ARGS=..., e.g. `make benchmark ARGS=sum_tail`.

CLI  := cargo run -q -p cli
NEST := cargo run -q -p nest
ARGS ?=

# Build options recorded by `./configure` (re-run it to change them). The
# `-include` is silent when config.mk is absent — `make install` then uses the
# defaults below (no GUI, ~/.local), so the build works with or without configure.
-include config.mk
PREFIX   ?= $(HOME)/.local
WITH_GUI ?= 0
# `WITH_GUI` anything but 0/empty → compile the native window backend into the
# binaries that run user code (brood, nest); the LSP never opens a UI.
GUI_FEATURES := $(if $(filter-out 0,$(WITH_GUI)),--features brood/gui,)
# `./configure --with-gui-gpu` (WITH_GUI_GPU) builds the EXPERIMENTAL OpenGL backend
# alongside `gui`. The `gui-gpu` feature implies `gui`, and cargo unions repeated
# `--features`, so this composes with GUI_FEATURES above.
WITH_GUI_GPU ?= 0
GUI_GPU_FEATURES := $(if $(filter-out 0,$(WITH_GUI_GPU)),--features brood/gui-gpu,)
# `./configure --with-audio` (WITH_AUDIO) compiles the audio backend (the
# `audio-beep` builtin, via rodio). OFF by default and INDEPENDENT of gui: on Linux
# it links libasound.so.2 as a hard runtime dep, so a default/gui release stays
# portable (no audio) and only an opt-in build carries the ALSA dependency.
WITH_AUDIO ?= 0
AUDIO_FEATURES := $(if $(filter-out 0,$(WITH_AUDIO)),--features brood/audio,)
# JIT (ADR-101): the tier-1 template JIT, ON by default — now a *cargo default
# feature* of the brood lib, so ordinary `cargo build`/`cargo test`/`make test`/
# rust-analyzer and every binary get it uniformly (hot compute loops run as native
# code). This variable only governs the `--no-default-features` release/install
# path below: `./configure --without-jit` (WITH_JIT=0) re-strips it for an
# unsupported host or a minimal build; otherwise the lean bundle re-adds it here.
WITH_JIT ?= 1
JIT_FEATURES := $(if $(filter-out 0,$(WITH_JIT)),--features brood/jit,)
# tree-sitter (foreign-language editor modes — ruby/elixir, ROADMAP §C). The
# generic `treesit` mechanism is in `default`, but the language grammars are NOT
# (the kernel ships no language-specific parser). A product install still wants
# them, so the lean install bakes in `treesit-grammars` explicitly. `make install`
# builds `--no-default-features`, so it's named here (cargo unions repeated
# `--features` flags, so this composes with GUI_FEATURES). Not gated on configure.
TS_FEATURES := --features brood/treesit-grammars

# Optional cross-compile target triple (empty = build for the host). When set, e.g.
# `make release TARGET=x86_64-apple-darwin`, cargo builds for that triple and emits into
# `target/<triple>/…`, so RELEASE_DIR (and the embedded-runtime path) follow it. This is
# how the release CI cross-builds the Intel-mac binary on an Apple-Silicon runner (the
# `x86_64-apple-darwin` target added via rustup). Empty keeps the exact prior behavior.
TARGET ?=
CARGO_TARGET := $(if $(TARGET),--target $(TARGET),)
TARGET_SUBDIR := $(if $(TARGET),$(TARGET)/,)

# Local (gitignored — `target/` is in .gitignore) output dir for the optimized
# binaries. `make release` builds into here; `make install` copies them out to
# $(PREFIX)/bin. So building and installing are separate steps. The `release-fast`
# profile (Cargo.toml: stripped, no LTO) builds in a fraction of the time the
# fat-LTO `release-lean` profile takes (bigger binary is the trade-off). (`release-lean`
# still exists; `nest release` uses it for the shippable runtime.)
RELEASE_DIR := target/$(TARGET_SUBDIR)release-fast

# Performance build flags for `release`: debug-assertions + overflow-checks OFF.
# rustc takes the LAST `-C <key>=` for a key, so these win even if the GC-debug
# build mode (`RUSTFLAGS="-C debug-assertions=on"`, see CLAUDE.md) is exported in
# the shell — the installed binary is never accidentally debug-armed (which would
# carry the GC tripwire/verifier overhead and skew benchmarks). Stripping is the
# `release-fast` profile's job (a profile `strip` reliably strips; `-C strip` here
# would not).
PERF_RUSTFLAGS := $(RUSTFLAGS) -C debug-assertions=off -C overflow-checks=off
# Features baked into the binaries that RUN user code (brood, nest).
# `--no-default-features` strips the dev/debug surface; cargo unions the rest.
# brood-lsp runs no hot user code, so it takes none of these.
#
# RUN_FEATURES is the LEAN set: no `dev-tools`, so no `repl`/`test`/`observer`/`mcp`
# DEV_MODULES. It builds the brood that gets EMBEDDED into nest for `nest release`
# app bundling (small apps) and that `make ab` measures — both want it lean.
RUN_FEATURES := --no-default-features $(GUI_FEATURES) $(GUI_GPU_FEATURES) $(AUDIO_FEATURES) $(TS_FEATURES) $(JIT_FEATURES)
# INSTALL_FEATURES is RUN_FEATURES + `dev-tools`: the set for the brood/nest
# actually INSTALLED onto your PATH, where the REPL, `nest test`, `nest observe`,
# and `nest mcp` must work. `make install` builds the lean brood (embed base) AND
# these dev binaries — the embedded runtime stays lean, the tools you run don't.
INSTALL_FEATURES := $(RUN_FEATURES) --features brood/dev-tools

# Copy the three binaries from $(1) into $(PREFIX)/bin — no rebuild, no cargo install.
define install_binaries
	@mkdir -p $(PREFIX)/bin
	install -m755 $(1)/brood     $(PREFIX)/bin/brood
	install -m755 $(1)/nest      $(PREFIX)/bin/nest
	install -m755 $(1)/brood-lsp $(PREFIX)/bin/brood-lsp
endef

.DEFAULT_GOAL := help
.PHONY: help build release perf-brood test test-light test-both breakagetests ensure-nextest bench benchmark quickbench suite repl configure install uninstall fmt clippy check clean

help: ## Show this help
	@echo "Brood — available make targets:"
	@grep -hE '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

build: ## Build the whole workspace
	cargo build

test: ## Run Rust tests + the in-language suite via cargo-nextest (each test case process-isolated and hard-capped at 2 min — see .config/nextest.toml)
	# nextest runs each test in its own process: a single hung case is killed at the
	# 2-min per-case cap (and a SIGSEGV — Brood's stack-overflow failure mode — is
	# contained to that case instead of aborting the whole binary). `--no-fail-fast`
	# surfaces every result. Install: `make ensure-nextest` (or see https://nexte.st).
	@command -v cargo-nextest >/dev/null 2>&1 || { echo ">>> cargo-nextest not found — run 'make ensure-nextest' (or install from https://nexte.st)"; exit 1; }
	# Grammars are out of `default` (the kernel ships no language-specific parser),
	# so the suite opts into `treesit-grammars` to exercise the ruby/elixir tests.
	# The .blsp grammar tests also self-skip when absent, so a bare `cargo nextest`
	# (no grammars) stays green too.
	# The `SETUP warm-boot-cache` line comes from a nextest setup script that boots
	# `brood`/`nest` once (~2.4s) before the fan-out, so the spawned children don't
	# each pay the cold prelude expansion — KI-38. `BROOD_NO_WARM_BOOT_CACHE=1`
	# turns it off; see .config/nextest.toml.
	cargo nextest run --no-fail-fast --features brood/treesit-grammars
	cargo test --doc   # nextest doesn't run doctests; none today, kept so future ones still run

# Concurrency cap for `test-light` — override on the command line, e.g. `make test-light LIGHT_JOBS=4`.
LIGHT_JOBS ?= 8  # concurrent test PROCESSES (nextest -j) and cargo build jobs

test-light: ## Like `make test` but capped + de-prioritized so it won't saturate the machine (override LIGHT_JOBS)
	# Same suite as `make test`, tuned to stay off your back on a busy desktop —
	# the default `make test` runs the compile AND the ~50-binary fan-out at full
	# core count, which oversubscribes a many-core box (load > nproc) and stalls
	# interactive work. This caps each layer instead:
	#   nice -n19        — lowest CPU priority, so interactive work always preempts it
	#                      (this is what actually keeps the desktop responsive)
	#   CARGO_BUILD_JOBS — caps the compile/link spike
	#   -j N             — caps how many test PROCESSES nextest runs at once (default: all cores)
	# Trade-off is less parallelism *stress*, not less coverage: every case still runs in its
	# own process — fewer cores just run concurrently. Keep the full-fat `make test` for the
	# "prove green under load" runs the docs call for (concurrency/scheduler/GC/JIT changes).
	#
	# NB: deliberately does NOT set BROOD_J (the runtime's scheduler-pool cap). BROOD_J is a
	# real knob for squeezing the in-language suite (`brood_suite_passes`), but it FLOORS the
	# worker pool, so it forces extra workers onto the tests that pin themselves to exactly one
	# (`cpu_bound_process_does_not_starve_peers_on_one_worker`) and reddens them — see
	# `worker_count` in crates/lisp/src/process/scheduler.rs. Set it by hand for an ad-hoc run
	# if you know you're not touching those, but it can't be a default here.
	@command -v cargo-nextest >/dev/null 2>&1 || { echo ">>> cargo-nextest not found — run 'make ensure-nextest' (or install from https://nexte.st)"; exit 1; }
	nice -n 19 env CARGO_BUILD_JOBS=$(LIGHT_JOBS) \
		cargo nextest run --no-fail-fast --features brood/treesit-grammars -j $(LIGHT_JOBS)
	nice -n 19 cargo test --doc

test-both: ## Run the whole suite through BOTH engines (tree-walker + VM) — the differential gate (ADR-076)
	# The VM is the default engine; this also exercises the tree-walker escape hatch
	# (BROOD_VM=0) so a regression in either is caught. `differential.rs` additionally
	# checks per-expression engine agreement within one run.
	@command -v cargo-nextest >/dev/null 2>&1 || { echo ">>> cargo-nextest not found — run 'make ensure-nextest'"; exit 1; }
	@echo ">>> suite under the VM (default engine)"
	BROOD_VM=1 cargo nextest run --no-fail-fast
	@echo ">>> suite under the tree-walker (BROOD_VM=0 escape hatch)"
	BROOD_VM=0 cargo nextest run --no-fail-fast

# Nothing is skipped: all 23 breakage files gate. Kept as a mechanism for the next file that
# needs it — name a file here and the runner prints the skip on every run rather than hiding it.
BREAKAGE_SKIP :=

# Per-file memory allowance. The runners default a ~1 GiB soft ceiling (ADR-043) so an
# adversarial test cannot take the machine down, but `chaos_map_volcano` legitimately needs
# more: its 1M-entry map peaks at ~3.0 GB RSS and finishes in ~13 s. Note the ordering rule
# from `core/alloc.rs` — the soft limit must sit BELOW the hard one, because soft is checked
# at an eval safepoint and raises a clean catchable error, while hard is enforced inside
# `alloc` and returns null, so Rust's OOM handler aborts the process. Setting them EQUAL
# leaves the safepoint no headroom and turns a graceful failure into an abort; that is the
# documented backstop working, not a bug (it briefly looked like one on 2026-08-13).
BREAKAGE_ENV_map_volcano := BROOD_MEM_SOFT_LIMIT=4000000000 BROOD_MEM_LIMIT=6000000000

check-examples: ## Run every `examples/` program and fail on an unbound symbol — the gate `examples/` never had
	# `examples/` sits outside `make test`, `nest check` AND the breakage suite, so nothing
	# ever ran it — and it rotted three ways unnoticed: `examples/editor` has called a module
	# that left the repo on 2026-05-31 (KI-45), ADR-227's stdlib move needed a `:use` in
	# `life.blsp`, and ADR-229's `require` removal stopped `webserver`/`hot-reload`/`editor`
	# loading at all. Same pattern as KI-42/43/44; this is the counter for examples/.
	#
	# It asserts NO `unbound symbol` diagnostic rather than exit 0, because several examples
	# legitimately cannot finish here (servers run until killed, `node_client` wants a peer,
	# `font-zoom` wants --features gui) — but an unbound name is never environment noise.
	@./scripts/check-examples.sh

breakagetests: ## Run the aggressive `breakage/` stress suite (JIT on, GC tripwire armed) — try to break the JIT/VM/memory. NOT part of `make test`.
	# These are deliberately abusive tests that live OUTSIDE tests/ (so neither
	# `make test` nor `nest test` ever discovers them) and try to make the JIT
	# diverge from the VM, overflow the eval stack, leak/corrupt the heap, or
	# deadlock the scheduler. Each file is a self-contained `brood --test` suite.
	#
	# Built fast-but-armed: `--release` for speed (the loops warm the JIT past its
	# tiering threshold, which needs real iteration counts; the JIT is a default
	# feature so it fires without a flag) + `-C debug-assertions=on` so the per-deref
	# GC tripwire and the heap verifier stay armed (catch a use-after-GC at the bad
	# deref, not as a distant SIGSEGV). For the heaviest GC hunt, re-run with
	# `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 make breakagetests` (much slower:
	# collects at every safepoint).
	#
	# Each file runs in its OWN process, so a segfault (Brood's stack-overflow
	# failure mode) is contained to that file — the loop keeps going and the
	# summary still prints. Exits non-zero if any file failed or crashed.
	@echo ">>> building brood (release, +jit, debug-assertions armed) ..."
	RUSTFLAGS="$(RUSTFLAGS) -C debug-assertions=on" cargo build --release -p cli
	@bin=target/release/brood; fail=0; \
	echo ">>> running breakage suite with $$bin"; \
	for f in breakage/*.blsp; do \
		case " $(BREAKAGE_SKIP) " in *" $$f "*) \
			echo ""; echo "===== $$f ===== SKIPPED (see docs/known-issues.md KI-42)"; \
			continue ;; \
		esac; \
		echo ""; echo "===== $$f ====="; \
		case "$$f" in \
			breakage/chaos_map_volcano.blsp) pre="$(BREAKAGE_ENV_map_volcano)" ;; \
			*) pre="" ;; \
		esac; \
		if [ -n "$$pre" ]; then echo ">>> (with $$pre)"; fi; \
		env $$pre $$bin --test "$$f"; \
		rc=$$?; \
		if [ $$rc -ne 0 ]; then \
			fail=1; \
			if [ $$rc -gt 128 ]; then echo ">>> CRASH ($$f): exit $$rc (signal $$((rc-128)) — likely SIGSEGV / stack overflow)"; \
			else echo ">>> FAIL ($$f): exit $$rc"; fi; \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -ne 0 ]; then echo ">>> breakage suite: FAILURES above"; exit 1; \
	else echo ">>> breakage suite: all files passed"; fi

ensure-nextest: ## Install cargo-nextest into ~/.local/bin (prebuilt binary) if it's missing
	@command -v cargo-nextest >/dev/null 2>&1 && { echo "cargo-nextest already installed: $$(cargo nextest --version)"; } || { \
		echo "installing cargo-nextest into $(HOME)/.local/bin ..."; \
		mkdir -p $(HOME)/.local/bin; \
		curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C $(HOME)/.local/bin; \
		echo "installed: $$(cargo nextest --version)"; }

bench: benchmark ## Alias for `benchmark`

benchmark: ## Run benchmarks; archive results to docs/benchmarks/<timestamp>.md
	./scripts/bench.sh $(ARGS)

quickbench: ## Fast (~10s) benchmark for iteration — no archive, few samples
	./scripts/quickbench.sh $(ARGS)

ab: ## A/B the working tree against a git ref on the cross-language rows: make ab BASE=<ref> ROWS="fib pfib" N=7
	./scripts/ab-bench.sh $(if $(BASE),-b $(BASE),) $(if $(N),-n $(N),) $(ROWS) $(ARGS)

doctor: ## Report the things that make a measurement or a gate lie (build drift, strays, boot cache, litter)
	# Read this BEFORE trusting a benchmark delta or a green gate. Every check maps to a
	# class that has cost a real session — chiefly build drift, because a stale binary fails
	# by AGREEING with the baseline (an A/B reads +0.0%, a flag sweep reads 1.0x on every
	# row, a lowering-witness diff comes back empty). `--strict` exits 1 on any finding.
	@./scripts/doctor.sh $(ARGS)

ab-pin: ## A/B against a PINNED baseline binary that survives ab-clean: make ab-pin BASE=<ref> ROWS="pipeline nqueens" N=15
	# `make ab` rebuilds its baseline in a throwaway worktree that `make ab-clean` removes, so
	# two runs on different days measure against two different binaries — and a few-percent row
	# cannot then be told from drift. ADR-228 hit exactly that: two best-of-15 runs of the same
	# comparison read -9.1% and -5.6%, so the ADR records a range instead of a number.
	#
	# This keeps the baseline in target/ab-pinned/<sha>/brood (gitignored, NOT under target/ab/)
	# and runs base-vs-base as the floor, which is the method CLAUDE.md prescribes.
	N=$(or $(N),15) ./scripts/ab-pin.sh $(or $(BASE),HEAD) $(or $(ROWS),pipeline nqueens)

ab-vm: ## A/B the VM's own call path (tier 1) — the regressions `make ab` structurally cannot see
	# At the DEFAULT ceiling a hot arm lowers to native, so the interpreter's call path never
	# executes and a cost added to it reads as flat. KI-40 was a 3.19x regression on that path
	# that `make ab` reported as +1.3%, because every row it runs lowers. Concurrency rows here
	# also run on all cores (see `parallel_rows`), which is what makes contention visible at all.
	./scripts/ab-bench.sh --tier 1 --floor $(if $(BASE),-b $(BASE),) $(if $(N),-n $(N),) $(or $(ROWS),pfib spawn-live fib collatz)

ab-clean: ## Remove the baseline worktrees + builds that `make ab` created under target/ab/
	@for d in target/ab/*/; do [ -d "$$d" ] && git worktree remove --force "$$d" 2>/dev/null || true; done
	@rm -rf target/ab
	@git worktree prune
	@echo "removed target/ab"

suite: ## Run the in-language suite via the project runner (discovers tests/**/*_test.blsp)
	$(NEST) test

stress: build ## The occasional BIG stress run (property/differential/race tests, 3 engines) — not part of CI
	./stress/run.sh

# ASAN_OPTIONS=symbolize=0 because the system llvm-symbolizer stalls ~90 s at
# EVERY exit against the 65 MB sancov binary (found 2026-07-23 — it made the
# loop look like 4 execs/min). A crash artifact can be re-run symbolized:
#   cd crates/lisp && cargo +nightly fuzz run <T> fuzz/artifacts/<T>/<file>
fuzz: ## Run one libFuzzer target briefly: make fuzz T=wire SECS=60 (targets: reader eval json wire bundle)
	cd crates/lisp && ASAN_OPTIONS=symbolize=0 cargo +nightly fuzz run $(or $(T),reader) -- -max_total_time=$(or $(SECS),60) -rss_limit_mb=4096

tsan: ## ThreadSanitizer over the concurrency-sensitive Rust tests (needs nightly + rust-src; system-alloc feature so mimalloc's un-instrumented internals don't report phantom races)
	RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu -p brood --release --features brood/system-alloc --test table_tsan --test concurrency_race --test preemption --test live_migration --test local_send_race --test gc

loom: ## Loom model-check of the dense-table migration protocol (exhaustive interleavings of a faithful model)
	cargo test -p brood --release --features brood/loom-model --test loom_table_protocol

asan: ## AddressSanitizer over the kernel-exercising Rust tests (needs nightly + rust-src; system-alloc so ASAN can intercept allocations instead of mimalloc's un-instrumented arena). Catches genuine OOB / use-after-free in the unsafe substrate (mmap table, JIT codegen buffers) that TSAN and the logical GC tripwires miss. `--tests` skips doctests, which don't LINK under ASAN + -Zbuild-std (a toolchain quirk, not a finding).
	# BROOD_STACK_BUDGET is raised because ASAN's redzones make every Rust frame far
	# fatter: the prelude's macro-expansion recursion measured **15.2 MB** of stack under
	# instrumentation against the 12 MiB default, so the runtime's own "recursion too deep"
	# guard fired during BOOT and took `differential.rs` down with it (the panic poisoned a
	# LazyLock, so the second test failed as a cascade). That looked like an ASAN finding and
	# was not one — ASAN itself reported nothing on 581 passing tests. Left unset, this gate
	# silently cannot run the differential corpus, which is most of its value. Found
	# 2026-08-17; with 64 MiB both tests pass and ASAN's checks still apply unchanged.
	BROOD_STACK_BUDGET=67108864 RUSTFLAGS="-Zsanitizer=address" cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu -p brood --release --features brood/system-alloc --tests

repl: ## Start the REPL
	$(CLI)

configure: ## Show current build options (./configure --with-gui to enable the GUI)
	@echo "PREFIX   = $(PREFIX)"
	@echo "WITH_GUI = $(WITH_GUI)$(if $(GUI_FEATURES), (GUI backend on),)"
	@echo "WITH_AUDIO = $(WITH_AUDIO)$(if $(AUDIO_FEATURES), (audio-beep on),)"
	@echo "Run ./configure --with-gui to enable the native window; ./configure --help for more."

release-brood: ## Build ONLY the `brood` binary into $(RELEASE_DIR) — the perf-A/B build step (skips nest + brood-lsp)
	# Exactly the flags `release` uses for the binary that RUNS user code, and
	# nothing else: `scripts/ab-bench.sh` builds both sides of an A/B through this
	# target so the two binaries cannot differ in profile or features. Note the
	# package is `cli` (the binary), NOT `-p brood` (the lib) — `-p brood` does not
	# relink $(RELEASE_DIR)/brood and silently benchmarks a stale binary.
	RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-fast -p cli $(RUN_FEATURES) $(CARGO_TARGET)

perf-brood: ## Build a counter-armed `brood` into $(RELEASE_DIR) — the attribution build ((perf/report), BROOD_PERF_STATS, BROOD_DEOPT_TRACE)
	# The VM work-attribution counters are a cargo feature (`perf.rs`), so a normal
	# binary — including an installed one — cannot answer "where does the time go".
	# This is that binary, and the reason it is a target rather than a documented
	# command line: the flags have to match `release-brood`'s exactly except for the
	# added feature, or you are comparing two different builds.
	#
	# Then: `BROOD_PERF_STATS=1 $(RELEASE_DIR)/brood prog.blsp` dumps the counters at
	# exit, or `(perf/report)` / `(perf/summary)` reads them in-image with the
	# docs/benchmarking.md §2 interpretation applied. `BROOD_DEOPT_TRACE=1` also needs
	# this build.
	#
	# NOT for timing. The counters are atomics on the hot path, so this binary is the
	# wrong one to measure *times* with — that is `make ab` / `scripts/bench-ratio.sh`
	# on a counter-free build. Keeping the two apart is what docs/benchmarking.md is
	# about; it was written after conflating them cost an afternoon.
	RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-fast -p cli \
		$(INSTALL_FEATURES) --features brood/perf-stats $(CARGO_TARGET)
	@echo "counter-armed brood: $(RELEASE_DIR)/brood"
	@echo "  NOTE this OVERWRITES the same path \`make release-brood\`/\`make ab\` use —"
	@echo "  re-run \`make release-brood\` before timing anything, or you will time the counters."
	@echo "  BROOD_PERF_STATS=1 $(RELEASE_DIR)/brood prog.blsp   # dump counters at exit"
	@echo "  (require 'perf) (perf/summary)                       # in-image triage"

release: release-brood ## Build optimized `brood`, `nest` and `brood-lsp` into $(RELEASE_DIR) (gitignored; does NOT install — ./configure --with-gui first for the window)
	# Build the configured (./configure) binaries into the local, gitignored
	# $(RELEASE_DIR) with the `release-fast` profile (stripped, no LTO) — fast
	# to build. The separate `install` target copies them out to $(PREFIX)/bin.
	#
	# The LEAN brood from `release-brood` (above) is what gets embedded into nest
	# (BROOD_EMBED_RUNTIME → ADR-038), so `nest release` ships a small self-contained
	# app with no Rust (an LTO'd shippable runtime rebuilds under `release-lean` — see
	# docs/release.md). nest embeds it here, at nest's build, so the bytes are baked in
	# before the dev `brood` below overwrites the file.
	BROOD_EMBED_RUNTIME=$(CURDIR)/$(RELEASE_DIR)/brood RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-fast -p nest $(INSTALL_FEATURES) $(CARGO_TARGET)
	RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-fast -p brood-lsp $(CARGO_TARGET)
	# The `brood` you actually RUN needs the REPL, so rebuild the installed binary WITH
	# dev-tools (repl/test/observer/mcp), overwriting the lean embed source nest has
	# already baked in. Split on purpose: apps ship the lean runtime; your PATH gets the
	# full one. `make ab`/`make release-brood` rebuild the lean brood as needed.
	RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-fast -p cli $(INSTALL_FEATURES) $(CARGO_TARGET)

install: release ## Build (per ./configure) + install `brood`, `nest`, `brood-lsp` into $(PREFIX)/bin
	$(call install_binaries,$(RELEASE_DIR))

uninstall: ## Remove the installed binaries from $(PREFIX)/bin (leaves the local $(RELEASE_DIR) build intact)
	# Removes only what `install` placed on the system — the local release build
	# in $(RELEASE_DIR) is left alone (use `make clean` to remove that).
	rm -f $(PREFIX)/bin/brood $(PREFIX)/bin/nest $(PREFIX)/bin/brood-lsp

gui-debug: ## Build + install JIT+GUI brood/nest with GC debug-assertions ARMED, for debugging the JIT+GC bug (bug #2). Then `nest run` in your project.
	# The diagnostic build for chasing the JIT+GC use-after-GC (bug #2): JIT ON (so
	# it still reproduces), GUI ON (so the app renders), and `-C debug-assertions=on`
	# so the per-deref GC tripwire + the `[jit-staged-stale]` staging check fire AT
	# the corruption site instead of surfacing as a distant OOB / `*: got nil`.
	# Installs over $(PREFIX)/bin so a plain `nest run` picks it up. Restore the fast
	# build afterwards with `make install` (or `make release && make install`).
	RUSTFLAGS="$(RUSTFLAGS) -C debug-assertions=on" cargo build --release -p cli  --features "brood/jit brood/gui"
	RUSTFLAGS="$(RUSTFLAGS) -C debug-assertions=on" cargo build --release -p nest --features "brood/jit brood/gui"
	@mkdir -p $(PREFIX)/bin
	install -m755 target/release/brood $(PREFIX)/bin/brood
	install -m755 target/release/nest  $(PREFIX)/bin/nest
	@echo ">>> debug-armed (JIT+GUI, debug-assertions) brood+nest installed to $(PREFIX)/bin"
	@echo ">>> now reproduce with diagnostics:  BROOD_GC_VERIFY=1 nest run"
	@echo ">>> capture the '[jit-staged-stale]' / 'use-after-GC' line + .brood_crash_dump"
	@echo ">>> (run 'make install' later to restore the fast, non-armed build)"

fmt: ## Format all Rust code
	cargo fmt

clippy: ## Lint with clippy (all targets + all features; warnings are FATAL via -D warnings)
	# `--all-features` type-checks + lints the optional backends (the `gui`
	# feature: winit/softbuffer/fontdue) too, so a dependency bump that breaks
	# `gui.rs` is caught here at the gate, not at `make install`. Compile/lint
	# only — GUI *runtime* behaviour still needs an on-display check (WITH_GUI=1).
	# `-D warnings` makes warning-clean a hard gate — a new lint fails the build.
	# The deliberate style exceptions are documented `#![allow(...)]`s in
	# crates/lisp/src/lib.rs and crates/lsp/src/main.rs.
	cargo clippy --all-targets --all-features -- -D warnings

check: clippy test ## Lint + test (the pre-commit gate). Run `make fmt` separately — it rewrites files.

clean: ## Remove all build artifacts (incl. the local $(RELEASE_DIR) build); does NOT touch installed binaries in $(PREFIX)/bin
	# Wipes the whole target/ tree — every build artifact, including the local
	# release build in $(RELEASE_DIR). Installed binaries in $(PREFIX)/bin are
	# untouched (use `make uninstall` to remove those).
	cargo clean
