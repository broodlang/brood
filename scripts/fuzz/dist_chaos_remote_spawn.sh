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
  [ "$conn" != "0" ] && c="(connect \"x@127.0.0.1:$conn\")"
  cat > "$D/n$i.blsp" <<EOF
(node-start :n$i "127.0.0.1:${PORT[$i]}" "$CK")
(start-remote-spawn)
$c
(register :srv (self))
(defn each (ns f) (when (not (empty? ns)) (do (try (f (first ns)) (catch e nil)) (each (rest ns) f))))
(defn loop (k) (if (= k 0) :done (do
   (each (nodes) (fn (p) (monitor-node p)))
   (each (nodes) (fn (p) (remote-spawn p (+ k 1))))   ; ships a closure capturing k
   (sleep 20) (loop (- k 1)))))
(loop 6000)
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
for i in $(seq 0 7); do
  kill -9 ${PID[$i]} 2>/dev/null; wait ${PID[$i]} 2>/dev/null; EXIT[$i]=$?
  case ${EXIT[$i]} in 0|137|143|"") ;; *) echo ">>> RUN$RUN n$i exit=${EXIT[$i]} CRASH"; crashed=1;; esac
done
grep -liE "panic|SIGSEGV|segmentation|use-after|cannot unwind" n*.err 2>/dev/null | while read f; do echo ">>> RUN$RUN STDERR $f:"; grep -iE "panic|segv|abort|use-after|cannot unwind" "$f"|head -2; done
[ -f .brood_crash_dump ] && { echo ">>> RUN$RUN CRASH DUMP"; grep "panicked at" .brood_crash_dump|head; crashed=1; }
echo "RUN$RUN crashed=$crashed exits=${EXIT[*]}"
