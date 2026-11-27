//! The push-out net's uses (ADR-839): how many garment vertices each frame's scenes found inside
//! her skin's garment contacts and pushed out, as the GPU counted them, for `gui/scene-push-outs`.
//! Filled by the GPU render target (`gpu::push_outs`) a frame or two after the frame it counts;
//! empty where nothing counts (the CPU painter, a scene with no garment).
//!
//! The net is a safety net, never the fit: every use is a fault in the garment's build, and the
//! clip probe and the gate read this to say so. One log for the process, as `scene_gpu_times`.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many frames' counts the log keeps, newest last.
pub const SCENE_PUSH_OUT_SAMPLES: usize = 256;

static LOG: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

/// Keep `count`, a frame's garment vertices pushed out, as the newest.
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn record(count: u64) {
    let mut log = LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if log.len() == SCENE_PUSH_OUT_SAMPLES {
        log.pop_front();
    }
    log.push_back(count);
}

/// The kept counts, oldest first.
pub fn scene_push_outs() -> Vec<u64> {
    let log = LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    log.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_the_newest_counts() {
        for count in 0..(SCENE_PUSH_OUT_SAMPLES as u64 + 10) {
            record(count);
        }
        let kept = scene_push_outs();
        assert_eq!(kept.len(), SCENE_PUSH_OUT_SAMPLES);
        assert_eq!(
            *kept.last().expect("a count"),
            SCENE_PUSH_OUT_SAMPLES as u64 + 9
        );
    }
}
