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
# JIT (ADR-101): the tier-1 template JIT, ON by default — hot compute loops run as
# native code, and it's compiled out (zero cost) only when disabled. `make install`
# defaults it on even without ./configure; `./configure --without-jit` (WITH_JIT=0)
# opts out for an unsupported host or a minimal build. Baked into the binaries that
# run user code (brood, nest); the LSP doesn't run hot user code, so it's left out.
WITH_JIT ?= 1
JIT_FEATURES := $(if $(filter-out 0,$(WITH_JIT)),--features brood/jit,)
# tree-sitter (foreign-language editor modes — ruby/elixir, ROADMAP §C) is a
# baseline runtime capability a modern editor needs, so the lean install always
# bakes it in. `make install` builds `--no-default-features`, so it's named here
# explicitly (cargo unions repeated `--features` flags, so this composes with
# GUI_FEATURES). Unlike the windowing stack it is NOT gated on configure.
TS_FEATURES := --features brood/treesit

.DEFAULT_GOAL := help
.PHONY: help build test ensure-nextest bench benchmark quickbench suite repl configure install uninstall fmt clippy check clean

help: ## Show this help
	@echo "Brood — available make targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Build the whole workspace
	cargo build

test: ## Run Rust tests + the in-language suite via cargo-nextest (each test case process-isolated and hard-capped at 2 min — see .config/nextest.toml)
	# nextest runs each test in its own process: a single hung case is killed at the
	# 2-min per-case cap (and a SIGSEGV — Brood's stack-overflow failure mode — is
	# contained to that case instead of aborting the whole binary). `--no-fail-fast`
	# surfaces every result. Install: `make ensure-nextest` (or see https://nexte.st).
	@command -v cargo-nextest >/dev/null 2>&1 || { echo ">>> cargo-nextest not found — run 'make ensure-nextest' (or install from https://nexte.st)"; exit 1; }
	cargo nextest run --no-fail-fast
	cargo test --doc   # nextest doesn't run doctests; none today, kept so future ones still run

test-both: ## Run the whole suite through BOTH engines (tree-walker + VM) — the differential gate (ADR-076)
	# The VM is the default engine; this also exercises the tree-walker escape hatch
	# (BROOD_VM=0) so a regression in either is caught. `differential.rs` additionally
	# checks per-expression engine agreement within one run.
	@command -v cargo-nextest >/dev/null 2>&1 || { echo ">>> cargo-nextest not found — run 'make ensure-nextest'"; exit 1; }
	@echo ">>> suite under the VM (default engine)"
	BROOD_VM=1 cargo nextest run --no-fail-fast
	@echo ">>> suite under the tree-walker (BROOD_VM=0 escape hatch)"
	BROOD_VM=0 cargo nextest run --no-fail-fast

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

suite: ## Run the in-language suite via the project runner (discovers tests/**/*_test.blsp)
	$(NEST) test

repl: ## Start the REPL
	$(CLI)

configure: ## Show current build options (./configure --with-gui to enable the GUI)
	@echo "PREFIX   = $(PREFIX)"
	@echo "WITH_GUI = $(WITH_GUI)$(if $(GUI_FEATURES), (GUI backend on),)"
	@echo "Run ./configure --with-gui to enable the native window; ./configure --help for more."

install: ## Install `brood`, `nest` and `brood-lsp` into $(PREFIX)/bin (./configure --with-gui first for the window)
	# Force a clean *performance* build: append `-C debug-assertions=off
	# -C overflow-checks=off` to any ambient RUSTFLAGS. rustc takes the LAST
	# `-C <key>=` for a key, so this wins even if the GC-debug build mode
	# (`RUSTFLAGS="-C debug-assertions=on"`, see CLAUDE.md) is exported in the
	# shell — so the installed binary is never accidentally debug-armed (which
	# would carry the GC tripwire/verifier overhead and skew benchmarks).
	# Build the single lean (+gui if `./configure --with-gui`) runtime that `nest`
	# embeds, so `nest release` ships a self-contained app with NO Rust at release
	# time (ADR-038, docs/release.md). `--no-default-features` strips the dev/debug
	# surface; `$(GUI_FEATURES)` adds `--features brood/gui` when GUI is configured.
	RUSTFLAGS="$(RUSTFLAGS) -C debug-assertions=off -C overflow-checks=off" cargo build --profile release-lean --no-default-features -p cli $(GUI_FEATURES) $(TS_FEATURES) $(JIT_FEATURES)
	RUSTFLAGS="$(RUSTFLAGS) -C debug-assertions=off -C overflow-checks=off" cargo install --path crates/cli  --force --root $(PREFIX) $(GUI_FEATURES) $(TS_FEATURES) $(JIT_FEATURES)
	# Bake the runtime built above into `nest` (crates/nest/build.rs reads BROOD_EMBED_RUNTIME).
	BROOD_EMBED_RUNTIME=$(CURDIR)/target/release-lean/brood RUSTFLAGS="$(RUSTFLAGS) -C debug-assertions=off -C overflow-checks=off" cargo install --path crates/nest --force --root $(PREFIX) $(GUI_FEATURES) $(TS_FEATURES) $(JIT_FEATURES)
	RUSTFLAGS="$(RUSTFLAGS) -C debug-assertions=off -C overflow-checks=off" cargo install --path crates/lsp  --force --root $(PREFIX)

uninstall: ## Remove the installed `brood`, `nest` and `brood-lsp` binaries from $(PREFIX)/bin
	cargo uninstall cli --root $(PREFIX)
	cargo uninstall nest --root $(PREFIX)
	cargo uninstall brood-lsp --root $(PREFIX)

fmt: ## Format all Rust code
	cargo fmt

clippy: ## Lint with clippy (all targets + all features; warnings reported, not fatal)
	# `--all-features` type-checks + lints the optional backends (the `gui`
	# feature: winit/softbuffer/fontdue) too, so a dependency bump that breaks
	# `gui.rs` is caught here at the gate, not at `make install`. Compile/lint
	# only — GUI *runtime* behaviour still needs an on-display check (WITH_GUI=1).
	cargo clippy --all-targets --all-features

check: clippy test ## Lint + test (the pre-commit gate). Run `make fmt` separately — it rewrites files.

clean: ## Remove build artifacts
	cargo clean
