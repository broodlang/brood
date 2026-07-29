//! Empirical mimalloc size-class boundaries, so per-process struct budgets are set against
//! the allocator's real steps rather than against `size_of`. Ignored by default: it is a
//! measurement, not an assertion. Run with
//! `cargo test -p brood --release --test size_class_probe -- --ignored --nocapture`.
//!
//! Reads **VmRSS** (current), not `VmHWM` — peak RSS is monotonic, so consecutive batches
//! would report meaningless deltas.

fn rss_kb() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("VmRSS:") {
            return v.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        }
    }
    0
}

#[test]
#[ignore]
fn report_size_classes() {
    const N: usize = 200_000;
    for size in [
        512usize, 640, 768, 896, 1000, 1024, 1088, 1152, 1208, 1280, 1344, 1408, 1536,
    ] {
        let base = rss_kb();
        let mut v: Vec<Box<[u8]>> = Vec::with_capacity(N);
        for _ in 0..N {
            let mut b = vec![0u8; size].into_boxed_slice();
            b[0] = 1; // touch, so the page is resident
            b[size - 1] = 1;
            v.push(b);
        }
        std::hint::black_box(&v);
        let per = (rss_kb().saturating_sub(base) as f64) * 1024.0 / N as f64;
        eprintln!("CLASS size={size:5} -> {per:7.1} B/alloc");
        drop(v);
    }
}
