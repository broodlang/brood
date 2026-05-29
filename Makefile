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

.DEFAULT_GOAL := help
.PHONY: help build test bench benchmark quickbench suite repl configure install uninstall fmt clippy check clean

help: ## Show this help
	@echo "Brood — available make targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Build the whole workspace
	cargo build

test: ## Run Rust tests + the in-language suite (via cargo test)
	cargo test

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
	cargo install --path crates/cli  --force --root $(PREFIX) $(GUI_FEATURES)
	cargo install --path crates/nest --force --root $(PREFIX) $(GUI_FEATURES)
	cargo install --path crates/lsp  --force --root $(PREFIX)

uninstall: ## Remove the installed `brood`, `nest` and `brood-lsp` binaries from $(PREFIX)/bin
	cargo uninstall cli --root $(PREFIX)
	cargo uninstall nest --root $(PREFIX)
	cargo uninstall brood-lsp --root $(PREFIX)

fmt: ## Format all Rust code
	cargo fmt

clippy: ## Lint with clippy (all targets; warnings reported, not fatal)
	cargo clippy --all-targets

check: clippy test ## Lint + test (the pre-commit gate). Run `make fmt` separately — it rewrites files.

clean: ## Remove build artifacts
	cargo clean
