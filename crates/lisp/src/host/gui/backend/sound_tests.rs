//! Tests of which `[:sound …]` ops a frame plays when it arrives (ADR-777): every one of a
//! sound the window holds, and a KEYED one only on the frame that first carries it.

use super::*;

fn held(handles: &[u32]) -> HashMap<u32, std::sync::Arc<crate::host::audio::SoundData>> {
    handles
        .iter()
        .map(|handle| {
            let data = crate::host::audio::SoundData::from_pcm16(8000, 1, &[0, 0, 0, 1]);
            (*handle, std::sync::Arc::new(data))
        })
        .collect()
}

fn sound(id: u32, key: Option<i64>) -> Op {
    Op::Sound {
        id,
        vol: 0.5,
        pan: -0.25,
        key,
    }
}

#[test]
fn an_unkeyed_sound_plays_on_every_frame_that_carries_it() {
    let sounds = held(&[1]);
    let frame = vec![sound(1, None)];
    assert_eq!(sounds_to_play(&sounds, &[], &frame), vec![(1, 0.5, -0.25)]);
    assert_eq!(
        sounds_to_play(&sounds, &frame, &frame),
        vec![(1, 0.5, -0.25)],
        "unkeyed, a repeat is a second play, as before keys"
    );
}

#[test]
fn a_keyed_sound_plays_once_however_many_frames_in_a_row_carry_it() {
    let sounds = held(&[1, 2]);
    let first = vec![sound(1, Some(7))];
    assert_eq!(sounds_to_play(&sounds, &[], &first).len(), 1, "new: plays");
    let again = vec![sound(1, Some(7)), sound(1, Some(8)), sound(2, Some(7))];
    assert_eq!(
        sounds_to_play(&sounds, &first, &again),
        vec![(1, 0.5, -0.25), (2, 0.5, -0.25)],
        "the same handle and key go on; another key, or another handle, is new"
    );
    let doubled = vec![sound(1, Some(9)), sound(1, Some(9))];
    assert_eq!(
        sounds_to_play(&sounds, &[], &doubled).len(),
        1,
        "a frame carrying one twice (a skipped frame's carried along) plays it once"
    );
    assert_eq!(
        sounds_to_play(&sounds, &[], &first).len(),
        1,
        "after a frame without it, it is new again"
    );
}

#[test]
fn a_sound_the_window_does_not_hold_plays_nothing() {
    assert!(sounds_to_play(&held(&[1]), &[], &[sound(3, None)]).is_empty());
}
