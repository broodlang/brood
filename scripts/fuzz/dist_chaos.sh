#!/bin/bash
set -u
BIN=${BROOD:-$(cd "$(dirname "$0")/../.." && pwd)/target/release/brood}
D="$(mktemp -d "${TMPDIR:-/tmp}/brood-chaos.XXXX")"
mkdir -p "$D"; cd "$D"
CK="chaos-cookie-16-chars+"
RUN=$1
rm -f .brood_crash_dump n*.blsp n*.err
base=$(( 24000 + (RUN*20) + RANDOM % 1000 ))
declare -A PORT PID EXIT
for i in $(seq 0 9); do PORT[$i]=$(( base + i )); done
seed=${PORT[0]}
mknode() {  # idx connectport extra
  local i=$1 conn=$2 cookie=${3:-$CK} c=""
  [ "$conn" != "0" ] && c="(connect \"x@127.0.0.1:$conn\")"
  cat > "$D/n$i.blsp" <<EOF
(node-start :n$i "127.0.0.1:${PORT[$i]}" "$cookie")
$c
(register :srv (self))
(defn pa (ns) (when (not (empty? ns)) (do (try (send {:name :srv :node (first ns)} [:ping]) (catch e nil)) (pa (rest ns)))))
(defn pg (k) (if (= k 0) :done (do (pa (nodes)) (sleep 20) (pg (- k 1)))))
(spawn (pg 6000))
(defn drain () (receive ([:ping] (drain)) (after 120000 :done)))
(drain)
EOF
  $BIN "$D/n$i.blsp" > "$D/n$i.err" 2>&1 &
  PID[$i]=$!
}
crashed=0
mknode 0 0; sleep 0.8
for i in 1 2 3 4 5; do mknode $i $seed; sleep 0.1; done
sleep 3
# wrong-cookie attacker hammering the seed's handshake
( for j in $(seq 1 40); do echo "(try (do (node-start :atk$j \"127.0.0.1:$((base+50+j))\" \"WRONG-cookie-16+x\") (connect \"x@127.0.0.1:$seed\") (sleep 50)) (catch e nil))" | $BIN /dev/stdin >/dev/null 2>&1 & done; wait ) &
# churn: several kill+rejoin cycles
kill -9 ${PID[2]} ${PID[4]} 2>/dev/null; sleep 0.5
mknode 6 $seed; sleep 0.1; mknode 7 ${PORT[3]}; sleep 1.5   # n7 joins via n3 (not seed)
kill -9 ${PID[0]} 2>/dev/null; sleep 1                       # hub dies
mknode 8 ${PORT[5]}; sleep 0.1; mknode 9 ${PORT[1]}; sleep 1.5
kill -9 ${PID[6]} ${PID[1]} 2>/dev/null; sleep 1
kill -9 ${PID[3]} 2>/dev/null; sleep 1
# collect ALL exit codes (distinguish my SIGKILL=137 from crashes 134/139/132/101)
for i in $(seq 0 9); do
  kill -9 ${PID[$i]} 2>/dev/null
  wait ${PID[$i]} 2>/dev/null; EXIT[$i]=$?
  case ${EXIT[$i]} in 0|137|143|"") ;; *) echo ">>> RUN$RUN n$i exit=${EXIT[$i]} (CRASH?)"; crashed=1;; esac
done
grep -liE "panic|SIGSEGV|segmentation|use-after|cannot unwind|RUST_BACKTRACE" n*.err 2>/dev/null | while read f; do echo ">>> RUN$RUN STDERR $f"; grep -iE "panic|segv|abort|use-after" "$f"|head -2; done
[ -f .brood_crash_dump ] && { echo ">>> RUN$RUN CRASH DUMP"; grep "panicked at" .brood_crash_dump|head; crashed=1; }
echo "RUN$RUN crashed=$crashed exits=${EXIT[*]}"
