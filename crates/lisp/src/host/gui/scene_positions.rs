//! Listed vertices of several pressed draws at their drawn places, read back every frame it can be
//! (ADR-848): each listed vertex's world position as the GPU drew it (skinned about its centres,
//! her keys, the loose lag, the contacts, a carried garment's carry and the push-out: the posed
//! buffer's first slot, ADR-846), for a solve on the CPU that must meet her drawn pieces (a cord's
//! ends on the piece each is sewn to). A standing request: `gui/scene-watch-positions` names each
//! draw (its index in the first scene's `:draws`) with its vertices, nil stops it;
//! `gui/scene-positions` gives the newest sample, a frame or two behind.

use std::sync::Mutex;

/// One frame's drawn places of the listed vertices of the watched draws.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionSample {
    /// Each listed vertex's world position, three floats a vertex, the draws in the watched order
    /// and each draw's vertices in its list's order.
    pub positions: Vec<f32>,
    /// Whether each watched draw was posed that frame (a draw not pressed, or past the scene's
    /// draws, is not: its vertices read 0).
    pub posed: Vec<bool>,
}

/// The watched draws: each draw's index and its vertices.
pub type Watch = Vec<(usize, Vec<u32>)>;

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);
static LATEST: Mutex<Option<PositionSample>> = Mutex::new(None);

/// Watch `request`'s draws and vertices from the next frame on, or stop with `None` (which drops
/// the last sample, so a new watch never reads an old one).
pub fn watch(request: Option<Watch>) {
    let stopped = request.is_none();
    *WATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = request;
    if stopped {
        *LATEST
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// The standing request, if any.
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn watched() -> Option<Watch> {
    WATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Keep `sample` as the newest.
#[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
pub(crate) fn record(sample: PositionSample) {
    *LATEST
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sample);
}

/// The newest sample read back, if any.
pub fn latest() -> Option<PositionSample> {
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
        let request = vec![(3, vec![1, 2]), (5, vec![7])];
        watch(Some(request.clone()));
        assert_eq!(watched(), Some(request.clone()));
        record(PositionSample {
            positions: vec![0.0; 9],
            posed: vec![true, false],
        });
        assert_eq!(watched(), Some(request), "a sample leaves it standing");
        assert!(latest().is_some());
        watch(None);
        assert_eq!(watched(), None);
        assert_eq!(latest(), None, "stopped, the old sample goes");
    }
}
