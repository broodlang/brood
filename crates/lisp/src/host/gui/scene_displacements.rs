//! A pressed draw's displacement at a list of its vertices, read back every frame it can be
//! (ADR-847): what the contact passes moved each listed vertex by this frame (its drawn place less
//! its skinned place, ADR-846), for a probe or a solve on the CPU that must follow her drawn skin
//! (a garment's cords against it). A standing request: `gui/scene-watch-displacements` names the
//! draw (its index in the first scene's `:draws`) and the vertices, nil stops it;
//! `gui/scene-displacements` gives the newest sample, a frame or two behind.

use std::sync::Mutex;

/// One frame's displacements of the listed vertices of one draw.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplacementSample {
    /// The draw's index in its scene's `:draws`.
    pub draw: usize,
    /// Each listed vertex's displacement, three floats a vertex, in the list's order.
    pub displacements: Vec<f32>,
}

static WATCH: Mutex<Option<(usize, Vec<u32>)>> = Mutex::new(None);
static LATEST: Mutex<Option<DisplacementSample>> = Mutex::new(None);

/// Watch draw `draw`'s `vertices` from the next frame on, or stop with `None`.
pub fn watch(request: Option<(usize, Vec<u32>)>) {
    *WATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = request;
    if WATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_none()
    {
        *LATEST
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// The standing request, if any.
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn watched() -> Option<(usize, Vec<u32>)> {
    WATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Keep `sample` as the newest.
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn record(sample: DisplacementSample) {
    *LATEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sample);
}

/// The newest sample read back, if any.
pub fn latest() -> Option<DisplacementSample> {
    LATEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watch_stands_until_stopped_and_stopping_drops_the_sample() {
        watch(Some((2, vec![5, 9])));
        assert_eq!(watched(), Some((2, vec![5, 9])));
        record(DisplacementSample {
            draw: 2,
            displacements: vec![0.0, 0.001, 0.0, 0.0, 0.0, 0.002],
        });
        assert_eq!(
            watched(),
            Some((2, vec![5, 9])),
            "a sample leaves it standing"
        );
        assert!(latest().is_some());
        watch(None);
        assert_eq!(watched(), None);
        assert_eq!(latest(), None);
    }
}
