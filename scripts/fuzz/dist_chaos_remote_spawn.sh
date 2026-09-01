#!/bin/bash
set -u
BIN=${BROOD:-$(cd "$(dirname "$0")/../.." && pwd)/target/release/brood}
D="$(mktemp -d "${TMPDIR:-/tmp}/brood-chaos.XXXX")"
mkdir -p "$D"; cd "$D"; RUN=$1
rm -f .brood_crash_dump n*.blsp n*.err
CK="chaos-cookie-16-chars+"
base=$(( 27000 + (RUN*20) + RANDOM % 800 ))
declare -A PORT PID EXIT
for i in $(seq 0 7); do PORT[$i]=$(( base + i )); done
seed=${PORT[0]}
mknode() {
  local i=$1 conn=$2 c=""
  [ "$conn" != "0" ] && c="(node/connect \"x@127.0.0.1:$conn\")"
  cat > "$D/n$i.blsp" <<EOF
(node/start :n$i "127.0.0.1:${PORT[$i]}" "$CK")
(node/serve-spawns)
$c
(proc/register :srv (self))
(defn chaos-each (ns f) (when (not (empty? ns)) (do (try (f (first ns)) (catch e nil)) (chaos-each (rest ns) f))))
(defn chaos-loop (k) (if (= k 0) :done (do
   (chaos-each (node/list) (fn (p) (monitor-node p)))
   (chaos-each (node/list) (fn (p) (remote-spawn p (+ k 1))))   ; ships a closure capturing k
   (sleep 20) (chaos-loop (- k 1)))))
(chaos-loop 6000)
EOF
  $BIN "$D/n$i.blsp" > "$D/n$i.err" 2>&1 &
  PID[$i]=$!
}
crashed=0
mknode 0 0; sleep 0.8
for i in 1 2 3 4 5; do mknode $i $seed; sleep 0.1; done
sleep 3
kill -9 ${PID[3]} 2>/dev/null; sleep 0.4
mknode 6 ${PORT[2]}; sleep 0.1                 # n6 via n2
kill -9 ${PID[0]} 2>/dev/null; sleep 0.8       # hub dies mid remote-spawn storm
mknode 7 ${PORT[5]}; sleep 1
kill -9 ${PID[2]} ${PID[5]} 2>/dev/null; sleep 1
kill -9 ${PID[1]} 2>/dev/null; sleep 0.8
# Harness rot is not a finding — it must not look like one. These node programs are
# heredocs inside a shell script, so `nest check` and `make check-corpora` (which scan
# .blsp files) cannot see them: when the v0.9/v0.10 namespacing waves renamed
# `node-start`/`register`/`nodes`/`start-remote-spawn`, every node here died at startup and
# the script kept reporting `crashed=1` — indistinguishable from a real crash, and
# unnoticed for months. An unbound symbol means THIS FILE is stale, not that the runtime is
# broken. Same lesson as KI-42/KI-44 in brood-benchmarks.
#
# Matches the DEFINITION-time error kinds attributed to the harness's own `nN.blsp` —
# unbound symbol, reserved-name collision (`each` became a stdlib name and broke this
# script a second time), arity, syntax. Deliberately NOT `runtime error`: this harness kills
# nodes on purpose, so "connect: Connection refused" is an EXPECTED outcome here, and
# treating it as rot would cry wolf on every healthy run. Anchored to the filename too: the checker's "catch discards the error unread" WARNING contains the words
# "hides an unbound symbol" in its prose, and matching the bare phrase made every clean run
# report rot.
if grep -qE "^n[0-9]+\.blsp:[0-9]+:[0-9]+: (unbound|type|arity|syntax|name) error:" n*.err 2>/dev/null; then
  echo ">>> RUN$RUN HARNESS ROT — a node failed to start, so this run tested NOTHING:"
  grep -hE "^n[0-9]+\.blsp:[0-9]+:[0-9]+: (unbound|type|arity|syntax|name) error:" n*.err 2>/dev/null | sed 's/^/      /' | sort -u | head -5
  echo ">>> fix the node program in $(basename "$0"); the runtime is not implicated."
  exit 2
fi
for i in $(seq 0 7); do
  kill -9 ${PID[$i]} 2>/dev/null; wait ${PID[$i]} 2>/dev/null; EXIT[$i]=$?
  case ${EXIT[$i]} in 0|137|143|"") ;; *) echo ">>> RUN$RUN n$i exit=${EXIT[$i]} CRASH"; crashed=1;; esac
done
grep -liE "panic|SIGSEGV|segmentation|use-after|cannot unwind" n*.err 2>/dev/null | while read f; do echo ">>> RUN$RUN STDERR $f:"; grep -iE "panic|segv|abort|use-after|cannot unwind" "$f"|head -2; done
[ -f .brood_crash_dump ] && { echo ">>> RUN$RUN CRASH DUMP"; grep "panicked at" .brood_crash_dump|head; crashed=1; }
echo "RUN$RUN crashed=$crashed exits=${EXIT[*]}"
