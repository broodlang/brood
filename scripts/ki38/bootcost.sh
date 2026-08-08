#!/usr/bin/env bash
# KI-38: what does a COLD expanded-prelude boot cache cost a debug `brood`,
# alone and under a concurrent herd?
#
# The previous session measured boot at 151 ms idle / 4066 ms worst under load
# and concluded a 20-30 s deadline cannot be the tail of that distribution.
# Every one of those samples was taken with a WARM cache. `build_id` embeds the
# binary's own mtime, so the cache is cold for the FIRST run after every
# rebuild -- which is exactly the state a suite run starts in.
#
# Isolated with XDG_CACHE_HOME so the real ~/.cache/brood is never touched.
set -u
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BROOD="$REPO/target/debug/brood"
WORK=$(mktemp -d /tmp/ki38-bootcost-XXXXXX)
trap 'rm -rf "$WORK"' EXIT

echo "(println \"up\")" > "$WORK/up.blsp"

ms() { python3 -c "import time;print(int(time.time()*1000))"; }

run_one() { # $1 = cache home
  local t0 t1
  t0=$(ms)
  XDG_CACHE_HOME="$1" "$BROOD" "$WORK/up.blsp" >/dev/null 2>&1
  t1=$(ms)
  echo $((t1 - t0))
}

echo "=== single COLD boots (fresh cache dir each time) ==="
for i in 1 2 3 4 5; do
  c="$WORK/cold$i"; mkdir -p "$c"
  echo "  cold $i: $(run_one "$c") ms"
done

echo "=== single WARM boots (same cache dir, already populated) ==="
warm="$WORK/warm"; mkdir -p "$warm"
run_one "$warm" >/dev/null            # populate
for i in 1 2 3 4 5; do
  echo "  warm $i: $(run_one "$warm") ms"
done

echo "=== confirm which path each took (BROOD_BOOT_TRACE) ==="
c="$WORK/tracecold"; mkdir -p "$c"
echo -n "  cold: "; XDG_CACHE_HOME="$c" BROOD_BOOT_TRACE=1 "$BROOD" "$WORK/up.blsp" 2>&1 >/dev/null | grep '^\[boot\]'
echo -n "  warm: "; XDG_CACHE_HOME="$c" BROOD_BOOT_TRACE=1 "$BROOD" "$WORK/up.blsp" 2>&1 >/dev/null | grep '^\[boot\]'

N="${1:-16}"
echo "=== $N CONCURRENT COLD boots, all sharing ONE cold cache dir ==="
echo "    (the herd a suite run starts with: every child misses, every child"
echo "     does the full source boot, and they all compete for the same cores)"
herd="$WORK/herd"; mkdir -p "$herd"
start=$(ms)
pids=()
for i in $(seq 1 "$N"); do
  ( t0=$(ms)
    XDG_CACHE_HOME="$herd" "$BROOD" "$WORK/up.blsp" >/dev/null 2>&1
    t1=$(ms)
    echo "$((t1 - t0))" > "$WORK/herd-$i.ms" ) &
  pids+=($!)
done
for p in "${pids[@]}"; do wait "$p"; done
end=$(ms)
cat "$WORK"/herd-*.ms | sort -n | python3 -c "
import sys
v=[int(x) for x in sys.stdin]
v.sort()
print(f'    n={len(v)}  min={v[0]}ms  med={v[len(v)//2]}ms  max={v[-1]}ms')
"
echo "    wall for the whole herd: $((end - start)) ms"

echo "=== $N CONCURRENT WARM boots, for contrast ==="
hw="$WORK/herdwarm"; mkdir -p "$hw"
XDG_CACHE_HOME="$hw" "$BROOD" "$WORK/up.blsp" >/dev/null 2>&1   # populate
start=$(ms)
pids=()
for i in $(seq 1 "$N"); do
  ( t0=$(ms)
    XDG_CACHE_HOME="$hw" "$BROOD" "$WORK/up.blsp" >/dev/null 2>&1
    t1=$(ms)
    echo "$((t1 - t0))" > "$WORK/hw-$i.ms" ) &
  pids+=($!)
done
for p in "${pids[@]}"; do wait "$p"; done
end=$(ms)
cat "$WORK"/hw-*.ms | sort -n | python3 -c "
import sys
v=[int(x) for x in sys.stdin]
v.sort()
print(f'    n={len(v)}  min={v[0]}ms  med={v[len(v)//2]}ms  max={v[-1]}ms')
"
echo "    wall for the whole herd: $((end - start)) ms"
