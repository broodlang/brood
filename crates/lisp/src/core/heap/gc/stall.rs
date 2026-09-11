//! The stall guard: a scope timer that reports any GC or compaction phase that ran past
//! `BROOD_STALL_MS`. Used by the RUNTIME compactor (`gc_runtime`), the scheduler and the
//! GUI paint path.

/// Gameplay-lag diagnostic. `BROOD_STALL_MS=<n>`: anything wrapped in a `stall_guard`
/// that runs ≥ n ms logs `[stall] <label> Nms` to stderr. Release-capable; zero cost
/// (no `Instant`) unless the env is set. Used to pinpoint a long pause (GC vs compaction
/// vs elsewhere) in a live session that can't be driven headless.
pub(crate) fn stall_threshold_ms() -> Option<u128> {
    static MS: std::sync::OnceLock<Option<u128>> = std::sync::OnceLock::new();
    *MS.get_or_init(|| {
        std::env::var("BROOD_STALL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    })
}

pub(crate) struct StallGuard {
    label: &'static str,
    pid: Option<u64>,
    t0: web_time::Instant,
    ms: u128,
}

impl Drop for StallGuard {
    fn drop(&mut self) {
        let el = self.t0.elapsed().as_millis();
        if el >= self.ms {
            match self.pid {
                Some(p) => eprintln!("[stall] {} (pid {}) took {}ms", self.label, p, el),
                None => eprintln!("[stall] {} took {}ms", self.label, el),
            }
        }
    }
}

/// A pid-less stall guard for a non-process work span (GC compaction/minor
/// collection, and the GUI paint path). The `heap::stall_guard` re-export is
/// gated on the `gui` feature since only `gui.rs` reaches it that way.
pub(crate) fn stall_guard(label: &'static str) -> Option<StallGuard> {
    stall_threshold_ms().map(|ms| StallGuard {
        label,
        pid: None,
        t0: web_time::Instant::now(),
        ms,
    })
}

pub(crate) fn stall_guard_pid(label: &'static str, pid: u64) -> Option<StallGuard> {
    stall_threshold_ms().map(|ms| StallGuard {
        label,
        pid: Some(pid),
        t0: web_time::Instant::now(),
        ms,
    })
}
