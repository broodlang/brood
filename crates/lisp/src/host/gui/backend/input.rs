//! Input translation: winit key and mouse events into the frontend-neutral
//! [`Key`]/[`Mouse`] vocabulary an app `receive`s, plus the click-chain and scroll
//! bookkeeping that turns raw events into `[:key …]` / `[:mouse …]` messages.

use super::*;
use crate::host::gui::{KeyModifiers, OptionMeta, character_chord, named_key};

/// Max gap between consecutive presses (same button, same cell) that still counts
/// as part of one click chain — the double/triple-click window, in milliseconds.
pub(super) const MULTI_CLICK_MS: u128 = 400;

/// A key as the Brood value `term-poll`/`gui` deliver: a printable → a 1-char
/// string, the rest → keywords. Built as a `Message` (no heap) so the GUI thread
/// can deliver it straight to a mailbox (ADR-059). Mirrors `key_to_value`.
pub(super) fn key_message(k: &Key) -> Message {
    match k {
        Key::Char(c) => Message::Str(c.to_string()),
        Key::Named(s) => Message::Keyword(value::intern(s)),
    }
}

/// What a press delivers: the key value (`key_message`), or `[:text "…"]` for a keystroke
/// that typed several characters.
pub(super) fn keystroke_message(stroke: &Keystroke) -> Message {
    match stroke {
        Keystroke::Key(k) => key_message(k),
        Keystroke::Text(text) => text_message(text),
    }
}

/// A key *release*, as the `[:key-up <press>]` vector — what its press delivered
/// (`keystroke_message`) tagged so the app can tell it from a press. The press
/// itself stays the bare value, so existing dispatch is untouched; release is
/// purely additive. An app that repeats a held key itself (`{:key-repeat :app}`)
/// pairs down→up to know when to stop.
pub(super) fn key_up_message(stroke: &Keystroke) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("key-up")),
        keystroke_message(stroke),
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
    // The pointer's exact position, as a trailing `{:at [row col]}` in FRACTIONAL window
    // cells — after the 7th slot, which is filled with nil when there is no count or delta
    // so every consumer reading those slots by position reads what it always did.
    if let Some((row, col)) = m.at {
        if v.len() == 6 {
            v.push(Message::Nil);
        }
        let mut extra = vec![(
            Message::Keyword(value::intern("at")),
            Message::Vector(vec![Message::Float(row), Message::Float(col)]),
        )];
        // in the strip a frame paints its next line into (ADR-615): `row` is that line's
        if m.partial {
            extra.push((
                Message::Keyword(value::intern("partial")),
                Message::Bool(true),
            ));
        }
        v.push(Message::Map(extra));
    }
    Message::Vector(v)
}

/// A synthetic release of held button `b` at cell `(col, row)` — delivered when the
/// pointer leaves the window or focus is lost while a button is down, so its real
/// (off-window) release can't strand the app thinking the button is still pressed.
pub(super) fn release_of(
    b: MouseButton,
    col: u16,
    row: u16,
    at: Option<(f64, f64)>,
    partial: bool,
    mods: &ModifiersState,
) -> Mouse {
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
        at,
        partial,
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

/// The `[:resize w h]` a PIXEL-input window gets: the same shape as `resize_message`,
/// in physical pixels — the unit every other message of that window uses.
pub(super) fn resize_message_px(w: u32, h: u32) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("resize")),
        Message::Int(w as i64),
        Message::Int(h as i64),
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
            at: w.pointer_at(),
            partial: w.pointer_partial(),
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

/// winit's modifier state as the frontend-neutral set a key name spells.
pub(super) fn key_modifiers(mods: ModifiersState) -> KeyModifiers {
    KeyModifiers {
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        shift: mods.shift_key(),
        super_key: mods.super_key(),
    }
}

/// The base name of a named key that carries its modifiers in the name — the arrows,
/// editing keys, F1–F24 and the rest of the keys a keyboard has beyond the typewriter
/// ones (ADR-441). Tab, Escape and Space are not here: each has its own rule in
/// `translate_key`. The names are the terminal frontend's (`builtins::key_to_value`)
/// and `std/vt`'s, so a key reads the same in a window, in a terminal and to a program
/// under a pty. Lock keys, PrintScreen and bare modifiers are not keys an app binds.
fn named_key_base(key: &NamedKey) -> Option<&'static str> {
    Some(match key {
        NamedKey::ArrowUp => "up",
        NamedKey::ArrowDown => "down",
        NamedKey::ArrowLeft => "left",
        NamedKey::ArrowRight => "right",
        NamedKey::Enter => "enter",
        NamedKey::Backspace => "backspace",
        NamedKey::Delete => "delete",
        NamedKey::Insert => "insert",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::PageUp => "page-up",
        NamedKey::PageDown => "page-down",
        NamedKey::ContextMenu => "menu",
        NamedKey::Pause => "pause",
        NamedKey::ScrollLock => "scroll-lock",
        NamedKey::Help => "help",
        NamedKey::Find => "find",
        NamedKey::Undo => "undo",
        NamedKey::Redo => "redo",
        NamedKey::Copy => "copy",
        NamedKey::Cut => "cut",
        NamedKey::Paste => "paste",
        NamedKey::Select => "select",
        NamedKey::Execute => "execute",
        NamedKey::Cancel => "cancel",
        NamedKey::Clear => "clear",
        NamedKey::BrowserBack => "back",
        NamedKey::BrowserForward => "forward",
        NamedKey::AudioVolumeUp => "volume-up",
        NamedKey::AudioVolumeDown => "volume-down",
        NamedKey::AudioVolumeMute => "volume-mute",
        NamedKey::MediaPlayPause => "media-play-pause",
        NamedKey::MediaStop => "media-stop",
        NamedKey::MediaTrackNext => "media-next",
        NamedKey::MediaTrackPrevious => "media-previous",
        NamedKey::F1 => "f1",
        NamedKey::F2 => "f2",
        NamedKey::F3 => "f3",
        NamedKey::F4 => "f4",
        NamedKey::F5 => "f5",
        NamedKey::F6 => "f6",
        NamedKey::F7 => "f7",
        NamedKey::F8 => "f8",
        NamedKey::F9 => "f9",
        NamedKey::F10 => "f10",
        NamedKey::F11 => "f11",
        NamedKey::F12 => "f12",
        NamedKey::F13 => "f13",
        NamedKey::F14 => "f14",
        NamedKey::F15 => "f15",
        NamedKey::F16 => "f16",
        NamedKey::F17 => "f17",
        NamedKey::F18 => "f18",
        NamedKey::F19 => "f19",
        NamedKey::F20 => "f20",
        NamedKey::F21 => "f21",
        NamedKey::F22 => "f22",
        NamedKey::F23 => "f23",
        NamedKey::F24 => "f24",
        _ => return None,
    })
}

/// The glyph a chord with Shift held names (Emacs `M->` is Alt+Shift+`.`). The layout
/// decides it: winit's `logical_key` has Shift (and the layout's levels) applied but not
/// Control, so on a German layout Alt+Shift+`.` is `M-:` and Alt+Shift+`<` is `M->`, as
/// the keycaps say. Two cases fall back: a logical key that is not plain printable ASCII
/// — macOS applies Option to it too (Option+Shift+`.` is `˘`), which is why chords read
/// `key_without_modifiers` in the first place — goes through the US table on the base key
/// (`shift_char`), and a letter keeps its base (the case is decided by the caller).
pub(super) fn shifted_chord_char(logical: Option<char>, base: char) -> char {
    match logical {
        Some(glyph) if glyph.is_ascii_graphic() && glyph != base => glyph,
        _ if base.is_alphabetic() => base,
        _ => shift_char(base),
    }
}

/// One key event as the translation reads it: winit's `KeyEvent` without the platform
/// parts a test cannot build, so the whole OS-key-to-Brood-key rule is a pure function
/// (`translate_keystroke`) a unit test drives with any layout's keys (ADR-541).
pub(super) struct KeyPress<'event> {
    /// What the layout says the key is, Shift and the layout's levels applied.
    pub(super) logical: &'event WKey,
    /// The text the keystroke typed, dead keys and compose applied — none on a release.
    pub(super) text: Option<&'event str>,
    /// The key with no modifier applied, in the ACTIVE layout (`key_without_modifiers`):
    /// `ф` for the A key under a Russian layout.
    pub(super) unmodified: WKey,
    /// Where the key is on the keyboard, whatever the layout.
    pub(super) physical: PhysicalKey,
}

impl<'event> KeyPress<'event> {
    pub(super) fn of(event: &'event KeyEvent) -> KeyPress<'event> {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        KeyPress {
            logical: &event.logical_key,
            text: event.text.as_deref(),
            unmodified: event.key_without_modifiers(),
            physical: event.physical_key,
        }
    }
}

/// The modifiers a key event is translated under: winit's state, with Alt dropped when
/// the only Option keys held are ones that compose (`OptionMeta`, macOS). `left_option` /
/// `right_option` are the sides the OS reports down — both false off macOS, where Alt is
/// always Meta (AltGr is never Alt to the OS on Linux or Windows, so it composes there
/// already).
pub(super) fn chord_modifiers(
    mods: ModifiersState,
    left_option: bool,
    right_option: bool,
    option_meta: OptionMeta,
) -> KeyModifiers {
    KeyModifiers {
        alt: mods.alt_key() && option_meta.alt_is_meta(left_option, right_option),
        ..key_modifiers(mods)
    }
}

/// True for a character a chord can be bound by as the layout gives it: ASCII, or a
/// letter or sign of the Latin script (`ö`, `ç`, `ß`, `§`). A Cyrillic, Greek, Hebrew,
/// Arabic, … letter is not — Emacs's keymaps spell `C-x`, not `C-ч`.
pub(super) fn is_latin(c: char) -> bool {
    c.is_ascii()
        || matches!(c,
            '\u{a0}'..='\u{24f}'          // Latin-1 Supplement, Latin Extended-A and -B
            | '\u{1e00}'..='\u{1eff}'     // Latin Extended Additional
            | '\u{2c60}'..='\u{2c7f}'     // Latin Extended-C
            | '\u{a720}'..='\u{a7ff}'     // Latin Extended-D
            | '\u{ab30}'..='\u{ab6f}') // Latin Extended-E
}

/// The character the key at `physical` types on a US QWERTY layout, unshifted — what a
/// chord falls back to when the active layout's character for it is not Latin.
pub(super) fn us_layout_char(physical: PhysicalKey) -> Option<char> {
    use winit::keyboard::KeyCode;
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    Some(match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Backquote => '`',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        _ => return None,
    })
}

/// True for the physical keys that are LETTERS on a US layout — the keys a layout's
/// script is told by.
fn is_letter_position(physical: PhysicalKey) -> bool {
    us_layout_char(physical).is_some_and(|c| c.is_ascii_lowercase())
}

/// Whether the active layout types a non-Latin script, after `press`: a letter key's
/// unmodified character says so (`ф` yes, `a` or `é` no), any other key leaves `before`
/// standing. Kept per window across presses because some keys of a non-Latin layout type
/// ASCII punctuation — Greek's Q key is `;`, Hebrew's is `/` — and a chord on one of them
/// must still be `C-q`, while a Dvorak layout's `'` on the same key must stay `C-'`.
pub(super) fn layout_is_non_latin(press: &KeyPress, before: bool) -> bool {
    if !is_letter_position(press.physical) {
        return before;
    }
    match &press.unmodified {
        WKey::Character(s) => match s.chars().next() {
            Some(c) if c.is_alphabetic() => !is_latin(c),
            _ => before,
        },
        _ => before,
    }
}

/// The text a plain keystroke (no Control / Alt / Super) typed, when it is MORE than one
/// character — a compose sequence or a layout key that types several. Delivered as
/// `[:text "…"]` rather than squeezed into one key (the first character was all that
/// survived before ADR-441). A one-character keystroke is not text: it stays the 1-char
/// key string `translate_key` makes of it, so every keymap that binds a character binds
/// it however it was typed.
pub(super) fn typed_text(press: &KeyPress, modifiers: KeyModifiers) -> Option<String> {
    if modifiers.ctrl || modifiers.alt || modifiers.super_key {
        return None;
    }
    let WKey::Character(_) = press.logical else {
        return None;
    };
    let text = press.text?;
    let mut chars = text.chars();
    chars.next()?;
    (chars.next().is_some() && !text.chars().any(char::is_control)).then(|| text.to_string())
}

/// A bare modifier or lock key: a press the app is never sent and so never answers with a
/// frame — the latency stamp (ADR-440) waits for the key it modifies instead.
pub(super) fn is_modifier_key(ke: &KeyEvent) -> bool {
    matches!(
        ke.logical_key,
        WKey::Named(
            NamedKey::Shift
                | NamedKey::Control
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Meta
                | NamedKey::Hyper
                | NamedKey::CapsLock
                | NamedKey::NumLock
                | NamedKey::Fn
                | NamedKey::FnLock
                | NamedKey::Symbol
                | NamedKey::SymbolLock
        )
    )
}

/// What one press delivers (ADR-441, ADR-541): the text it typed when that is several
/// characters (`typed_text`), else its key (`translate_key`). `modifiers` are the ones the
/// chord is read under (`chord_modifiers`), `non_latin_layout` what `layout_is_non_latin`
/// last said.
pub(super) fn translate_keystroke(
    press: &KeyPress,
    modifiers: KeyModifiers,
    non_latin_layout: bool,
) -> Option<Keystroke> {
    typed_text(press, modifiers)
        .map(Keystroke::Text)
        .or_else(|| translate_key(press, modifiers, non_latin_layout).map(Keystroke::Key))
}

/// The character a key names in a chord, and whether it is the LAYOUT's (true) or the US
/// position's fallback (false). The key read WITHOUT modifiers, so the layout's
/// composition (some layouts' Alt+`-` is `–`, macOS Option+letters accents) does not mangle
/// the chord — the keymap binds the base character (`-`, `f`). A dead key in a chord is the
/// accent it carries (German Ctrl+`^`), where it used to be dropped. And when that character
/// is not Latin — the A key under a Russian layout is `ф` — or is punctuation on a letter
/// key of a non-Latin layout (Greek's `;` on Q), the chord is the key's US identity:
/// `C-ф` is `C-a`, as in Emacs, so `C-x C-f` works whichever layout is active.
fn chord_base(press: &KeyPress, non_latin_layout: bool) -> Option<(char, bool)> {
    let character = |key: &WKey| match key {
        WKey::Character(s) => s.chars().next(),
        WKey::Dead(accent) => *accent,
        _ => None,
    };
    let layout = character(&press.unmodified).or_else(|| character(press.logical));
    let positional = us_layout_char(press.physical);
    match layout {
        Some(c)
            if is_latin(c)
                && !(non_latin_layout
                    && is_letter_position(press.physical)
                    && !c.is_alphabetic()) =>
        {
            Some((c, true))
        }
        _ => positional.map(|c| (c, false)).or(layout.map(|c| (c, true))),
    }
}

/// The key one press (or release) names, or none for a key the app is never sent (a bare
/// modifier, a dead key's half of a composition).
pub(super) fn translate_key(
    press: &KeyPress,
    modifiers: KeyModifiers,
    non_latin_layout: bool,
) -> Option<Key> {
    let chord = modifiers.ctrl || modifiers.alt || modifiers.super_key;
    match press.logical {
        // Space carries modifiers like a character key would, so Ctrl/Alt survive (Emacs
        // `C-SPC` set-mark → :ctrl- , `C-M-SPC` mark-sexp → :ctrl-meta- , matching
        // crossterm) rather than collapsing to a self-inserted space.
        WKey::Named(NamedKey::Space) => Some(character_chord(' ', modifiers)),
        // Tab as both frontends spell it (`tab_key`): Shift+Tab `:back-tab`, Ctrl+Tab
        // `:ctrl-tab`, Ctrl+Shift+Tab `:ctrl-shift-tab`.
        WKey::Named(NamedKey::Tab) => Some(Key::Named(crate::host::gui::tab_key(modifiers))),
        WKey::Named(NamedKey::Escape) => Some(Key::Named("escape")),
        // The named keys carry their modifiers in the name (`crate::host::gui::named_key`,
        // the rule both frontends share): `:ctrl-left`, `:alt-shift-up`, `:super-f5`, … —
        // so the editor binds `C-<left>` / `C-S-<arrow>` as distinctly as `:shift-up`
        // (shift-select) from a plain arrow.
        WKey::Named(named) => Some(Key::Named(named_key(named_key_base(named)?, modifiers))),
        WKey::Character(s) if !chord => {
            // Plain typing keeps what the key TYPED: `text` has dead keys and compose
            // applied (`´` then `e` types `é`), where the logical key is the bare `e`, and
            // is the layout's own character (`ф`, `@` from AltGr+Q). A release has no text,
            // so it falls back to the logical key — a release is matched to its press by
            // the physical key, never by this.
            let typed = press.text.and_then(|t| t.chars().next());
            let c = typed
                .filter(|c| !c.is_control())
                .or_else(|| s.chars().next())?;
            Some(Key::Char(c))
        }
        // A dead key alone starts a composition: the character it ends in arrives as text.
        WKey::Character(_) | WKey::Dead(_) | WKey::Unidentified(_) if chord => {
            let (base, from_layout) = chord_base(press, non_latin_layout)?;
            // `chord_base` strips SHIFT too, so a chord with Shift held takes its glyph
            // from the layout (`shifted_chord_char`): Emacs `M->` = Alt+Shift+`.` must reach
            // the `alt->` binding, not `alt-.`. A chord read by its US position takes the
            // US glyph: the layout's shifted character is for a different key.
            let c = if !modifiers.shift {
                base
            } else if from_layout {
                let logical = match press.logical {
                    WKey::Character(s) => s.chars().next(),
                    _ => None,
                };
                shifted_chord_char(logical, base)
            } else {
                shifted_chord_char(None, base)
            };
            Some(character_chord(c, modifiers))
        }
        _ => None,
    }
}

// ---- input methods and focus ------------------------------------------------------------

/// A commit of composed text as `[:text "…"]` — what an input method (or a keystroke
/// that typed several characters) inserts at point.
pub(super) fn text_message(text: &str) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("text")),
        Message::Str(text.to_string()),
    ])
}

/// The input method's composition in progress, as `[:preedit "…" [from to]]`: the text
/// to show at point (not yet inserted), and the IME's cursor inside it as a CHARACTER
/// range — winit reports bytes, which a Brood string does not index by — or nil when the
/// IME hides it. An empty text with nil clears the composition.
pub(super) fn preedit_message(text: &str, cursor: Option<(usize, usize)>) -> Message {
    let char_index = |byte: usize| text[..byte.min(text.len())].chars().count() as i64;
    let range = match cursor {
        Some((from, to)) if text.is_char_boundary(from) && text.is_char_boundary(to) => {
            Message::Vector(vec![
                Message::Int(char_index(from)),
                Message::Int(char_index(to)),
            ])
        }
        _ => Message::Nil,
    };
    Message::Vector(vec![
        Message::Keyword(value::intern("preedit")),
        Message::Str(text.to_string()),
        range,
    ])
}

/// What an IME commit becomes. A single character typed with NO composition shown is a
/// keystroke an input method passed through (some IMEs commit every key they do not
/// compose), so it is the 1-char key string a keyboard would have sent — the keymap
/// cannot tell the difference, and must not. Anything that ended a composition, or is
/// longer than one character, is `[:text "…"]`.
pub(super) fn commit_message(text: &str, after_preedit: bool) -> Option<Message> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if !after_preedit && chars.next().is_none() && !first.is_control() {
        Some(key_message(&Key::Char(first)))
    } else {
        Some(text_message(text))
    }
}

/// Where the IME's candidate window goes: the first `Op::Cursor` of the frame, as a
/// grid-relative PIXEL rect `(x, y, w, h)` one cell in size, walking INTO the regions
/// with the coordinate math `render_ops` paints them with (a cursor in a scrolled or
/// zoomed pane is placed where it is drawn). `None` when the frame has no cursor.
pub(super) fn ime_cursor_area(ops: &[Op], r: &Renderer) -> Option<(isize, isize, usize, usize)> {
    find_cursor(ops, r, 0, 0, r.cell_w.max(1), r.cell_h.max(1), 0)
}

fn find_cursor(
    ops: &[Op],
    r: &Renderer,
    ox: isize,
    oy: isize,
    cw: usize,
    ch: usize,
    scroll_dy: isize,
) -> Option<(isize, isize, usize, usize)> {
    ops.iter().find_map(|op| match op {
        Op::Cursor { row, col, .. } => Some((
            ox.saturating_add((*col as isize).saturating_mul(cw as isize)),
            oy.saturating_add((*row as isize).saturating_mul(ch as isize))
                .saturating_sub(scroll_dy),
            cw,
            ch,
        )),
        Op::ScrollRegion { dy_frac, ops } => {
            find_cursor(ops, r, ox, oy, cw, ch, scroll_px(*dy_frac, ch))
        }
        Op::Fragment(fragment) => find_cursor(&fragment.0, r, ox, oy, cw, ch, scroll_dy),
        // laid out on the uniform rows already, the space this walk measures in (ADR-615)
        Op::AboveSeam { ops } => find_cursor(ops, r, ox, oy, cw, ch, scroll_dy),
        Op::CellRegion { x, y, px, ops, .. } => {
            let region = r.metrics_at(*px);
            if region.cell_w == 0 || region.cell_h == 0 {
                return None;
            }
            find_cursor(
                ops,
                r,
                ox.saturating_add((*x as isize).saturating_mul(cw as isize)),
                oy.saturating_add((*y as isize).saturating_mul(ch as isize))
                    .saturating_sub(scroll_dy),
                region.cell_w,
                region.cell_h,
                0,
            )
        }
        _ => None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn press_at(at: Option<(f64, f64)>, count: u8) -> Vec<Message> {
        let m = Mouse {
            action: MouseAction::Press,
            button: Some(MouseButton::Left),
            row: 3,
            col: 7,
            ctrl: false,
            alt: false,
            shift: false,
            count,
            scroll_dy: 0.0,
            at,
            partial: false,
        };
        // `Message` implements `Drop` (its children are freed iteratively), so the vector
        // is taken out by `mem::take` rather than moved out of the pattern.
        match &mut mouse_message(&m) {
            Message::Vector(v) => std::mem::take(v),
            _ => panic!("a mouse message is a vector"),
        }
    }

    #[test]
    fn the_exact_position_trails_the_positional_slots() {
        // a press: slot 6 stays the click count, the position follows it
        let v = press_at(Some((3.75, 7.25)), 1);
        assert_eq!(v.len(), 8);
        assert!(matches!(v[3], Message::Int(3)));
        assert!(matches!(v[6], Message::Int(1)));
        let Message::Map(entries) = &v[7] else {
            panic!("the position is a map")
        };
        let (_, Message::Vector(at)) = &entries[0] else {
            panic!("{{:at [row col]}}")
        };
        assert!(matches!(at[0], Message::Float(r) if r == 3.75));
        assert!(matches!(at[1], Message::Float(c) if c == 7.25));
    }

    #[test]
    fn with_nothing_in_slot_six_it_is_nil_so_the_slots_keep_their_places() {
        let v = press_at(Some((1.5, 2.5)), 0);
        assert_eq!(v.len(), 8);
        assert!(matches!(v[6], Message::Nil));
    }

    #[test]
    fn no_position_is_the_message_it_always_was() {
        let v = press_at(None, 2);
        assert_eq!(v.len(), 7);
    }
}

#[cfg(test)]
mod adr_431_tests {
    use super::*;

    fn modifiers(ctrl: bool, alt: bool, shift: bool, super_key: bool) -> KeyModifiers {
        KeyModifiers {
            ctrl,
            alt,
            shift,
            super_key,
        }
    }

    fn keyword_of(key: Key) -> String {
        match key {
            Key::Named(name) => name.to_string(),
            Key::Char(c) => c.to_string(),
        }
    }

    #[test]
    fn a_shifted_chord_takes_the_layouts_glyph() {
        // US: Alt+Shift+. is M->
        assert_eq!(shifted_chord_char(Some('>'), '.'), '>');
        // German: Shift+. is `:` and Shift+7 is `/` — the keycaps, not the US table
        assert_eq!(shifted_chord_char(Some(':'), '.'), ':');
        assert_eq!(shifted_chord_char(Some('/'), '7'), '/');
        // macOS applies Option to the logical key: the US table answers instead
        assert_eq!(shifted_chord_char(Some('˘'), '.'), '>');
        // a letter keeps its base; the case is the chord's business
        assert_eq!(shifted_chord_char(Some('Å'), 'a'), 'a');
        assert_eq!(shifted_chord_char(Some('A'), 'a'), 'A');
    }

    #[test]
    fn super_chords_spell_emacs_order() {
        assert_eq!(
            keyword_of(character_chord('x', modifiers(false, false, false, true))),
            "super-x"
        );
        assert_eq!(
            keyword_of(character_chord('S', modifiers(false, false, true, true))),
            "super-S"
        );
        assert_eq!(
            keyword_of(character_chord('S', modifiers(true, false, true, true))),
            "ctrl-super-s"
        );
        assert_eq!(
            keyword_of(character_chord('>', modifiers(false, true, true, true))),
            "alt-super->"
        );
    }

    #[test]
    fn chords_without_super_are_what_they_always_were() {
        assert_eq!(
            keyword_of(character_chord('f', modifiers(true, true, false, false))),
            "ctrl-meta-f"
        );
        assert_eq!(
            keyword_of(character_chord('O', modifiers(false, true, true, false))),
            "alt-O"
        );
        assert_eq!(
            keyword_of(character_chord('A', modifiers(true, false, true, false))),
            "ctrl-a"
        );
        assert_eq!(
            keyword_of(character_chord(' ', modifiers(true, false, false, false))),
            "ctrl- "
        );
        assert_eq!(
            keyword_of(character_chord(' ', modifiers(false, false, false, false))),
            " "
        );
    }

    #[test]
    fn named_keys_cover_the_function_and_editing_keys() {
        assert_eq!(named_key_base(&NamedKey::F1), Some("f1"));
        assert_eq!(named_key_base(&NamedKey::F24), Some("f24"));
        assert_eq!(named_key_base(&NamedKey::Insert), Some("insert"));
        assert_eq!(named_key_base(&NamedKey::ContextMenu), Some("menu"));
        assert_eq!(named_key_base(&NamedKey::PrintScreen), None);
        assert_eq!(named_key_base(&NamedKey::Shift), None);
    }

    fn vector_of(message: Message) -> Vec<Message> {
        match &mut { message } {
            Message::Vector(items) => std::mem::take(items),
            _ => panic!("a vector message"),
        }
    }

    #[test]
    fn a_preedit_cursor_is_counted_in_characters() {
        // "啊b" — the IME's cursor after the first character is byte 3
        let items = vector_of(preedit_message("啊b", Some((3, 3))));
        assert!(matches!(&items[1], Message::Str(s) if s == "啊b"));
        let Message::Vector(range) = &items[2] else {
            panic!("a range")
        };
        assert!(matches!(range[0], Message::Int(1)));
        assert!(matches!(range[1], Message::Int(1)));
        // the clearing preedit
        let cleared = vector_of(preedit_message("", None));
        assert!(matches!(cleared[2], Message::Nil));
    }

    #[test]
    fn a_passed_through_character_is_a_key_and_a_composition_is_text() {
        assert!(matches!(&commit_message("a", false), Some(Message::Str(s)) if s == "a"));
        let composed = vector_of(commit_message("é", true).expect("a commit"));
        assert!(matches!(&composed[1], Message::Str(s) if s == "é"));
        let several = vector_of(commit_message("啊不", false).expect("a commit"));
        assert!(matches!(&several[1], Message::Str(s) if s == "啊不"));
        assert!(commit_message("", true).is_none());
    }
}

#[cfg(test)]
mod keystroke_tests {
    use super::*;

    /// `message` is `[:<tag> payload]`; the payload.
    fn payload<'message>(message: &'message Message, tag: &str) -> &'message Message {
        match message {
            Message::Vector(parts) if parts.len() == 2 => {
                assert!(
                    matches!(parts[0], Message::Keyword(k) if k == value::intern(tag)),
                    "tagged :{tag}"
                );
                &parts[1]
            }
            _ => panic!("a two-element vector"),
        }
    }

    /// A release names what its press delivered: the key value for a key, `[:text …]` for
    /// a keystroke that typed several characters.
    #[test]
    fn a_release_is_tagged_with_what_its_press_delivered() {
        let release = key_up_message(&Keystroke::Key(Key::Char('(')));
        assert!(matches!(payload(&release, "key-up"), Message::Str(s) if s == "("));

        let release = key_up_message(&Keystroke::Text("ǘ̃".to_string()));
        let text = payload(payload(&release, "key-up"), "text");
        assert!(matches!(text, Message::Str(s) if s == "ǘ̃"));
    }

    #[test]
    fn a_keystroke_delivers_its_key_or_its_text() {
        assert!(matches!(
            keystroke_message(&Keystroke::Key(Key::Named("up"))),
            Message::Keyword(k) if k == value::intern("up")
        ));
        let press = keystroke_message(&Keystroke::Text("ab".to_string()));
        assert!(matches!(payload(&press, "text"), Message::Str(s) if s == "ab"));
    }
}

/// ADR-541: chords under a non-Latin layout read the key's US position, an Option key that
/// composes is not Meta, and a dead key in a chord is not dropped.
#[cfg(test)]
mod adr_541_tests {
    use super::*;
    use winit::keyboard::{KeyCode, SmolStr};

    fn character(s: &str) -> WKey {
        WKey::Character(SmolStr::new(s))
    }

    /// A press of the key at `code`: what the layout calls it (`logical`), the text it
    /// typed, and its unmodified character in the active layout.
    fn press<'event>(
        logical: &'event WKey,
        text: Option<&'event str>,
        unmodified: WKey,
        code: KeyCode,
    ) -> KeyPress<'event> {
        KeyPress {
            logical,
            text,
            unmodified,
            physical: PhysicalKey::Code(code),
        }
    }

    fn held(ctrl: bool, alt: bool, shift: bool) -> KeyModifiers {
        KeyModifiers {
            ctrl,
            alt,
            shift,
            super_key: false,
        }
    }

    /// What the press delivers, spelled: a key's name or character, `[:text …]`'s text in
    /// brackets, `-` for nothing.
    fn delivered(press: &KeyPress, modifiers: KeyModifiers, non_latin: bool) -> String {
        match translate_keystroke(press, modifiers, non_latin) {
            Some(Keystroke::Key(Key::Named(name))) => name.to_string(),
            Some(Keystroke::Key(Key::Char(c))) => c.to_string(),
            Some(Keystroke::Text(text)) => format!("[{text}]"),
            None => "-".to_string(),
        }
    }

    #[test]
    fn a_russian_chord_is_the_keys_us_identity_and_typing_stays_russian() {
        // Russian: the A key types ф, the X key ч, the F key а
        let ef = character("ф");
        let a_key = press(&ef, Some("ф"), character("ф"), KeyCode::KeyA);
        assert_eq!(delivered(&a_key, held(true, false, false), true), "ctrl-a");
        assert_eq!(delivered(&a_key, held(false, true, false), true), "alt-a");
        assert_eq!(
            delivered(&a_key, held(true, true, false), true),
            "ctrl-meta-a"
        );
        assert_eq!(delivered(&a_key, held(false, false, false), true), "ф");
        let che = character("ч");
        let x_key = press(&che, Some("ч"), character("ч"), KeyCode::KeyX);
        assert_eq!(delivered(&x_key, held(true, false, false), true), "ctrl-x");
        // whatever the window remembered of the layout: the character itself says it
        assert_eq!(delivered(&x_key, held(true, false, false), false), "ctrl-x");
        // Shift: the US glyph, not the layout's character for the key
        let big_ef = character("Ф");
        let shifted = press(&big_ef, Some("Ф"), character("ф"), KeyCode::KeyA);
        assert_eq!(delivered(&shifted, held(false, true, true), true), "alt-A");
        let big_yu = character("Ю");
        let period_key = press(&big_yu, Some("Ю"), character("ю"), KeyCode::Period);
        assert_eq!(
            delivered(&period_key, held(false, true, true), true),
            "alt->"
        );
        // the Russian `.` (on the US `/` key) is ASCII: the layout's own
        let period = character(".");
        let slash_key = press(&period, Some("."), character("."), KeyCode::Slash);
        assert_eq!(
            delivered(&slash_key, held(true, false, false), true),
            "ctrl-."
        );
    }

    #[test]
    fn greek_hebrew_and_arabic_chords_read_the_us_position() {
        let theta = character("θ");
        let u_key = press(&theta, Some("θ"), character("θ"), KeyCode::KeyU);
        assert_eq!(delivered(&u_key, held(true, false, false), true), "ctrl-u");
        // Greek's Q key types `;` — on a non-Latin layout a letter key is its letter
        let semicolon = character(";");
        let q_key = press(&semicolon, Some(";"), character(";"), KeyCode::KeyQ);
        assert_eq!(delivered(&q_key, held(true, false, false), true), "ctrl-q");
        assert_eq!(delivered(&q_key, held(false, false, false), true), ";");
        // Hebrew: the X key is ס, the W key `'`
        let samekh = character("ס");
        let x_key = press(&samekh, Some("ס"), character("ס"), KeyCode::KeyX);
        assert_eq!(delivered(&x_key, held(true, false, false), true), "ctrl-x");
        let apostrophe = character("'");
        let w_key = press(&apostrophe, Some("'"), character("'"), KeyCode::KeyW);
        assert_eq!(delivered(&w_key, held(true, false, false), true), "ctrl-w");
        // Arabic: the S key is س
        let seen = character("س");
        let s_key = press(&seen, Some("س"), character("س"), KeyCode::KeyS);
        assert_eq!(delivered(&s_key, held(true, false, false), true), "ctrl-s");
    }

    #[test]
    fn latin_layouts_keep_their_own_characters() {
        // Dvorak: the Q key types `'`, its chords are the layout's
        let apostrophe = character("'");
        let q_key = press(&apostrophe, Some("'"), character("'"), KeyCode::KeyQ);
        assert_eq!(delivered(&q_key, held(true, false, false), false), "ctrl-'");
        // and the X key types q
        let q = character("q");
        let x_key = press(&q, Some("q"), character("q"), KeyCode::KeyX);
        assert_eq!(delivered(&x_key, held(true, false, false), false), "ctrl-q");
        // German: ö is Latin, so C-ö stays C-ö
        let o_umlaut = character("ö");
        let semicolon_key = press(&o_umlaut, Some("ö"), character("ö"), KeyCode::Semicolon);
        assert_eq!(
            delivered(&semicolon_key, held(true, false, false), false),
            "ctrl-ö"
        );
        // AZERTY: the Q key types a
        let a = character("a");
        let azerty_q = press(&a, Some("a"), character("a"), KeyCode::KeyQ);
        assert_eq!(
            delivered(&azerty_q, held(true, false, false), false),
            "ctrl-a"
        );
    }

    #[test]
    fn the_layouts_script_is_told_by_its_letter_keys() {
        let ef = character("ф");
        let russian_a = press(&ef, Some("ф"), character("ф"), KeyCode::KeyA);
        assert!(layout_is_non_latin(&russian_a, false));
        let a = character("a");
        let us_a = press(&a, Some("a"), character("a"), KeyCode::KeyA);
        assert!(!layout_is_non_latin(&us_a, true));
        // punctuation, or a key that is not a letter's, says nothing either way
        let semicolon = character(";");
        let greek_q = press(&semicolon, Some(";"), character(";"), KeyCode::KeyQ);
        assert!(layout_is_non_latin(&greek_q, true));
        assert!(!layout_is_non_latin(&greek_q, false));
        let one = character("1");
        let digit = press(&one, Some("1"), character("1"), KeyCode::Digit1);
        assert!(layout_is_non_latin(&digit, true));
    }

    #[test]
    fn altgr_and_composing_option_text_inserts() {
        // AltGr+Q on a German layout: the OS reports no Alt (ISO_Level3_Shift on Linux,
        // Ctrl+Alt filtered on Windows), so the press is the `@` it typed
        let at = character("@");
        let q_key = press(&at, Some("@"), character("q"), KeyCode::KeyQ);
        assert_eq!(delivered(&q_key, held(false, false, false), false), "@");
        // macOS, right Option composing: Alt is held, but not Meta
        let modifiers = chord_modifiers(ModifiersState::ALT, false, true, OptionMeta::Left);
        assert!(!modifiers.alt);
        let digit_key = press(&at, Some("@"), character("2"), KeyCode::Digit2);
        assert_eq!(delivered(&digit_key, modifiers, false), "@");
        // left Option is Meta
        let modifiers = chord_modifiers(ModifiersState::ALT, true, false, OptionMeta::Left);
        assert_eq!(delivered(&digit_key, modifiers, false), "alt-2");
    }

    #[test]
    fn which_option_key_is_meta() {
        // no side reported (Linux, Windows): Alt is Meta whatever the option says
        for option in [
            OptionMeta::Left,
            OptionMeta::Right,
            OptionMeta::Both,
            OptionMeta::None,
        ] {
            assert!(option.alt_is_meta(false, false));
        }
        assert!(OptionMeta::Left.alt_is_meta(true, false));
        assert!(!OptionMeta::Left.alt_is_meta(false, true));
        assert!(OptionMeta::Right.alt_is_meta(false, true));
        assert!(!OptionMeta::Right.alt_is_meta(true, false));
        assert!(OptionMeta::Both.alt_is_meta(false, true));
        assert!(!OptionMeta::None.alt_is_meta(true, true));
        // both held, one of them Meta: Meta
        assert!(OptionMeta::Left.alt_is_meta(true, true));
        // Control and the rest are never touched
        let modifiers = chord_modifiers(
            ModifiersState::ALT | ModifiersState::CONTROL,
            false,
            true,
            OptionMeta::Left,
        );
        assert!(modifiers.ctrl && !modifiers.alt);
    }

    #[test]
    fn a_dead_key_in_a_chord_is_its_accent_and_alone_it_composes() {
        // German: the key left of 1 is a dead `^`
        let circumflex = WKey::Dead(Some('^'));
        let dead = press(&circumflex, None, WKey::Dead(Some('^')), KeyCode::Backquote);
        assert_eq!(delivered(&dead, held(true, false, false), false), "ctrl-^");
        assert_eq!(delivered(&dead, held(false, false, false), false), "-");
        // macOS Option+e without Option as Meta is a dead `´`; as Meta it is M-e
        let acute = WKey::Dead(Some('´'));
        let e_key = press(&acute, None, character("e"), KeyCode::KeyE);
        assert_eq!(delivered(&e_key, held(false, true, false), false), "alt-e");
        // a dead key naming no accent falls back to its position
        let unnamed = WKey::Dead(None);
        let silent = press(&unnamed, None, WKey::Dead(None), KeyCode::Quote);
        assert_eq!(
            delivered(&silent, held(true, false, false), false),
            "ctrl-'"
        );
    }

    #[test]
    fn composed_text_arrives_as_text_and_named_keys_keep_their_modifiers() {
        // dead `´` then `e`: the second press typed é — the logical key is the bare e
        let e = character("e");
        let composed = press(&e, Some("é"), character("e"), KeyCode::KeyE);
        assert_eq!(delivered(&composed, held(false, false, false), false), "é");
        // a compose sequence that typed several characters is text
        let several = press(&e, Some("ǘ̃"), character("e"), KeyCode::KeyE);
        assert_eq!(delivered(&several, held(false, false, false), false), "[ǘ̃]");
        // a non-text key under a non-Latin layout keeps every modifier
        let left = WKey::Named(NamedKey::ArrowLeft);
        let arrow = press(
            &left,
            None,
            WKey::Named(NamedKey::ArrowLeft),
            KeyCode::ArrowLeft,
        );
        assert_eq!(
            delivered(&arrow, held(true, false, true), true),
            "ctrl-shift-left"
        );
        let space = WKey::Named(NamedKey::Space);
        let space_key = press(
            &space,
            Some(" "),
            WKey::Named(NamedKey::Space),
            KeyCode::Space,
        );
        assert_eq!(
            delivered(&space_key, held(true, false, false), true),
            "ctrl- "
        );
        let tab = WKey::Named(NamedKey::Tab);
        let tab_key = press(&tab, Some("\t"), WKey::Named(NamedKey::Tab), KeyCode::Tab);
        assert_eq!(delivered(&tab_key, held(false, false, false), false), "tab");
        assert_eq!(
            delivered(&tab_key, held(false, false, true), false),
            "back-tab"
        );
        assert_eq!(
            delivered(&tab_key, held(true, false, false), false),
            "ctrl-tab"
        );
        assert_eq!(
            delivered(&tab_key, held(true, false, true), false),
            "ctrl-shift-tab"
        );
    }

    #[test]
    fn a_release_names_the_same_key_its_press_did() {
        // a release has no text; under a Russian layout it is still `C-a`'s key
        let ef = character("ф");
        let release = press(&ef, None, character("ф"), KeyCode::KeyA);
        assert!(matches!(
            translate_key(&release, held(true, false, false), true),
            Some(Key::Named(name)) if name == "ctrl-a"
        ));
        assert!(matches!(
            translate_key(&release, held(false, false, false), true),
            Some(Key::Char('ф'))
        ));
    }

    #[test]
    fn the_us_position_table_covers_the_typewriter_keys() {
        assert_eq!(us_layout_char(PhysicalKey::Code(KeyCode::KeyZ)), Some('z'));
        assert_eq!(
            us_layout_char(PhysicalKey::Code(KeyCode::Digit0)),
            Some('0')
        );
        assert_eq!(
            us_layout_char(PhysicalKey::Code(KeyCode::Backslash)),
            Some('\\')
        );
        assert_eq!(us_layout_char(PhysicalKey::Code(KeyCode::F1)), None);
        assert!(is_latin('a') && is_latin('ö') && is_latin('ß') && is_latin('ł'));
        assert!(!is_latin('ф') && !is_latin('θ') && !is_latin('ס') && !is_latin('س'));
    }
}
