//! A pressed draw's vertices as the GPU drew them, read back for one frame (ADR-841): where its
//! compute passes put each vertex (skinned with its centres, morphed, its flesh lagging, pressed by
//! the scene's contacts, a garment pushed out of her) in world space, the positions the vertex stage
//! draws. Asked for with `gui/scene-request-posed` (a draw's index in the first scene's `:draws`),
//! filled by the GPU render target (`gpu::posed_reader`) a frame or two later, read with
//! `gui/scene-posed`: a probe's ground truth for what is on the screen.

use std::sync::Mutex;

/// One frame's posed vertices of one draw.
#[derive(Clone, Debug, PartialEq)]
pub struct PosedSample {
    /// The draw's index in its scene's `:draws`.
    pub draw: usize,
    /// Its vertices' world positions, three floats a vertex.
    pub positions: Vec<f32>,
    /// Their normals, three floats a vertex.
    pub normals: Vec<f32>,
}

static REQUEST: Mutex<Option<usize>> = Mutex::new(None);
static LATEST: Mutex<Option<PosedSample>> = Mutex::new(None);

/// Ask for draw `draw`'s posed vertices at the next frame that draws it pressed.
pub fn request(draw: usize) {
    *REQUEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(draw);
}

/// The draw asked for, if any (kept until it is served).
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn requested() -> Option<usize> {
    *REQUEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Keep `sample` as the newest, and let the request go.
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn record(sample: PosedSample) {
    let mut request = REQUEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *request == Some(sample.draw) {
        *request = None;
    }
    *LATEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sample);
}

/// The newest sample read back, if any.
pub fn latest() -> Option<PosedSample> {
    LATEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_served_once_and_the_sample_kept() {
        request(3);
        assert_eq!(requested(), Some(3));
        record(PosedSample {
            draw: 3,
            positions: vec![1.0, 2.0, 3.0],
            normals: vec![0.0, 0.0, 1.0],
        });
        assert_eq!(requested(), None);
        assert_eq!(
            latest().map(|sample| sample.positions),
            Some(vec![1.0, 2.0, 3.0])
        );
    }
}
