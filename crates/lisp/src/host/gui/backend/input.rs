//! Input translation: winit key and mouse events into the frontend-neutral
//! [`Key`]/[`Mouse`] vocabulary an app `receive`s, plus the click-chain and scroll
//! bookkeeping that turns raw events into `[:key …]` / `[:mouse …]` messages.

use super::*;
use crate::host::gui::named_key;

/// Max gap between consecutive presses (same button, same cell) that still counts
/// as part of one click chain — the double/triple-click window, in milliseconds.
pub(super) const MULTI_CLICK_MS: u128 = 400;

/// A key as the Brood value `term-poll`/`gui` deliver: a printable → a 1-char
/// string, the rest → keywords. Built as a `Message` (no heap) so the GUI thread
/// can deliver it straight to a mailbox (ADR-058). Mirrors `key_to_value`.
pub(super) fn key_message(k: &Key) -> Message {
    match k {
        Key::Char(c) => Message::Str(c.to_string()),
        Key::Ctrl(c) => Message::Keyword(value::intern(&format!("ctrl-{c}"))),
        Key::Alt(c) => Message::Keyword(value::intern(&format!("alt-{c}"))),
        Key::CtrlAlt(c) => Message::Keyword(value::intern(&format!("ctrl-meta-{c}"))),
        Key::Named(s) => Message::Keyword(value::intern(s)),
    }
}

/// A key *release*, as the `[:key-up <key>]` vector — the press value (what
/// `key_message` yields) tagged so the app can tell it from a press. The press
/// itself stays the bare value, so existing dispatch is untouched; release is
/// purely additive. Apps pair down→up to track a held key and drive their own
/// repeat (consumer-paced), rather than relying on the OS auto-repeat we drop.
pub(super) fn key_up_message(k: &Key) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("key-up")),
        key_message(k),
    ])
}

/// A mouse event as the shared `[:mouse action button row col mods]` vector,
/// built as a `Message` (no heap). Mirrors `builtins::mouse_to_value`'s shape so
/// the two frontends stay identical.
pub(super) fn mouse_message(m: &Mouse) -> Message {
    let action = match m.action {
        MouseAction::Press => "press",
        MouseAction::Release => "release",
        MouseAction::Drag => "drag",
        MouseAction::Move => "move",
        MouseAction::ScrollUp => "scroll-up",
        MouseAction::ScrollDown => "scroll-down",
    };
    let button = match m.button {
        Some(MouseButton::Left) => Message::Keyword(value::intern("left")),
        Some(MouseButton::Right) => Message::Keyword(value::intern("right")),
        Some(MouseButton::Middle) => Message::Keyword(value::intern("middle")),
        None => Message::Nil,
    };
    // Held modifiers, in a stable order (ctrl, alt, shift); empty `[]` when none.
    let mut mods = Vec::new();
    if m.ctrl {
        mods.push(Message::Keyword(value::intern("ctrl")));
    }
    if m.alt {
        mods.push(Message::Keyword(value::intern("alt")));
    }
    if m.shift {
        mods.push(Message::Keyword(value::intern("shift")));
    }
    let mut v = vec![
        Message::Keyword(value::intern("mouse")),
        Message::Keyword(value::intern(action)),
        button,
        Message::Int(m.row as i64),
        Message::Int(m.col as i64),
        Message::Vector(mods),
    ];
    // A press carries its click-chain count as a trailing 7th element.
    // A scroll carries the line-unit delta as a trailing 7th element (float).
    // Mirrors `builtins::mouse_value`.
    if m.count > 0 {
        v.push(Message::Int(m.count as i64));
    } else if m.scroll_dy != 0.0 {
        v.push(Message::Float(m.scroll_dy));
    }
    Message::Vector(v)
}

/// A synthetic release of held button `b` at cell `(col, row)` — delivered when the
/// pointer leaves the window or focus is lost while a button is down, so its real
/// (off-window) release can't strand the app thinking the button is still pressed.
pub(super) fn release_of(b: MouseButton, col: u16, row: u16, mods: &ModifiersState) -> Mouse {
    Mouse {
        action: MouseAction::Release,
        button: Some(b),
        row,
        col,
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        shift: mods.shift_key(),
        count: 0,
        scroll_dy: 0.0,
    }
}

/// A resize event as the `[:resize cols rows]` vector (the new cell grid),
/// built as a `Message` (no heap) so the GUI thread can deliver it to a
/// mailbox. Wakes the app loop so it re-renders at the new size instead of
/// waiting out its poll timeout.
pub(super) fn resize_message(cols: u16, rows: u16) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("resize")),
        Message::Int(cols as i64),
        Message::Int(rows as i64),
    ])
}

/// Deliver a scroll event to a window's subscriber. `dy > 0` is scroll-up, `dy < 0` is
/// scroll-down. Modifiers come from the window's current modifier state. Used both for
/// live gesture events and the kinetic-momentum synthetic events fired after gesture end.
pub(super) fn deliver_scroll(w: &Win, dy: f64) {
    let action = if dy > 0.0 {
        MouseAction::ScrollUp
    } else {
        MouseAction::ScrollDown
    };
    let (col, row) = w.cursor;
    deliver(
        w.subscriber,
        mouse_message(&Mouse {
            action,
            button: None,
            row,
            col,
            ctrl: w.mods.control_key(),
            alt: w.mods.alt_key(),
            shift: w.mods.shift_key(),
            count: 0,
            scroll_dy: dy.abs(),
        }),
    );
}

pub(super) fn translate_button(b: WMouseButton) -> Option<MouseButton> {
    match b {
        WMouseButton::Left => Some(MouseButton::Left),
        WMouseButton::Right => Some(MouseButton::Right),
        WMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

/// The US-layout shifted form of a base character — `.` → `>`, `1` → `!`, etc.
/// Used to re-apply Shift to a modifier chord whose base came from
/// `key_without_modifiers()` (which drops Shift along with Ctrl/Alt). Letters and
/// anything without a shifted punctuation form pass through unchanged (a letter's
/// shift is just upper-case, and the chord is lower-cased anyway). Matches the glyphs
/// the crossterm frontend reports for the same physical chord.
pub(super) fn shift_char(c: char) -> char {
    match c {
        '`' => '~',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        other => other,
    }
}

pub(super) fn translate_key(ke: &KeyEvent, mods: ModifiersState) -> Option<Key> {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    match &ke.logical_key {
        WKey::Named(n) => {
            // The named keys carry their modifiers in the name (`crate::host::gui::named_key`,
            // the rule both frontends share): `:ctrl-left`, `:alt-shift-up`, … — so the
            // editor binds `C-<left>` / `C-S-<arrow>` as distinctly as `:shift-up`
            // (shift-select) from a plain arrow. Tab and Escape keep their own spellings.
            let with_mods = |base: &'static str| {
                Key::Named(named_key(
                    base,
                    mods.control_key(),
                    mods.alt_key(),
                    mods.shift_key(),
                ))
            };
            Some(match n {
                NamedKey::ArrowUp => with_mods("up"),
                NamedKey::ArrowDown => with_mods("down"),
                NamedKey::ArrowLeft => with_mods("left"),
                NamedKey::ArrowRight => with_mods("right"),
                NamedKey::Enter => with_mods("enter"),
                NamedKey::Escape => Key::Named("escape"),
                NamedKey::Backspace => with_mods("backspace"),
                // Shift+Tab is back-tab — match the crossterm frontend's :back-tab.
                NamedKey::Tab if mods.shift_key() => Key::Named("back-tab"),
                NamedKey::Tab => Key::Named("tab"),
                NamedKey::Delete => with_mods("delete"),
                NamedKey::Home => with_mods("home"),
                NamedKey::End => with_mods("end"),
                NamedKey::PageUp => with_mods("page-up"),
                NamedKey::PageDown => with_mods("page-down"),
                // Space carries modifiers like a character key would, so Ctrl/Alt
                // survive (Emacs `C-SPC` set-mark → :ctrl- , `C-M-SPC` mark-sexp →
                // :ctrl-meta- , matching crossterm) rather than collapsing to a
                // self-inserted space. The Ctrl+Alt arm must come first.
                NamedKey::Space if mods.control_key() && mods.alt_key() => Key::CtrlAlt(' '),
                NamedKey::Space if mods.control_key() => Key::Ctrl(' '),
                NamedKey::Space if mods.alt_key() => Key::Alt(' '),
                NamedKey::Space => Key::Char(' '),
                _ => return None,
            })
        }
        WKey::Character(s) => {
            // For a Ctrl/Alt chord, read the key WITHOUT modifiers, so layout
            // composition (on some layouts Alt+`-` composes to en-dash `–`, Alt+
            // letters to accents) doesn't mangle the chord — the keymap binds the
            // BASE character (`-`, `f`). Plain typing keeps the composed/logical
            // char, so AltGr and dead keys still insert their glyph.
            let base = if mods.control_key() || mods.alt_key() {
                match ke.key_without_modifiers() {
                    WKey::Character(b) => b.chars().next(),
                    _ => s.chars().next(),
                }
            } else {
                s.chars().next()
            }?;
            // `key_without_modifiers()` also strips SHIFT, so a shifted-punctuation
            // chord (Emacs `M->` = Alt+Shift+`.`) would lose its shift and arrive as
            // `alt-.` — never matching the `alt->` binding. Re-apply Shift via the
            // US-layout map so the chord reaches the shifted glyph it names (`>`, `<`,
            // `{`, `}`, `%`, `^`, …), matching what the crossterm frontend already
            // delivers (`builtins::key_to_value`). Letters are untouched here (they're
            // lowercased below); plain typing never reaches this (it keeps `s`).
            let c = if (mods.control_key() || mods.alt_key()) && mods.shift_key() {
                shift_char(base)
            } else {
                base
            };
            if mods.control_key() && mods.alt_key() {
                Some(Key::CtrlAlt(c.to_ascii_lowercase()))
            } else if mods.control_key() {
                Some(Key::Ctrl(c.to_ascii_lowercase()))
            } else if mods.alt_key() {
                // Meta is case-SENSITIVE in Emacs (`M-O` open-line-above ≠ `M-o`): a
                // shifted letter stays upper-case so the two are distinct, while an
                // unshifted chord lower-cases (so Caps Lock / a stray Shift can't change
                // the binding). Control chords (above) stay case-insensitive, as in Emacs.
                Some(Key::Alt(if mods.shift_key() {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }))
            } else {
                Some(Key::Char(c))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod shift_char_tests {
    use super::shift_char;
    #[test]
    fn maps_us_shifted_punctuation() {
        assert_eq!(shift_char('.'), '>'); // Emacs M-> (end-of-buffer)
        assert_eq!(shift_char(','), '<'); // M-< (beginning-of-buffer)
        assert_eq!(shift_char('['), '{');
        assert_eq!(shift_char(']'), '}'); // M-{ / M-} paragraph motion
        assert_eq!(shift_char('5'), '%'); // M-% (query-replace)
        assert_eq!(shift_char('6'), '^'); // M-^ (join-line)
        assert_eq!(shift_char('f'), 'f'); // letters pass through (lower-cased later)
    }
}
