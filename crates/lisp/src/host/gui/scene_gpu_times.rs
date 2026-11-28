//! The GPU's own time on a frame's 3D scenes (ADR-801): what the card spent rendering their
//! shadow maps and their colour, as its timestamp queries measured it, for `gui/scene-gpu-times`
//! and the `BROOD_GUI_TRACE` line. Filled by the GPU render target (`gpu::scene_timer`) a few
//! frames after the frame it measures; empty where no target measures (the CPU painter, an
//! adapter without timestamp queries, `BROOD_GUI_GPU_TIMES=0`).
//!
//! One log for the process, not one a window: a program draws its scenes in one window, and
//! the films and probes that read this open one.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many frames' measures the log keeps, newest last.
pub const SCENE_GPU_SAMPLES: usize = 64;

/// One frame's scenes as the GPU timed them, in microseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneGpuSample {
    /// From the start of the first shadow pass to the end of the last, over every scene timed.
    pub shadow_us: f64,
    /// The colour passes (rasterising, shading, the multisample resolve), over every scene timed.
    pub scene_us: f64,
    /// The contact compute passes (posing, relax, press and its smoothing, ADR-844), over every
    /// scene timed.
    pub contacts_us: f64,
    /// How many scenes were timed (at most `gpu::scene_timer::TIMED_SCENES`).
    pub scenes: u32,
}

static LOG: Mutex<VecDeque<SceneGpuSample>> = Mutex::new(VecDeque::new());

/// Keep `sample` as the newest, dropping the oldest past `SCENE_GPU_SAMPLES`.
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn record(sample: SceneGpuSample) {
    let mut log = LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if log.len() == SCENE_GPU_SAMPLES {
        log.pop_front();
    }
    log.push_back(sample);
}

/// The kept samples, oldest first.
pub fn scene_gpu_times() -> Vec<SceneGpuSample> {
    let log = LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    log.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_keeps_the_newest_samples_in_order() {
        for index in 0..(SCENE_GPU_SAMPLES + 5) {
            record(SceneGpuSample {
                shadow_us: index as f64,
                scene_us: 0.0,
                contacts_us: 0.0,
                scenes: 1,
            });
        }
        let kept = scene_gpu_times();
        assert_eq!(kept.len(), SCENE_GPU_SAMPLES);
        assert_eq!(
            kept.last().map(|sample| sample.shadow_us),
            Some((SCENE_GPU_SAMPLES + 4) as f64)
        );
    }
}
