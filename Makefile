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
# `./configure --with-gui-gpu` (WITH_GUI_GPU) builds the wgpu render target
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
# The stdlib startup image (ADR-256/281), a default feature like the JIT and governed the
# same way: this variable only affects the `--no-default-features` release/install path,
# where `./configure --without-stdimage` (WITH_STDIMAGE=0) leaves it out so the shipped
# binary never consults or writes a cache directory. `BROOD_NO_STDIMAGE=1` is the per-run
# switch on a build that has it.
WITH_STDIMAGE ?= 1
STDIMAGE_FEATURES := $(if $(filter-out 0,$(WITH_STDIMAGE)),--features brood/stdimage,)
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
RUN_FEATURES := --no-default-features $(GUI_FEATURES) $(GUI_GPU_FEATURES) $(AUDIO_FEATURES) $(TS_FEATURES) $(JIT_FEATURES) $(STDIMAGE_FEATURES)
# INSTALL_FEATURES is RUN_FEATURES + `dev-tools`: the set for the brood/nest
# actually INSTALLED onto your PATH, where the REPL, `nest test`, `nest observe`,
# and `nest mcp` must work. `make install` builds the lean brood (embed base) AND
# these dev binaries — the embedded runtime stays lean, the tools you run don't.
INSTALL_FEATURES := $(RUN_FEATURES) --features brood/dev-tools

# The binaries `install` copies into $(PREFIX)/bin and `uninstall` removes.
BINARIES := brood nest brood-lsp

# The address-space cap every test-running step sits behind (CLAUDE.md "Build uncapped, run
# capped"): one runaway dies in seconds with its panic site named instead of eating the box.
# 16 GB, not less — the runtime reserves ~3 GB before doing any work. Put it in front of the
# RUN step only, never a build: the linker inherits it and dies with `LLVM ERROR: out of memory`.
RUN_CAP_KB := $(shell sed -n 's/^RUN_CAP_KB=//p' scripts/lib/run-cap.sh)
RUN_CAP := ulimit -v $(RUN_CAP_KB);

# The first recipe line of every target that runs the suite through cargo-nextest.
REQUIRE_NEXTEST := @command -v cargo-nextest >/dev/null 2>&1 || { echo ">>> cargo-nextest not found — run 'make ensure-nextest' (or install from https://nexte.st)"; exit 1; }

# The profiling builds get their OWN profiles, so neither overwrites $(RELEASE_DIR)/brood —
# the binary `make ab` and every timing compares against. Both inherit `release-fast`, so the
# flags match `release-brood` except for what each build exists to add.
PERF_DIR    := target/$(TARGET_SUBDIR)release-perf
SYMBOLS_DIR := target/$(TARGET_SUBDIR)release-symbols

.DEFAULT_GOAL := help
# Every target is phony — none names a file it builds. A target missing here silently does
# nothing once a file or directory of its name appears (`stress/`, `fuzz/`…); `tool_pins.rs`
# fails on one.
.PHONY: ab ab-clean ab-pin ab-vm affected-tests asan benchmark breakagetests build check \
	check-corpora check-cost check-desktop check-examples check-imaged check-stress clean clippy \
	configure doctor \
	ci-wait enable-perf-check ensure-nextest fmt fuzz gcstress gcstress-all green green-all help hooks install \
	install-features \
	latency-gate loom miri mutants perf-brood perf-symbols prepush quickbench release release-brood repl \
	rr-hunt runtime-soak shuttle smoke-bedit stress suite test test-both test-contracts test-light tier-audit tsan \
	uninstall wasm-test

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
	$(REQUIRE_NEXTEST)
	# Grammars are out of `default` (the kernel ships no language-specific parser),
	# so the suite opts into `treesit-grammars` to exercise the ruby/elixir tests.
	# The .blsp grammar tests also self-skip when absent, so a bare `cargo nextest`
	# (no grammars) stays green too.
	# The `SETUP warm-boot-cache` line comes from a nextest setup script that boots
	# `brood`/`nest` once before the fan-out (KI-38); `BROOD_NO_WARM_BOOT_CACHE=1` turns it
	# off — see .config/nextest.toml. Built uncapped, then run under $(RUN_CAP).
	# The run goes through `nextest-no-flakes.sh`, as CI's does: a test that passes only on
	# its retry fails the target and is named, instead of reading green.
	$(TEST_NICE) $(TEST_BUILD_ENV) cargo nextest run --no-run --features brood/treesit-grammars
	$(RUN_CAP) $(TEST_NICE) ./scripts/nextest-no-flakes.sh --no-fail-fast --features brood/treesit-grammars $(TEST_JOBS_FLAG)
	$(TEST_NICE) cargo test --doc   # nextest doesn't run doctests; none today, kept so future ones still run

# `make test`'s three knobs, all empty (full speed) unless `test-light` sets them: a CPU
# priority prefix, a build-job cap, and a cap on concurrent test processes.
TEST_NICE ?=
TEST_BUILD_ENV ?=
TEST_JOBS_FLAG ?=

# Concurrency cap for `test-light` — override on the command line, e.g. `make test-light LIGHT_JOBS=4`.
LIGHT_JOBS ?= 8# concurrent test PROCESSES (nextest -j) and cargo build jobs

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
	@$(MAKE) --no-print-directory test TEST_NICE='nice -n 19' \
		TEST_BUILD_ENV='env CARGO_BUILD_JOBS=$(LIGHT_JOBS)' TEST_JOBS_FLAG='-j $(LIGHT_JOBS)'

test-both: ## Run the whole suite through BOTH engines (tree-walker + VM) — the differential gate (ADR-076)
	# The VM (+ JIT) is the default; this also runs the tree-walker (BROOD_TIER=0) so a
	# regression in either is caught. `differential.rs` additionally checks per-expression
	# engine agreement within one run.
	$(REQUIRE_NEXTEST)
	# BOTH halves run whatever the first one does, and the target fails at the end if
	# either did — a gate that stops before the thing it exists to check is no gate.
	cargo nextest run --no-run
	@vm=0; tw=0; \
	 echo ">>> suite under the default ceiling (VM + JIT)"; \
	 ( $(RUN_CAP) ./scripts/nextest-no-flakes.sh --no-fail-fast ) || vm=$$?; \
	 echo ">>> suite under the tree-walker (BROOD_TIER=0)"; \
	 ( $(RUN_CAP) BROOD_TIER=0 ./scripts/nextest-no-flakes.sh --no-fail-fast ) || tw=$$?; \
	 if [ $$vm -ne 0 ] || [ $$tw -ne 0 ]; then \
	   echo ">>> test-both FAILED — VM exit $$vm, tree-walker exit $$tw"; \
	   echo ">>> (a red on ONE engine is a differential finding; a red on BOTH is usually the shared cause)"; \
	   exit 1; \
	 fi; \
	 echo ">>> test-both: both engines green"

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

check-imaged: ## Run the checker gate WITH the prelude image on (KI-106) — the feature's own tests all passed while `nest check` went red
	# A feature's own tests are shaped by what its authors thought could break; the project's
	# gate is not — so run that gate UNDER the image (KI-106). The script asserts the boot
	# took the image path: a gate that quietly ran the source path proves nothing.
	@$(RUN_CAP) ./scripts/check-imaged.sh

check-examples: ## Run every `examples/` program and fail on an unbound symbol — the gate `examples/` never had
	# `examples/` sits outside every other gate (KI-45). Asserts NO `unbound symbol` rather
	# than exit 0: servers run until killed and some examples want a peer or a display, but
	# an unbound name is never environment noise.
	@$(RUN_CAP) ./scripts/check-examples.sh

check-corpora: ## STATICALLY check examples/ stress/ fuzz-stress/ breakage/ for names that don't resolve — the paths a RUN never takes
	# The runtime gates only catch a dead name on an EXECUTED path; this static pass covers the
	# branches no run takes. Bar is zero `unbound symbol`, not zero warnings — these corpora are
	# adversarial on purpose; a genuinely dynamic global says so with `(check-allow :unbound …)`.
	@$(RUN_CAP) ./scripts/check-corpora.sh

check-stress: ## Run every `stress/` + `scripts/fuzz/stress/` harness and fail on an unbound symbol — the gate the stress dirs never had
	# Same counter as `check-examples`, one directory over: asserts NO `unbound symbol` (soaks
	# and storms never finish under a gate), except `stress/*_test.blsp`, held to passing.
	@$(RUN_CAP) ./scripts/check-stress.sh

GC_STRESS_TESTS := concurrency receive_filter receive_under_lock mailbox_order proc \
                   ref_alias supervisor gen agent pid_identity offload process_limit

gcstress: ## Run the concurrency/mailbox tests with GC_STRESS+GC_VERIFY armed — the use-after-GC hunt
	# A threshold-driven collection only catches a handle held across a collection when one
	# happens to land in the window (KI-57); BROOD_GC_STRESS collects at EVERY safepoint and
	# BROOD_GC_VERIFY catches a stale handle *stored* rather than dereferenced. Only the
	# process/mailbox-heavy files, where values cross heaps. The DEBUG build deliberately:
	# a release build with debug-assertions reported KI-57 clean on the pre-fix code.
	@echo ">>> building brood (debug — release cannot see KI-57) ..."
	cargo build -p cli
	@$(RUN_CAP) bin=target/debug/brood; fail=0; \
	for t in $(GC_STRESS_TESTS); do \
		printf ">>> %-24s " "$$t"; \
		if BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 $$bin --test tests/$$t\_test.blsp >/tmp/gcstress.$$t.log 2>&1; then \
			echo "ok"; \
		else \
			echo "FAILED"; fail=1; \
			grep -E "test failed|use-after-GC|tests," /tmp/gcstress.$$t.log | head -5 | sed 's/^/      /'; \
		fi; \
	done; \
	if [ $$fail -ne 0 ]; then echo ">>> gcstress: FAILURES above"; exit 1; fi; \
	echo ">>> gcstress: all clean"

gcstress-all: ## EVERY tests/*_test.blsp under GC_STRESS+GC_VERIFY (debug build, ~80 s at -P 6) — `gcstress` widened past the twelve process/mailbox files. Nightly + green-all, not per push
	cargo build -p cli -p nest
	@./scripts/gcstress-all.sh

test-contracts: ## The in-language suite with every `sig` armed as a runtime contract — what `nest test` users get (ADR-381); `make test`'s wrapper runs it unarmed to stay inside its 5-min budget
	cargo build --release -p cli -p nest
	@./scripts/test-contracts.sh

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
	@$(RUN_CAP) bin=target/release/brood; fail=0; \
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
		out=$$(env $$pre $$bin --test "$$f" 2>&1); rc=$$?; \
		printf '%s\n' "$$out"; \
		if printf '%s' "$$out" | grep -qE 'correct[^:]*: *false'; then \
			rc=1; \
			echo ">>> WRONG RESULT ($$f): a self-check printed 'correct: false'"; \
		fi; \
		if [ $$rc -ne 0 ]; then \
			fail=1; \
			if [ $$rc -gt 128 ]; then echo ">>> CRASH ($$f): exit $$rc (signal $$((rc-128)) — likely SIGSEGV / stack overflow)"; \
			else echo ">>> FAIL ($$f): exit $$rc"; fi; \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -ne 0 ]; then echo ">>> breakage suite: FAILURES above"; exit 1; \
	else echo ">>> breakage suite: all files passed"; fi

# The cargo-nextest CI installs. `crates/lisp/tests/tool_pins.rs` fails when this and the
# workflows' `cargo-nextest@…` pins disagree.
NEXTEST_VERSION := 0.9.150

ensure-nextest: ## Install CI's pinned cargo-nextest into ~/.local/bin (prebuilt binary) unless that version is already there
	@cargo nextest --version 2>/dev/null | grep -qw "$(NEXTEST_VERSION)" && { echo "cargo-nextest $(NEXTEST_VERSION) already installed"; } || { \
		echo "installing cargo-nextest $(NEXTEST_VERSION) into $(HOME)/.local/bin ..."; \
		mkdir -p $(HOME)/.local/bin; \
		curl -LsSf https://get.nexte.st/$(NEXTEST_VERSION)/linux | tar zxf - -C $(HOME)/.local/bin; \
		echo "installed: $$(cargo nextest --version | head -1)"; }

benchmark: ## Run benchmarks; archive results to docs/benchmarks/<timestamp>.md
	./scripts/bench.sh $(ARGS)

quickbench: ## Fast (~10s) benchmark for iteration — no archive, few samples
	./scripts/bench.sh --quick $(ARGS)

ab: ## A/B the working tree against a git ref on the cross-language rows: make ab BASE=<ref> ROWS="fib pfib" N=7
	./scripts/ab-bench.sh $(if $(BASE),-b $(BASE),) $(if $(N),-n $(N),) $(ROWS) $(ARGS)

green: ## Answer one question honestly: is this tree green? (completed CI runs + the gates `make check` skips)
	# A cancelled CI run is not evidence, and `make check` is not what CI runs (KI-68/KI-69).
	# `--local` skips the CI half, `--remote` skips the local half.
	@./scripts/green.sh $(ARGS)

ci-wait: ## Wait for CI's verdict on REF (default HEAD) — through cancellations, counting a later run that contains it — and name what failed
	@./scripts/ci-wait.sh $(REF)

green-all: check-examples check-stress check-imaged smoke-bedit tier-audit wasm-test test-contracts gcstress-all latency-gate ## `make green` plus clippy (CI's flags), the two slow .blsp corpus gates, the bedit smoke, the tier audit, the wasm behavioural suite, the suite with contracts armed, every test file under GC stress, and the latency gate
	@./scripts/green.sh --clippy
	@echo "green-all: clippy, the .blsp gates, the bedit smoke, the tier audit, the wasm suite, the armed-contract suite, the whole-suite GC stress and the latency gate passed too. Still not run: make test, breakage, tree-walker differential."

tier-audit: release-brood ## Every benchmark row under BROOD_JIT_BAIL_TRACE — fail on any arm latched off the native tier or any lowering bug (KI-132's class)
	# Every other gate is a value gate, and the KI-132 class is right-but-slow: an arm that
	# deopts per activation is latched onto the interpreter after sixteen, and nothing but a
	# benchmark ever noticed. Finds the benchmark checkout with scripts/bench-dir.sh (BENCH_DIR overrides).
	@./scripts/tier-audit.sh

wasm-test: ## Run the wasm32 cooperative scheduler BEHAVIOURALLY (build + wasm-bindgen + node)
	# The native suite cannot cover the cooperative scheduler by construction (off wasm there
	# are OS workers and a real clock), so this runs the real artifact under node.
	# Needs: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli
	# --version <the pin in crates/playground/Cargo.toml>; node.
	@./scripts/wasm-suite.sh

smoke-bedit: ## Run bedit's gates (nest check, --check-boot, nest test) against THIS tree's nest — the downstream smoke CI's `downstream-bedit` job runs
	# bedit is where a brood regression surfaces first. Uses ../bedit (or BEDIT_DIR) and the
	# NEWEST nest under target/ (or NEST=...); a missing checkout is a note, not a failure.
	# "Newest" is not "most capable": a grammar-less debug nest fails bedit's ruby/elixir
	# mode tests, which reads as a regression — pass a treesit-grammars NEST=... explicitly.
	@./scripts/smoke-bedit.sh $(ARGS)

doctor: ## Report the things that make a measurement or a gate lie (build drift, strays, boot cache, litter)
	# Read this BEFORE trusting a benchmark delta or a green gate. Every check maps to a
	# class that has cost a real session — chiefly build drift, because a stale binary fails
	# by AGREEING with the baseline (an A/B reads +0.0%, a flag sweep reads 1.0x on every
	# row, a lowering-witness diff comes back empty). `--strict` exits 1 on any finding.
	@./scripts/doctor.sh $(ARGS)

ab-pin: ## A/B against a PINNED baseline binary that survives ab-clean: make ab-pin BASE=<ref> ROWS="pipeline nqueens" N=15
	# `make ab` rebuilds its baseline in a throwaway worktree, so runs on different days
	# measure against different binaries and a few-percent row cannot be told from drift.
	# This keeps the baseline in target/ab-pinned/<sha>/brood (gitignored, NOT under target/ab/)
	# and runs base-vs-base as the floor, which is the method CLAUDE.md prescribes.
	N=$(or $(N),15) ./scripts/ab-pin.sh $(or $(BASE),HEAD) $(or $(ROWS),pipeline nqueens)

latency-gate: release-brood ## Tail latency on THIS box: an open-loop request probe, run 5x — fails on quanta >1 ms (code that stopped being preemptible) or a worst p99 over budget. Not for CI (docs/benchmarking.md)
	./scripts/latency-gate.sh $(or $(RUNS),5)

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

check-cost: ## Instruction cost of `brood --check` per program, this tree vs BASE (callgrind, deterministic): make check-cost BASE=<ref> [SCAN_ALIVE=1]
	@./scripts/check-cost.sh --base $(or $(BASE),HEAD) $(if $(SCAN_ALIVE),--scan-alive,)

suite: ## Run the in-language suite via the project runner (discovers tests/**/*_test.blsp)
	cargo build -q -p nest
	$(RUN_CAP) target/debug/nest test

stress: build ## The occasional BIG stress run (property/differential/race tests, 3 engines) — not part of CI
	$(RUN_CAP) ./stress/run.sh

runtime-soak: ## Boot the concurrency soak fresh, round after round, for SOAK_SECONDS (default 600) — the KI-88/KI-127 hunt
	RUSTFLAGS="-C debug-assertions=on" cargo build --release -p cli
	$(RUN_CAP) ./stress/runtime_soak.sh $(or $(SOAK_SECONDS),600)

# ASAN_OPTIONS=symbolize=0 because the system llvm-symbolizer stalls ~90 s at
# EVERY exit against the 65 MB sancov binary (found 2026-07-23 — it made the
# loop look like 4 execs/min). A crash artifact can be re-run symbolized:
#   cd crates/lisp && cargo +nightly fuzz run <T> fuzz/artifacts/<T>/<file>
fuzz: ## Run one libFuzzer target briefly: make fuzz T=wire SECS=60 (targets: reader eval json wire bundle)
	cd crates/lisp && ASAN_OPTIONS=symbolize=0 cargo +nightly fuzz run $(or $(T),reader) -- -max_total_time=$(or $(SECS),60) -rss_limit_mb=4096

# What both sanitizer targets run under their own RUSTFLAGS: an instrumented std, and the system
# allocator so the sanitizer sees allocations mimalloc would hide in its un-instrumented arena.
SANITIZER_TEST := cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu -p brood --release --features brood/system-alloc

tsan: ## ThreadSanitizer over the concurrency-sensitive Rust tests (needs nightly + rust-src; system-alloc feature so mimalloc's un-instrumented internals don't report phantom races)
	RUSTFLAGS="-Zsanitizer=thread" $(SANITIZER_TEST) --test table_tsan --test concurrency_race --test preemption --test live_migration --test local_send_race --test gc

loom: ## Loom model-check of the dense-table migration protocol (exhaustive interleavings of a faithful model)
	cargo test -p brood --release --features brood/loom-model --test loom_table_protocol

shuttle: ## Shuttle randomized (random + PCT) model-check of the scheduler's park/spawn handshake and steal ownership (a faithful miniature; each property's sabotage is a test too)
	cargo test -p brood --release --features brood/shuttle-model --test shuttle_scheduler_model

# The kernel modules whose `unsafe` Miri can interpret: no JIT, no mmap beyond what it models,
# no foreign calls (hence the system allocator — mimalloc is C). `local_string` is left out:
# its tests loop past what Miri, ~1000x slower than native, finishes in 15 minutes.
MIRI_MODULES ?= core::value core::alloc core::map_champ core::table core::heap::slabs \
	core::heap::roots_buf core::heap::small_map
miri: ## Miri (undefined-behaviour interpreter) over the kernel unit tests that hold `unsafe` (needs nightly + the miri component; MIRI_MODULES narrows)
	@for m in $(MIRI_MODULES); do \
	  echo ">>> miri $$m"; \
	  MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test -q -p brood --lib \
	    --no-default-features --features system-alloc $$m || exit 1; \
	done

# Mutation testing: cargo-mutants breaks one thing at a time in F (flips a comparison, deletes
# a statement, returns a default) and reports every change no test noticed — CLAUDE.md's
# "sabotage every guard" rule, run by a tool. Scope it: the whole crate is hours. TESTS is
# the cargo-test filter to run per mutant (the module's own tests, by default all of `--lib`).
# Its build copy goes under target/ (TMPDIR): /tmp is a RAM disk here, and a copied brood
# build filled it until the linker died of SIGBUS. The report lands in target/mutants.out.
F ?=
TESTS ?=
mutants: ## Mutation-test one Rust file: make mutants F=crates/lisp/src/core/heap/small_map.rs TESTS=core::heap::small_map (needs cargo-mutants)
	@test -n "$(F)" || { echo "usage: make mutants F=path/to/file.rs [TESTS=test-filter]"; exit 2; }
	@mkdir -p target/mutants-tmp
	TMPDIR=$(CURDIR)/target/mutants-tmp cargo mutants --no-shuffle -p brood -f $(F) \
		--cargo-arg=--lib -o target -- $(TESTS)

# rr records a run so a one-in-N failure can be replayed exactly, and stepped backwards.
# Chaos mode perturbs scheduling so a rare interleaving shows up sooner. Needs `rr` installed
# (apt install rr) and kernel.perf_event_paranoid <= 1.
RR_TEST ?=
RR_RUNS ?= 50
RR ?= rr
rr-hunt: ## Record a test binary under `rr record --chaos` until it fails, keeping that recording: make rr-hunt RR_TEST="target/debug/deps/brood-HASH case_name" RR_RUNS=50 [RR=/path/to/rr, 5.10+ on glibc 2.42+]
	@RR="$(RR)" scripts/rr-hunt.sh $(RR_RUNS) $(RR_TEST)

asan: ## AddressSanitizer over the kernel-exercising Rust tests (needs nightly + rust-src + cargo-nextest; system-alloc so ASAN can intercept allocations instead of mimalloc's un-instrumented arena). Catches genuine OOB / use-after-free in the unsafe substrate (mmap table, JIT codegen buffers) that TSAN and the logical GC tripwires miss. Through nextest, which also skips doctests (they don't LINK under ASAN + build-std, a toolchain quirk).
	# `--cfg brood_asan` sizes `WORKER_STACK_BYTES` to 64 MiB for this build, because ASAN's
	# redzones roughly double every Rust frame: the instrumented prelude boot measured
	# 16 768 736 bytes against the ordinary 16 MiB stack. Raise the stack, never
	# BROOD_STACK_BUDGET: a budget larger than the stack disables the stale-base backstop.
	# No $(RUN_CAP) here or on tsan: the sanitizers reserve terabytes of shadow address space.
	# nextest, process per test, like every other run of these binaries: under `cargo test`
	# the integration tests shared one process and its process-wide scheduler knobs, and
	# `preemption_shapes` failed for no reason ASAN found. CARGO_UNSTABLE_BUILD_STD is
	# `-Zbuild-std`, which nextest does not pass through as a flag.
	# Line tables (CARGO_PROFILE_RELEASE_DEBUG) on both steps, so a leak or fault report names
	# its frames; the two steps must agree or the second rebuilds.
	# The `brood` and `nest` binaries first, into the same target directory: suite tests run
	# a child beside the harness (`sibling-binary`, a doctest markdown block, a terminal app
	# under `nest run`), and with `-p brood` alone there was none to find.
	$(REQUIRE_NEXTEST)
	CARGO_UNSTABLE_BUILD_STD=std CARGO_PROFILE_RELEASE_DEBUG=line-tables-only RUSTFLAGS="-Zsanitizer=address --cfg brood_asan" \
		cargo +nightly build --release --target x86_64-unknown-linux-gnu \
		-p cli -p nest --features brood/system-alloc
	CARGO_UNSTABLE_BUILD_STD=std CARGO_PROFILE_RELEASE_DEBUG=line-tables-only RUSTFLAGS="-Zsanitizer=address --cfg brood_asan" \
		cargo +nightly nextest run --profile asan --no-fail-fast --target x86_64-unknown-linux-gnu \
		-p brood --release --features brood/system-alloc

repl: ## Start the REPL
	$(CLI)

configure: ## Show current build options (./configure --with-gui to enable the GUI)
	@echo "PREFIX        = $(PREFIX)"
	@echo "WITH_GUI      = $(WITH_GUI)"
	@echo "WITH_GUI_GPU  = $(WITH_GUI_GPU)"
	@echo "WITH_AUDIO    = $(WITH_AUDIO)"
	@echo "WITH_JIT      = $(WITH_JIT)"
	@echo "WITH_STDIMAGE = $(WITH_STDIMAGE)"
	@echo "install features: $(INSTALL_FEATURES)"
	@echo "Run ./configure --with-gui to enable the native window; ./configure --help for more."

install-features: ## Print INSTALL_FEATURES — what CI builds the `make install` toolchain with, so the set is spelled once
	@echo $(INSTALL_FEATURES)

release-brood: ## Build ONLY the `brood` binary into $(RELEASE_DIR) — the perf-A/B build step (skips nest + brood-lsp)
	# Exactly the flags `release` uses for the binary that RUNS user code, and
	# nothing else: `scripts/ab-bench.sh` builds both sides of an A/B through this
	# target so the two binaries cannot differ in profile or features. Note the
	# package is `cli` (the binary), NOT `-p brood` (the lib) — `-p brood` does not
	# relink $(RELEASE_DIR)/brood and silently benchmarks a stale binary.
	RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-fast -p cli $(RUN_FEATURES) $(CARGO_TARGET)

perf-symbols: ## Build an UNSTRIPPED `brood` for `perf` into $(SYMBOLS_DIR) (release-fast strips every symbol)
	# `perf` on a `release-fast` binary reports bare addresses (that profile strips). Same
	# flags as `release-brood` otherwise, in its own profile so the timing binary is untouched.
	# Needs `perf` permitted — `make enable-perf-check` says whether it is.
	RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-symbols --bin brood \
		$(RUN_FEATURES) $(CARGO_TARGET) \
		--config 'profile.release-symbols.inherits="release-fast"' \
		--config 'profile.release-symbols.strip=false' \
		--config 'profile.release-symbols.debug=1'
	@echo
	@echo "  perf record --call-graph dwarf -- $(SYMBOLS_DIR)/brood bench.blsp"
	@echo "  perf report --no-children      # self time, which the frontier tables quote"

enable-perf-check: ## Report whether `perf` can profile here (no root, no changes)
	@./scripts/enable-perf.sh --check

perf-brood: ## Build a counter-armed `brood` into $(PERF_DIR) — the attribution build ((perf/report), BROOD_PERF_STATS, BROOD_DEOPT_TRACE)
	# The VM work-attribution counters are a cargo feature (`diagnostics/perf.rs`), so a normal
	# binary cannot answer "where does the time go". Flags match `release-brood` except for the
	# added feature, in its own profile so the timing binary is never the counter-armed one.
	# NOT for timing: the counters are atomics on the hot path (docs/benchmarking.md).
	RUSTFLAGS="$(PERF_RUSTFLAGS)" cargo build --profile release-perf -p cli \
		--config 'profile.release-perf.inherits="release-fast"' \
		$(INSTALL_FEATURES) --features brood/perf-stats $(CARGO_TARGET)
	@echo "counter-armed brood: $(PERF_DIR)/brood"
	@echo "  BROOD_PERF_STATS=1 $(PERF_DIR)/brood prog.blsp   # dump counters at exit"
	@echo "  (perf/summary)                                   # in-image triage"

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
	@mkdir -p $(PREFIX)/bin
	for binary in $(BINARIES); do install -m755 $(RELEASE_DIR)/$$binary $(PREFIX)/bin/$$binary || exit 1; done
	# Build the standard-library startup image for the binaries just installed (ADR-218).
	# Without it every `require` of a std module re-evaluates its embedded source — `format`
	# alone is ~77 ms — and a program pulling http+json+format+datetime pays ~124 ms instead
	# of ~27 ms. Keyed on `build-id`, which includes each executable's mtime, so the freshly
	# installed binaries need their own; that is exactly why this runs here.
	# Best-effort: a failure must not fail the install, since everything still works from
	# source, just slower.
	-$(PREFIX)/bin/nest stdimage

uninstall: ## Remove the installed binaries from $(PREFIX)/bin (leaves the local $(RELEASE_DIR) build intact)
	# Removes only what `install` placed on the system — the local release build
	# in $(RELEASE_DIR) is left alone (use `make clean` to remove that).
	rm -f $(addprefix $(PREFIX)/bin/,$(BINARIES))

fmt: ## Format all Rust code
	cargo fmt

prepush: ## Run the pre-push hook's gates by hand (rustfmt, the BROOD_* flag catalogue; with a .blsp involved: nest format --check + the tests naming what changed, the surface audit and the doc examples)
	@sh scripts/git-hooks/pre-push && echo "prepush: clean"

affected-tests: ## Print (don't run) the tests that name what this change touches: .blsp files for `nest test`, `cargo:` lines for nextest
	@sh scripts/affected-tests.sh

check-desktop: ## The static gates CI runs, safe on a workstation (fmt, the BROOD_* flag catalogue, nest format, nest check + --strict, clippy), plus TEST=tests/x_test.blsp if given; push after it with --no-verify
	cargo fmt --all --check
	scripts/check-flag-catalogue.sh
	cargo build -p nest
	$(RUN_CAP) target/debug/nest format --check
	$(RUN_CAP) target/debug/nest check $$(find std tests examples -name '*.blsp' | sort)
	$(RUN_CAP) target/debug/nest check --strict $$(find std tests examples -name '*.blsp' | sort)
	cargo clippy --all-targets --all-features -- -D warnings
	$(if $(TEST),$(RUN_CAP) target/debug/nest test $(TEST))
	@echo "check-desktop: clean"

hooks: ## Install the local git pre-push hook (`make prepush` on every push, before CI sees it)
	# Opt-in and local: the hook lives in scripts/git-hooks/ so it is version-controlled
	# and survives a fresh clone, but nothing installs it for you. Always: `cargo fmt
	# --check` and the BROOD_* flag catalogue (under a second). With a .blsp involved:
	# `nest format --check` (whole-project by design, ~50s) and the tests that name the
	# changed modules plus the surface audit and the executed doc examples (~1 min) —
	# so a Rust-only push stays under a second. `make prepush` runs the same script.
	@mkdir -p .git/hooks
	@if [ -e .git/hooks/pre-push ] && ! cmp -s scripts/git-hooks/pre-push .git/hooks/pre-push; then \
		cp .git/hooks/pre-push .git/hooks/pre-push.bak; \
		echo "existing hook backed up to .git/hooks/pre-push.bak"; \
	fi
	@cp scripts/git-hooks/pre-push .git/hooks/pre-push && chmod +x .git/hooks/pre-push
	@echo "installed .git/hooks/pre-push (bypass a single push with --no-verify)"
	# A global `core.hooksPath` REPLACES `.git/hooks`, so with one set the hook just
	# installed never runs. Say so rather than let `installed …` read as armed; the fix is
	# a chaining hook in that directory, or `make prepush` by hand.
	@hp=$$(git config --get core.hooksPath || true); \
	if [ -n "$$hp" ]; then \
		echo ""; \
		echo "WARNING: core.hooksPath is set to '$$hp' — git will NOT run .git/hooks/pre-push."; \
		if [ -x "$$hp/pre-push" ]; then \
			echo "  '$$hp/pre-push' exists; make sure it chains to the repo hook, or this gate is inert."; \
		else \
			echo "  There is no pre-push there, so THIS GATE IS INERT. Fix it once, for every repo:"; \
			echo "      cp scripts/git-hooks/global-pre-push '$$hp/pre-push' && chmod +x '$$hp/pre-push'"; \
			echo "  Until then, run 'make prepush' by hand before each push."; \
		fi; \
	fi

clippy: ## Lint with clippy (all targets + all features; warnings are FATAL via -D warnings)
	# `--all-features` type-checks + lints the optional backends (the `gui`
	# feature: winit/softbuffer/fontdue) too, so a dependency bump that breaks
	# `host/gui.rs` is caught here at the gate, not at `make install`. Compile/lint
	# only — GUI *runtime* behaviour still needs an on-display check (WITH_GUI=1).
	# `-D warnings` makes warning-clean a hard gate — a new lint fails the build.
	# The deliberate style exceptions are documented `#![allow(...)]`s in
	# crates/lisp/src/lib.rs and crates/lsp/src/main.rs.
	cargo clippy --all-targets --all-features -- -D warnings
	# A superset hides a field only a narrower feature leaves unread: lint the configured
	# release's `gui` without `gui-gpu` too (CI's lint job runs the same).
	cargo clippy -p brood --lib --features gui -- -D warnings

check: clippy test ## Lint + test (the pre-commit gate). Run `make fmt` separately — it rewrites files.

clean: ## Remove all build artifacts (incl. the local $(RELEASE_DIR) build); does NOT touch installed binaries in $(PREFIX)/bin
	# Wipes the whole target/ tree — every build artifact, including the local
	# release build in $(RELEASE_DIR). Installed binaries in $(PREFIX)/bin are
	# untouched (use `make uninstall` to remove those).
	cargo clean
