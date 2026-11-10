//! Tests of where the event loop is hosted: the decision `hosting_required_from` makes for
//! a platform with nowhere else for the loop, for Wayland/X11 by default, and under the
//! `BROOD_GUI_MAIN_THREAD` lever.

use super::hosting_required_from;

/// macOS/Windows: the loop has one legal home and the lever must not be able to
/// move it. A `=0` that *was* honoured would not read as a config mistake — the GUI
/// would simply fail to start, on the platform with no second path to fall back to.
#[test]
fn a_platform_with_nowhere_else_always_hosts_on_main() {
    for lever in [None, Some("0"), Some(""), Some("1")] {
        assert!(
            hosting_required_from(false, lever),
            "lever {lever:?} must not move the loop off the main thread"
        );
    }
}

/// Wayland/X11 keep the dedicated `brood-gui` thread by default — this is the path
/// that has always worked and the one every existing GUI app runs on.
#[test]
fn wayland_and_x11_keep_the_dedicated_thread_by_default() {
    assert!(!hosting_required_from(true, None));
    assert!(!hosting_required_from(true, Some("0")));
    assert!(!hosting_required_from(true, Some("")));
}

/// ...but can opt in, which is the only reason the main-thread path is runnable
/// anywhere this project actually tests. Without this, the macOS code would ship
/// having been compiled and never executed.
#[test]
fn the_lever_opts_a_permissive_platform_into_the_main_thread_path() {
    assert!(hosting_required_from(true, Some("1")));
    assert!(hosting_required_from(true, Some("yes")));
}
