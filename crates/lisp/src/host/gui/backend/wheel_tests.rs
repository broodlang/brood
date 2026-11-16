//! Tests of the wheel glide's calibration — one notch glides about three lines, a streak
//! of quick notches accelerates slightly and is capped — and of the coast at any refresh.

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

/// What a coast covers, stepped by `coast_step` every `step_ms` until it stops: `(lines, ms)`.
fn coast(v0: f64, decay: f64, step_ms: f64) -> (f64, f64) {
    let (mut velocity, mut lines, mut elapsed) = (v0, 0.0, 0.0);
    while velocity.abs() >= 0.0005 {
        let (moved, left) = coast_step(velocity, decay, step_ms);
        lines += moved;
        velocity = left;
        elapsed += step_ms;
    }
    (lines, elapsed)
}

/// A notch glides the same three lines, in the same time, stepped once a 60 Hz refresh, once
/// a 144 Hz one, or in the 12 ms ticks it was calibrated in (ADR-726) — and that is the
/// distance `glide_distance` documents.
#[test]
fn a_coast_covers_the_same_ground_at_any_refresh() {
    let v0 = wheel_velocity(0.0, 0, 1.0);
    let (ticked, _) = coast(v0, WHEEL_DECAY, COAST_TICK_MS);
    assert!((ticked - glide_distance(v0, WHEEL_DECAY)).abs() < 0.05);
    for decay in [WHEEL_DECAY, TRACKPAD_DECAY] {
        let (at60, time60) = coast(v0, decay, 1000.0 / 60.0);
        let (at144, time144) = coast(v0, decay, 1000.0 / 144.0);
        let whole = v0 / (1.0 - decay);
        assert!(
            (at60 - whole).abs() / whole < 0.01,
            "60 Hz: {at60} of {whole}"
        );
        assert!(
            (at144 - whole).abs() / whole < 0.01,
            "144 Hz: {at144} of {whole}"
        );
        // and it takes about as long: within a refresh or two of each other
        assert!(
            (time60 - time144).abs() < 40.0,
            "{time60} ms vs {time144} ms"
        );
    }
}

/// A step that comes late moves what the time it spans would have, not one tick's worth.
#[test]
fn a_late_step_catches_up() {
    let (one, left_one) = coast_step(1.0, WHEEL_DECAY, COAST_TICK_MS);
    let (two, _) = coast_step(left_one, WHEEL_DECAY, COAST_TICK_MS);
    let (both, _) = coast_step(1.0, WHEEL_DECAY, 2.0 * COAST_TICK_MS);
    assert!((one + two - both).abs() < 1e-9);
}

/// A window steps by its monitor's refresh: 60 Hz when the platform cannot say, and a rate
/// out of 30..=360 Hz clamped into it rather than dropped to 60.
#[test]
fn the_frame_period_follows_the_refresh_and_is_clamped() {
    let ms = |rate: Option<u32>| super::period_of(rate).as_secs_f64() * 1000.0;
    assert!((ms(Some(144_000)) - 1000.0 / 144.0).abs() < 1e-6);
    assert!((ms(None) - 1000.0 / 60.0).abs() < 1e-6);
    assert!((ms(Some(500_000)) - 1000.0 / 360.0).abs() < 1e-6);
    assert!((ms(Some(24_000)) - 1000.0 / 30.0).abs() < 1e-6);
}
