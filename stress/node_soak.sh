#!/usr/bin/env bash
# The node soak: three relay nodes and a hub passing a token around them
# (stress/node_soak.blsp), for a fixed time, while this driver hard-kills one relay every
# KILL_EVERY seconds and restarts it a few seconds later. The hub fails the run when no lap
# completes for 90 s; this driver fails it on the hub's exit status, a missing PASS line, or
# any runtime alarm on any node's stderr.
#
# Every node's log is kept, and so is a one-line record of every kill: a hunt that keeps
# nothing on success cannot be believed afterwards.
#
# Usage: stress/node_soak.sh [SECONDS] [KILL_EVERY]     (defaults 3600, 120)
#        BROOD=path/to/brood  SOAK_LOG_DIR=dir  DOWN_SECONDS=seconds (default 5)
#        NODE_CAP_KB=address-space cap per node (default 8000000)
set -u
cd "$(dirname "$0")/.."
BROOD=${BROOD:-target/release/brood}
duration=${1:-3600}
kill_every=${2:-120}
down_seconds=${DOWN_SECONDS:-5}
log_dir=${SOAK_LOG_DIR:-target/node-soak}
mkdir -p "$log_dir"
events="$log_dir/events.log"
: >"$events"

# Lines the runtime prints only when something is wrong (as in runtime_soak.sh).
alarms='STRANDED WORK|\[sched\] LEDGER|\[require\] BUG|lost: the scheduler|panicked at|use-after-GC|stale handle'

# Names unique to this run, so a stale socket from an earlier run is never in the way.
names=("soak1-$$" "soak2-$$" "soak3-$$")
declare -A relay_pid

# Each node runs under an address-space cap, so a node that runs away dies naming the cap
# instead of starving the runner: on 2026-11-16 (edb9962e) the hosted runner "lost
# communication" 74 min into this soak, which an exhausted runner looks like, and the logs
# went down with it. The runtime reserves ~3 GB of address space before it does any work
# (CLAUDE.md), so 8 GB is room to run and far short of the runner's memory times four.
node_cap_kb=${NODE_CAP_KB:-8000000}

start_relay() {
  local name=$1
  ( ulimit -v "$node_cap_kb"; exec "$BROOD" stress/node_soak.blsp -- relay "$name" ) \
    >>"$log_dir/$name.log" 2>&1 &
  relay_pid[$name]=$!
  echo "$(date -u +%FT%TZ) start $name pid ${relay_pid[$name]}" >>"$events"
}

cleanup() {
  for name in "${names[@]}"; do kill -9 "${relay_pid[$name]:-0}" 2>/dev/null; done
  kill -9 "${hub:-0}" 2>/dev/null
}
trap cleanup EXIT

for name in "${names[@]}"; do start_relay "$name"; done
sleep 3

# The hub runs a little longer than the kill loop, so its PASS comes after the last restart.
( ulimit -v "$node_cap_kb"; exec "$BROOD" stress/node_soak.blsp -- hub "$((duration + 30))" "${names[@]}" ) \
  >"$log_dir/hub.log" 2>&1 &
hub=$!

# One line per kill cycle with every node's resident memory, to the job log as well as the
# events file: if the runner dies, the job log up to that point is what survives.
report_memory() {
  local line="$(date -u +%FT%TZ) rss_kb"
  for name in "${names[@]}"; do
    line="$line $name=$(ps -o rss= -p "${relay_pid[$name]}" 2>/dev/null | tr -d ' ')"
  done
  line="$line hub=$(ps -o rss= -p "$hub" 2>/dev/null | tr -d ' ')"
  echo "$line" | tee -a "$events"
}

deadline=$(($(date +%s) + duration))
kills=0
while [ "$(date +%s)" -lt "$deadline" ] && kill -0 "$hub" 2>/dev/null; do
  sleep "$kill_every"
  report_memory
  [ "$(date +%s)" -lt "$deadline" ] || break
  kill -0 "$hub" 2>/dev/null || break
  victim=${names[$((RANDOM % 3))]}
  kill -9 "${relay_pid[$victim]}" 2>/dev/null
  wait "${relay_pid[$victim]}" 2>/dev/null
  echo "$(date -u +%FT%TZ) kill $victim" >>"$events"
  kills=$((kills + 1))
  sleep "$down_seconds"
  start_relay "$victim"
done

wait "$hub"
status=$?

verdict=ok
if [ "$status" -ne 0 ]; then
  verdict="hub FAILED (exit $status)"
elif ! grep -q '^node-soak: PASS' "$log_dir/hub.log"; then
  verdict="no PASS line from the hub"
elif grep -qE "$alarms" "$log_dir"/*.log; then
  verdict="runtime ALARM in a node's log"
fi

if [ "$verdict" != ok ]; then
  echo "node soak: $verdict after $kills kills — logs kept in $log_dir"
  echo "--- hub, last 30 lines ---"
  tail -30 "$log_dir/hub.log"
  grep -lE "$alarms" "$log_dir"/*.log 2>/dev/null | while read -r log; do
    echo "--- alarm in $log ---"
    grep -E -m5 "$alarms" "$log"
  done
  exit 1
fi
echo "node soak: clean for ${duration}s with $kills kills — $(grep '^node-soak: PASS' "$log_dir/hub.log")"
