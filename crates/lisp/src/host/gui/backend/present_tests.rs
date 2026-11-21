//! Tests of a window's present events (`{:present-events true}`, ADR-775): the
//! `[:presented t-ns]` a successful present sends, only when asked, on `os/now-ns`'s clock.

use super::*;

/// The nanoseconds a `[:presented t-ns]` message carries, or a panic naming its shape.
fn presented_ns(message: Message) -> i64 {
    match &message {
        Message::Vector(items) => match items.as_slice() {
            [Message::Keyword(tag), Message::Int(ns)] if *tag == value::intern("presented") => *ns,
            _ => panic!("a vector, but not [:presented t-ns]"),
        },
        _ => panic!("not a vector"),
    }
}

/// A window that asked hears `[:presented t-ns]` for a frame that reached the compositor;
/// one that did not ask (the default) hears nothing, and neither does a paint that
/// presented nothing.
#[test]
fn a_present_is_told_only_to_a_window_that_asked() {
    let now = Instant::now();
    assert!(
        !WindowSpec::default().present_events,
        "present events are opt-in"
    );
    assert!(present_event(false, Some(now)).is_none());
    assert!(present_event(true, None).is_none());
    assert!(present_event(false, None).is_none());
    let message = present_event(true, Some(now)).expect("a window that asked is told");
    presented_ns(message);
}

/// The stamp is on `os/now-ns`'s clock — nanoseconds since the Unix epoch — and kept
/// monotonic: two presents a refresh apart are a refresh apart.
#[test]
fn the_stamp_is_wall_clock_nanoseconds_and_monotonic() {
    let first = Instant::now();
    let later = first + Duration::from_micros(16_667);
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let first_ns = presented_ns(present_event(true, Some(first)).unwrap());
    let later_ns = presented_ns(present_event(true, Some(later)).unwrap());
    assert_eq!(later_ns - first_ns, 16_667_000);
    assert!(
        (first_ns - wall).abs() < 1_000_000_000,
        "{first_ns} is not near the wall clock's {wall}"
    );
}
