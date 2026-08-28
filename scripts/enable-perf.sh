#!/bin/sh
# Make `perf` usable for profiling brood on this machine, and say what else is needed.
#
# Two separate things block a readable profile, and fixing one without the other looks like
# failure in different ways:
#
#   1. `kernel.perf_event_paranoid` — Ubuntu ships 4, which refuses ALL unprivileged perf use.
#      `perf record` then fails with "Failure to open any events for recording", which reads
#      like a broken perf install rather than a policy setting. THIS is what the script fixes.
#
#   2. `[profile.release-fast]` sets `strip = true`, so even with perf permitted the report
#      shows bare addresses instead of `dispatch::dispatch` — a profile you cannot act on.
#      The script cannot fix that (it is a build flag, not a sysctl); it prints the build
#      command at the end.
#
# Values for perf_event_paranoid, so you can choose rather than trust the default:
#
#   -1  everything, including raw tracepoints
#    0  kernel profiling + CPU events; no raw tracepoint access
#    1  CPU events + kernel profiling            <- the default here
#    2  user-space profiling only; kernel profiling refused
#    3+ (Ubuntu) no unprivileged perf at all     <- what you have
#
# **2 is enough** for self-time inside brood's own functions, which is what the compute
# frontier work needs. 1 additionally resolves KERNEL symbols, which is worth having the
# first time a row turns out to be dominated by the allocator or a syscall rather than by
# brood. Pass the level as $1 to override.
#
# Undo: `sudo rm /etc/sysctl.d/99-brood-perf.conf && sudo sysctl --system`
set -eu

LEVEL="${1:-1}"
CONF=/etc/sysctl.d/99-brood-perf.conf

# `--check` reports and changes nothing — so you can see what the script would act on, and
# what the current setting actually permits, without granting it root.
if [ "$LEVEL" = "--check" ] || [ "$LEVEL" = "-n" ]; then
    cur=$(cat /proc/sys/kernel/perf_event_paranoid)
    printf '  %-32s %s\n' "kernel.perf_event_paranoid" "$cur"
    printf '  %-32s %s\n' "kernel.kptr_restrict" "$(cat /proc/sys/kernel/kptr_restrict)"
    printf '  %-32s %s\n' "persisted setting" "$( [ -f "$CONF" ] && echo "$CONF" || echo '(none — distro default)' )"
    if [ "$cur" -ge 3 ] 2>/dev/null; then
        echo "  -> refuses ALL unprivileged perf; \`perf record\` fails to open events"
    elif [ "$cur" -eq 2 ]; then
        echo "  -> user-space profiling OK (enough for brood self-time); kernel profiling refused"
    else
        echo "  -> user-space and kernel profiling permitted"
    fi
    if perf record -q -o /tmp/.brood-perf-probe.$$ -- true 2>/dev/null; then
        echo "  -> probe: \`perf record\` works now"; rm -f "/tmp/.brood-perf-probe.$$"
    else
        echo "  -> probe: \`perf record\` cannot open events"
    fi
    exit 0
fi

case "$LEVEL" in
    -1|0|1|2) ;;
    *) echo "enable-perf: level must be -1, 0, 1 or 2 (got '$LEVEL'); 3+ is what blocks perf" >&2
       exit 2 ;;
esac

show() {
    printf '  %-32s %s\n' "kernel.perf_event_paranoid" "$(cat /proc/sys/kernel/perf_event_paranoid)"
    printf '  %-32s %s\n' "kernel.kptr_restrict" "$(cat /proc/sys/kernel/kptr_restrict)"
}

echo "before:"; show

# Re-exec under sudo rather than telling the user to retry — but only if we are not root and
# sudo exists, and never silently: the command being escalated is printed.
if [ "$(id -u)" -ne 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
        echo
        echo "enable-perf: needs root to write $CONF; escalating:"
        echo "  sudo $0 $LEVEL"
        exec sudo "$0" "$LEVEL"
    fi
    echo "enable-perf: not root and no sudo found — run this as root" >&2
    exit 1
fi

# Persist first, then apply, so a reboot cannot leave the two disagreeing.
cat > "$CONF" <<EOF
# Written by scripts/enable-perf.sh (brood). Remove this file and run
# \`sudo sysctl --system\` to restore the distro default.
#
# Ubuntu defaults perf_event_paranoid to 4, which refuses all unprivileged perf use.
# $LEVEL permits the profiling the compute-frontier work needs.
kernel.perf_event_paranoid = $LEVEL
EOF

sysctl -q -w "kernel.perf_event_paranoid=$LEVEL"
[ "$LEVEL" -le 1 ] 2>/dev/null && sysctl -q -w kernel.kptr_restrict=0 || true

echo
echo "after:"; show
echo "  persisted in $CONF"

# Verify by DOING it, not by trusting the value we just wrote — the whole reason this script
# exists is that the failure mode looked like a broken perf rather than a refused event.
echo
if perf record -q -o /tmp/.brood-perf-probe.$$ -- true 2>/dev/null; then
    echo "verified: \`perf record\` opened events successfully"
    rm -f "/tmp/.brood-perf-probe.$$"
else
    echo "WARNING: perf still cannot record. Check for a container/VM without PMU access," >&2
    echo "         a hardened kernel, or an LSM (apparmor/lockdown) also refusing it." >&2
    exit 1
fi

cat <<'EOF'

Second half, which no sysctl can fix: `[profile.release-fast]` sets `strip = true`, so a
profile of the ordinary build shows addresses, not symbols. Build unstripped, with the SAME
flags `make ab` measures, or you are profiling a different binary:

  RUSTFLAGS="-C debug-assertions=off -C overflow-checks=off" \
    cargo build --profile release-fast --bin brood \
    --config 'profile.release-fast.strip=false' --config 'profile.release-fast.debug=1'

  perf record --call-graph dwarf -- ./target/release-fast/brood bench.blsp
  perf report --no-children          # --no-children: self time, which is what the
                                     # compute-frontier tables quote
EOF
