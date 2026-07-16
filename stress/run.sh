#!/usr/bin/env bash
# The occasional BIG stress run (not part of `make test` / CI):
#   make stress          — everything below, across 3 engines
# Coverage: property-based table model vs an immutable-map oracle, multi-process
# races across migrations/drops, VM/JIT loop preempt-resume checksums, match
# lowering vs a cond oracle, a cross-language (python) differential, and GC-stress
# reruns. Known-bug repros live in xfail_*; they are EXPECTED to fail and are
# reported separately (an xfail that PASSES means a known bug got fixed — promote
# it to tests/).
set -u
cd "$(dirname "$0")/.."
BROOD=${BROOD:-target/release/brood}
pass=0; fail=0; xfail=0; xpass=0

run_one() { # file, env-prefix, label
  local f=$1 envs=$2 label=$3
  if env $envs "$BROOD" --test "$f" >/dev/null 2>&1; then
    if [[ $f == */xfail_* ]]; then echo "XPASS(!!) $label $f — known bug fixed? promote to tests/"; xpass=$((xpass+1));
    else echo "pass  $label $f"; pass=$((pass+1)); fi
  else
    if [[ $f == */xfail_* ]]; then echo "xfail $label $f (known bug)"; xfail=$((xfail+1));
    else echo "FAIL  $label $f"; fail=$((fail+1)); fi
  fi
}

for f in stress/*_test.blsp; do
  run_one "$f" "BROOD_VM=1" "jit    "
  # xfail repros are JIT-only bugs — a no-jit pass is expected, not a promotion signal
  [[ $f == */xfail_* ]] || run_one "$f" "BROOD_VM=1 BROOD_NO_JIT=1" "no-jit "
done
# GC-stress pass on the table suites (the GC-sensitive ones); loops are too slow under stress
for f in stress/table_model_test.blsp stress/match_props_test.blsp stress/core_semantics_test.blsp stress/collections_test.blsp; do
  run_one "$f" "BROOD_VM=1 BROOD_GC_STRESS=1" "gc-str "
done

# Random-program differential fuzzer (engine/GC/chaos-preempt configs must agree)
if python3 stress/fuzz_programs.py --seeds 25 >/dev/null 2>&1; then
  echo "pass  program fuzzer (25 seeds x 4 configs)"; pass=$((pass+1))
else
  echo "FAIL  program fuzzer — divergent seeds kept in stress/fuzz_out/"; fail=$((fail+1))
fi

# Chaos-preemption pass: a tiny prime reduction budget forces preempt/resume
# storms through every loop — the capture machinery's torture test.
for f in stress/table_model_test.blsp stress/vm_loops_test.blsp; do
  run_one "$f" "BROOD_VM=1 BROOD_REDUCTIONS=97" "chaos  "
done

# Cross-language differential
a=$("$BROOD" stress/table_digest.blsp 2>/dev/null)
b=$(python3 stress/table_oracle.py)
if [ "$a" = "$b" ]; then echo "pass  x-lang table digest ($a)"; pass=$((pass+1));
else echo "FAIL  x-lang: brood='$a' python='$b'"; fail=$((fail+1)); fi

echo "---- stress: $pass passed, $fail failed, $xfail known-bug xfails, $xpass unexpected xpasses"
[ "$fail" -eq 0 ] && [ "$xpass" -eq 0 ]
