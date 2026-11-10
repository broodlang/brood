//! Tests of the wheel glide's calibration: one notch glides about three lines, and a streak
//! of quick notches accelerates slightly and is capped.

use super::*;

/// One notch glides about three lines — the distance a plain wheel notch scrolled
/// before it glided — and a notch against a running glide starts over.
#[test]
fn a_single_notch_glides_about_three_lines() {
    let v0 = wheel_velocity(0.0, 0, 1.0);
    let lines = glide_distance(v0, WHEEL_DECAY);
    assert!((2.7..3.3).contains(&lines), "a notch glides {lines} lines");
    assert!(wheel_velocity(0.0, 0, -1.0) < 0.0);
}

/// A streak accelerates slightly and is capped: the fifth quick notch scrolls
/// farther than the first, the twentieth no farther than the cap allows.
#[test]
fn a_streak_accelerates_slightly_and_is_capped() {
    let first = wheel_velocity(0.0, 0, 1.0);
    let fifth = wheel_velocity(0.0, 4, 1.0);
    let twentieth = wheel_velocity(0.0, 19, 1.0);
    assert!(
        fifth > first * 1.5 && fifth < first * 2.0,
        "fifth: {fifth} vs first {first}"
    );
    assert!(
        (twentieth - first * WHEEL_STREAK_CAP).abs() < 1e-9,
        "the cap holds"
    );
    // and a notch on top of a running glide adds to it rather than replacing it
    assert!(wheel_velocity(0.3, 0, 1.0) > first);
}
