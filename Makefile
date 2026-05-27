# Convenience wrapper around the Cargo workspace. Cargo remains the source of
# truth — these targets just give short, memorable commands. Pass extra args
# with ARGS=..., e.g. `make benchmark ARGS=sum_tail`.

CLI := cargo run -q -p cli
ARGS ?=

.DEFAULT_GOAL := help
.PHONY: help build test bench benchmark suite repl install fmt clippy check clean

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

suite: ## Run the in-language suite via the project runner (discovers tests/**/*_test.blsp)
	$(CLI) test

repl: ## Start the REPL
	$(CLI)

install: ## Install the `brood` binary (REPL, file runner, `brood test`) onto PATH
	cargo install --path crates/cli --force

fmt: ## Format all Rust code
	cargo fmt

clippy: ## Lint with clippy (all targets; warnings reported, not fatal)
	cargo clippy --all-targets

check: clippy test ## Lint + test (the pre-commit gate). Run `make fmt` separately — it rewrites files.

clean: ## Remove build artifacts
	cargo clean
