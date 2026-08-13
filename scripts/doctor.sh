#!/usr/bin/env bash
# `make doctor` — check the things that make a measurement or a test run lie.
#
# Every check here corresponds to a class that has actually cost a session. None of them
# fail a build; the point is that they are *silent* otherwise. Read the report before
# trusting a benchmark delta or a green gate.
#
#   1. BUILD DRIFT — the biggest one. There are three brood binaries on a dev box
#      (target/release, target/release-fast, $PREFIX/bin) and a stale one fails by
#      *agreeing with the baseline*, so an A/B reads +0.0% and a flag sweep reads 1.0x on
#      every row. A `brood` predating a flag simply ignores it: a `BROOD_TIER` sweep against
#      a pre-ADR-222 binary reports no difference between the tiers, which reads as a
#      finding rather than a mistake (2026-08-13).
#   2. STRAY PROCESSES — a long-lived `brood`/`nest`/monitor left by a previous test run or
#      debugging session burns CPU and skews every subsequent timing. KI-29 was the test
#      -orphan version of this (one child found alive 9 days later); an interactive session
#      leaving a sampler running is the other.
#   3. BOOT-CACHE STATE — the expanded-prelude cache is keyed on the executable's mtime, so
#      every rebuild colds it. A cold boot is ~1.2 s against ~16 ms warm (KI-38), which is
#      larger than most benchmark rows.
#   4. DISK LITTER — `make ab` worktrees and temp dirs (KI-30: 4484 dirs / 168 MB).
#
# Exit status is 0 unless --strict is passed, which makes any finding exit 1 (for CI).
set -u

strict=0
[ "${1:-}" = "--strict" ] && strict=1
findings=0
note() { printf '  \033[33m!\033[0m %s\n' "$1"; findings=$((findings+1)); }
ok()   { printf '  \033[32mok\033[0m %s\n' "$1"; }

cd "$(dirname "$0")/.." || exit 1
head_sha=$(git rev-parse --short HEAD 2>/dev/null || echo "?")

echo "brood doctor — HEAD is $head_sha"
echo
echo "1. build drift"

# The sha a binary reports (`brood --version` prints "brood <ver> (<sha>)"), or "" if the
# binary is old enough not to report one at all — which is itself the finding.
binary_sha() { "$1" --version 2>/dev/null | sed -n 's/.*(\([0-9a-f]\{7,\}\)).*/\1/p'; }

for b in target/release-fast/brood target/release/brood "$(command -v brood 2>/dev/null)"; do
  [ -n "$b" ] && [ -x "$b" ] || continue
  sha=$(binary_sha "$b")
  if [ -z "$sha" ]; then
    note "$b reports no build sha — predates the sha in --version; cannot be checked, rebuild it"
  elif [ "$sha" != "$head_sha" ]; then
    note "$b is built from $sha, HEAD is $head_sha — it will silently ignore anything newer"
  else
    ok "$b matches HEAD"
  fi
done

# A binary older than the newest source is stale even when its sha matches (uncommitted work).
for b in target/release-fast/brood target/release/brood; do
  [ -x "$b" ] || continue
  newer=$(find crates -name '*.rs' -newer "$b" -print -quit 2>/dev/null || true)
  [ -n "$newer" ] && note "$b is older than $newer — rebuild before measuring"
done

echo
echo "2. stray processes"
# Anything running our binaries for more than an hour is almost certainly a leftover: the
# whole test suite is ~10 min and no benchmark row runs that long.
# Match the EXECUTABLE ($4, the first args token), not the whole line: a shell whose cwd
# happens to be .../broodlang/brood matches any line-wide pattern and buries the real finding.
strays=$(ps -eo pid,etimes,pcpu,args 2>/dev/null \
  | awk '$2 > 3600 {
      exe = $4; sub(/.*\//, "", exe);
      # Daemons are SUPPOSED to be long-lived — a language server, or the MCP server,
      # legitimately runs for days. Flagging them trains you to ignore this section, so
      # they are excluded by subcommand rather than reported and explained away.
      # (No apostrophes in here: the awk program is inside a single-quoted shell string.)
      if ($5 ~ /^(mcp|lsp|serve|node|observe|watch)$/) next;
      if (exe == "brood-lsp") next;
      if (exe == "brood" || exe == "nest" || exe == "sysmon.py" || exe == "harness.py")
        print;
      else if (exe ~ /^(python3?|sh|bash)$/ ) {
        # An interpreter: judge it by the script it was handed ($5), same basename rule.
        scr = $5; sub(/.*\//, "", scr);
        if (scr == "sysmon.py" || scr == "harness.py") print;
      }
    }')
if [ -n "$strays" ]; then
  echo "$strays" | while IFS= read -r l; do
    note "long-lived: $(echo "$l" | cut -c1-120)"
  done
  echo "     (kill deliberately — one of these may be a run you still want)"
else
  ok "no brood/nest/bench process older than an hour"
fi

echo
echo "3. boot cache"
cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/brood"
n_cache=$(find "$cache_dir" -name 'prelude-expanded-*.blsp' 2>/dev/null | wc -l | tr -d ' ')
if [ "${n_cache:-0}" -eq 0 ]; then
  note "no expanded-prelude cache — the next boot of each binary pays ~1.2 s instead of ~16 ms"
  echo "     warm it with scripts/warm-boot-cache.sh before timing or fanning out a suite"
else
  ok "$n_cache expanded-prelude cache file(s) in $cache_dir"
  stale=$(find "$cache_dir" -name 'prelude-expanded-*.blsp' -mtime +14 2>/dev/null | wc -l | tr -d ' ')
  [ "${stale:-0}" -gt 0 ] && note "$stale cache file(s) older than 14 days — keyed on binaries that are probably gone"
fi

echo
echo "4. disk litter"
if [ -d target/ab ]; then
  n_wt=$(find target/ab -maxdepth 1 -mindepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
  sz=$(du -sh target/ab 2>/dev/null | cut -f1)
  [ "${n_wt:-0}" -gt 0 ] && note "$n_wt \`make ab\` baseline worktree(s) in target/ab ($sz) — \`make ab-clean\`"
else
  ok "no make-ab worktrees"
fi
n_tmp=$(find /tmp -maxdepth 1 -name 'brood-*' 2>/dev/null | wc -l | tr -d ' ')
if [ "${n_tmp:-0}" -gt 200 ]; then
  note "$n_tmp /tmp/brood-* entries — the KI-30 shape; check the temp-dir purge still runs"
else
  ok "${n_tmp:-0} /tmp/brood-* entries"
fi

echo
if [ "$findings" -eq 0 ]; then
  printf '\033[32mno findings\033[0m\n'
  exit 0
fi
printf '%s finding(s)\n' "$findings"
[ "$strict" -eq 1 ] && exit 1
exit 0
