#!/usr/bin/env bash
# Run the Brood benchmarks and archive the results, with full environment
# metadata, into docs/benchmarks/<UTC-timestamp>.md. Benchmark numbers are only
# meaningful alongside the machine and commit they came from — so every run is
# stamped with arch, CPU, toolchain, and git state.
#
# Usage: scripts/bench.sh [extra args passed through to the bench binary]
#   e.g. scripts/bench.sh sum_tail      # only run benches matching "sum_tail"
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

outdir="docs/benchmarks"
mkdir -p "$outdir"

stamp_utc="$(date -u +%Y-%m-%dT%H-%M-%SZ)"   # filename-safe (no colons)
outfile="$outdir/$stamp_utc.md"

# --- gather environment metadata ------------------------------------------
arch="$(uname -m)"
kernel="$(uname -sr)"
host="$(hostname)"
cpu="$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || echo unknown)"
cores="$(nproc 2>/dev/null || echo '?')"
rustc_v="$(rustc --version)"
cargo_v="$(cargo --version)"
divan_v="$(awk '/name = "divan"/{f=1} f&&/version/{gsub(/[",]/,"",$3); print $3; exit}' Cargo.lock)"
commit="$(git rev-parse --short HEAD)"
branch="$(git rev-parse --abbrev-ref HEAD)"
if [ -n "$(git status --porcelain)" ]; then
  tree_state="dirty (uncommitted changes — results not reproducible from commit alone)"
else
  tree_state="clean"
fi

# --- run the benchmarks, capturing plain-text output ----------------------
# Not a tty when captured, so divan emits no color; strip any ANSI just in case.
echo "Running benchmarks (output -> $outfile) ..." >&2
raw="$(NO_COLOR=1 cargo bench --benches -- "$@" 2>&1)" || {
  echo "$raw" >&2
  echo "benchmark run failed; not writing $outfile" >&2
  exit 1
}
# Strip ANSI, drop the empty lib/bin unit-test harness sections that `cargo
# bench` also runs, and squeeze the blank lines they leave behind.
clean="$(printf '%s\n' "$raw" \
  | sed -E 's/\x1b\[[0-9;]*m//g' \
  | grep -vE 'Running unittests|^running 0 tests$|^test result: ok\. 0 ' \
  | cat -s)"

# --- write the results file -----------------------------------------------
{
  echo "# Benchmark run — $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
  echo
  echo "| | |"
  echo "|---|---|"
  echo "| Date (UTC) | $(date -u '+%Y-%m-%d %H:%M:%S') |"
  echo "| Host | \`$host\` |"
  echo "| Arch | \`$arch\` |"
  echo "| CPU | $cpu ($cores logical cores) |"
  echo "| OS / kernel | $kernel |"
  echo "| rustc | ${rustc_v#rustc } |"
  echo "| cargo | ${cargo_v#cargo } |"
  echo "| divan | $divan_v |"
  echo "| Profile | \`bench\` (release, opt-level 3) |"
  echo "| Git commit | \`$commit\` (branch \`$branch\`) |"
  echo "| Working tree | $tree_state |"
  echo "| Command | \`cargo bench --benches -- $*\` |"
  echo
  echo '```'
  printf '%s\n' "$clean"
  echo '```'
} > "$outfile"

echo "Wrote $outfile" >&2
