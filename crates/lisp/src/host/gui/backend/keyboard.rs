//! A window's keyboard, apart from winit so its rules can be tested: which keystroke is
//! held, what every key down delivered, and the gate on the OS repeat (ADR-086, ADR-441).
//! The event loop hands it each press, release and focus loss and delivers what it answers.

use super::*;

/// What the Brood side reads of a window's keyboard — `gui-held-key` and
/// `gui-key-repeats` — kept current by the GUI thread.
#[derive(Default)]
pub(super) struct KeyReport {
    pub(super) held: Option<Keystroke>,
    pub(super) repeats: RepeatCounts,
}

/// One press's message for the app: a fresh press, or an OS repeat carrying the gate's
/// credit (`process::deliver_credited`), which the release can take back. A fresh press
/// carries a credit too, a receipt costing nothing (`PressReceipt`): every press marks, as
/// the app takes it from its mailbox, whether it was a repeat (`gui-key-repeat?`, ADR-612).
pub(super) enum KeyMessage {
    Press(Message, crate::process::DeliveryCredit),
    Repeat(Message, crate::process::DeliveryCredit),
    /// A key still down now types another keystroke (Shift let go while `(` repeats, so it
    /// repeats as `9`): the key-up of the one it typed, then the press of the new one, so
    /// every press still has its key-up.
    Restroke {
        up: Message,
        press: Message,
        receipt: crate::process::DeliveryCredit,
    },
}

pub(super) struct Keyboard {
    /// Who repeats a held key (`WindowSpec::key_repeat`).
    repeat: KeyRepeat,
    /// What the Brood side reads; `held` is the keystroke held down, set on a fresh press
    /// and cleared on its release or focus loss. A press of the keystroke already held is
    /// its repeat — told by transition, since winit's `ke.repeat` is unreliable on Wayland.
    report: Arc<Mutex<KeyReport>>,
    /// The PHYSICAL key of the held keystroke. A release is matched to it by physical key,
    /// not logical: a shifted chord (`(` = Shift+9) whose Shift comes up first releases
    /// as `9`, which would never clear a held `(` — the key stuck and the repeat running
    /// away. The physical key is the same down and up whatever the modifiers.
    held_physical: Option<PhysicalKey>,
    /// Every key down, by physical key, with what its press delivered, so its release
    /// names that press: `[:key-up "("]` though Shift came up first, `[:key-up [:text
    /// "…"]]` for a compose sequence. All of it goes on focus loss, since a release in
    /// another window is never seen.
    down: Vec<(PhysicalKey, Keystroke)>,
    /// At most one OS repeat undelivered, and that one taken back at the release.
    gate: Arc<RepeatGate>,
    /// When the held keystroke was pressed, and when it last repeated: the delay and
    /// interval `RepeatCounts` reports.
    pressed_at: Option<Instant>,
    repeated_at: Option<Instant>,
}

impl Keyboard {
    pub(super) fn new(repeat: KeyRepeat) -> Keyboard {
        Keyboard {
            repeat,
            report: Arc::default(),
            held_physical: None,
            down: Vec::new(),
            gate: Arc::default(),
            pressed_at: None,
            repeated_at: None,
        }
    }

    /// The state the Brood side reads, shared with the window's handle.
    pub(super) fn report(&self) -> Arc<Mutex<KeyReport>> {
        self.report.clone()
    }

    /// Whether the last press the app took from its mailbox was an OS repeat, shared with
    /// the window's handle for `gui-key-repeat?` (ADR-612).
    pub(super) fn received_repeat(&self) -> Arc<AtomicBool> {
        self.gate.received_repeat.clone()
    }

    /// One press of `stroke` from `physical` at `at`. A fresh press is always sent. A
    /// repeat is dropped under `KeyRepeat::App`, where the app repeats from `gui-held-key`;
    /// under `KeyRepeat::Os` it goes through the gate, so at most one is ever undelivered.
    pub(super) fn press(
        &mut self,
        stroke: Keystroke,
        physical: PhysicalKey,
        at: Instant,
    ) -> Option<KeyMessage> {
        let message = keystroke_message(&stroke);
        let mut report = lock(&self.report);
        if self.held_physical != Some(physical) || report.held.as_ref() != Some(&stroke) {
            let typed_before = self
                .down
                .iter()
                .position(|(down, _)| *down == physical)
                .map(|at| self.down.remove(at).1)
                .filter(|before| *before != stroke);
            self.down.push((physical, stroke.clone()));
            report.held = Some(stroke);
            self.held_physical = Some(physical);
            self.pressed_at = Some(at);
            self.repeated_at = None;
            return Some(match typed_before {
                Some(before) => KeyMessage::Restroke {
                    up: key_up_message(&before),
                    press: message,
                    receipt: PressReceipt::credit(&self.gate),
                },
                None => KeyMessage::Press(message, PressReceipt::credit(&self.gate)),
            });
        }
        let micros = |since: Instant| at.saturating_duration_since(since).as_micros() as u64;
        match self.repeated_at {
            Some(last) => report.repeats.interval_us = Some(micros(last)),
            // a new key's first repeat: the interval it reports is its own, once it has one
            None => {
                report.repeats.delay_us = self.pressed_at.map(micros);
                report.repeats.interval_us = None;
            }
        }
        self.repeated_at = Some(at);
        if self.repeat != KeyRepeat::Os {
            return None;
        }
        match RepeatGate::charge(&self.gate) {
            Some(credit) => {
                report.repeats.delivered += 1;
                Some(KeyMessage::Repeat(message, credit))
            }
            None => {
                report.repeats.dropped += 1;
                None
            }
        }
    }

    /// The release of `physical`, which reads as `translated` now: `[:key-up <what its
    /// press delivered>]`, and for the held key, nothing held any more. A release with no
    /// press seen here (a key pressed in another window) names its own key; a bare
    /// modifier (`translated` none) names nothing.
    pub(super) fn release(
        &mut self,
        physical: PhysicalKey,
        translated: Option<Key>,
    ) -> Option<Message> {
        if self.held_physical == Some(physical) {
            self.let_go();
        }
        let pressed = self
            .down
            .iter()
            .position(|(down, _)| *down == physical)
            .map(|at| self.down.swap_remove(at).1);
        pressed
            .or_else(|| translated.map(Keystroke::Key))
            .map(|stroke| key_up_message(&stroke))
    }

    /// The window lost focus: a release there is never seen, so nothing is held or down.
    pub(super) fn blur(&mut self) {
        self.let_go();
        self.down.clear();
    }

    /// Nothing is held any more, and the repeat still queued for it is taken back.
    fn let_go(&mut self) {
        self.held_physical = None;
        self.gate.withdraw();
        lock(&self.report).held = None;
    }
}

/// The gate that keeps at most ONE OS auto-repeat undelivered per window
/// (`{:key-repeat :os}`, ADR-441). A repeat is delivered with a credit charged here; the
/// credit comes back when the message leaves the mailbox (`process::deliver_credited`),
/// and until it does, further repeats are dropped. So an app whose frame is slower than
/// the repeat rate repeats once per turn instead of queueing a burst that plays on after
/// the key is released — the ADR-086 bug — while a fast app gets every repeat. The
/// release takes back the one still queued (`withdraw`), so the screen stops with the
/// key, not a frame after it.
#[derive(Default)]
pub(super) struct RepeatGate {
    outstanding: AtomicBool,
    withdrawn: AtomicBool,
    /// Whether the last key press the app took from its mailbox was an OS repeat: set as
    /// a repeat's credit comes back, cleared as a fresh press's receipt does
    /// (`PressReceipt`). A message leaves the mailbox when the app receives it, so while
    /// the app folds a press this names that press (ADR-612).
    received_repeat: Arc<AtomicBool>,
}

impl RepeatGate {
    /// A credit for one repeat, or `None` while the last one is still queued.
    fn charge(gate: &Arc<RepeatGate>) -> Option<crate::process::DeliveryCredit> {
        gate.outstanding
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        gate.withdrawn.store(false, Ordering::Release);
        Some(crate::process::DeliveryCredit::new(gate.clone(), 1))
    }

    /// Take back the repeat still queued, if one is: the key was released, or the window
    /// lost focus. Charging the next repeat clears it, so it only ever names the one in
    /// flight (both run on the GUI thread).
    fn withdraw(&self) {
        self.withdrawn.store(true, Ordering::Release);
    }
}

impl crate::process::CreditSource for RepeatGate {
    fn release(&self, _cost: usize) {
        // a repeat taken back at the release never reached the app: it marks nothing
        if !self.withdrawn.load(Ordering::Acquire) {
            self.received_repeat.store(true, Ordering::Release);
        }
        self.outstanding.store(false, Ordering::Release);
    }

    fn withdrawn(&self) -> bool {
        self.withdrawn.load(Ordering::Acquire)
    }
}

/// A fresh press's receipt (ADR-612): a credit that costs nothing and gates nothing, and
/// clears the gate's `received_repeat` as the press leaves the mailbox — so the app,
/// folding that press, reads it as not a repeat.
pub(super) struct PressReceipt {
    received_repeat: Arc<AtomicBool>,
}

impl PressReceipt {
    fn credit(gate: &Arc<RepeatGate>) -> crate::process::DeliveryCredit {
        let receipt = PressReceipt {
            received_repeat: gate.received_repeat.clone(),
        };
        crate::process::DeliveryCredit::new(Arc::new(receipt), 0)
    }
}

impl crate::process::CreditSource for PressReceipt {
    fn release(&self, _cost: usize) {
        self.received_repeat.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::CreditSource;
    use std::time::Duration;
    use winit::keyboard::KeyCode;

    fn physical(code: KeyCode) -> PhysicalKey {
        PhysicalKey::Code(code)
    }

    fn char_key(c: char) -> Keystroke {
        Keystroke::Key(Key::Char(c))
    }

    /// The payload of a `[:key-up …]` / `[:text …]` message, or the bare key string.
    fn spelled(message: &Message) -> String {
        match message {
            Message::Str(s) => s.clone(),
            Message::Keyword(k) => format!(":{}", value::symbol_name(*k)),
            Message::Vector(parts) => parts.iter().map(spelled).collect::<Vec<_>>().join(" "),
            _ => panic!("not a key message"),
        }
    }

    fn sent(answer: Option<KeyMessage>) -> (&'static str, String) {
        match answer {
            Some(KeyMessage::Press(message, _)) => ("press", spelled(&message)),
            Some(KeyMessage::Repeat(message, _)) => ("repeat", spelled(&message)),
            Some(KeyMessage::Restroke { up, press, .. }) => (
                "restroke",
                format!("{} then {}", spelled(&up), spelled(&press)),
            ),
            None => ("nothing", String::new()),
        }
    }

    fn held(keyboard: &Keyboard) -> Option<Keystroke> {
        lock(&keyboard.report).held.clone()
    }

    #[test]
    fn a_press_goes_and_its_repeats_wait_for_the_last_to_be_received() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let start = Instant::now();
        let a = physical(KeyCode::KeyA);
        assert_eq!(
            sent(keyboard.press(char_key('a'), a, start)),
            ("press", "a".into())
        );
        assert!(held(&keyboard) == Some(char_key('a')));

        let first = keyboard.press(char_key('a'), a, start + Duration::from_millis(500));
        let Some(KeyMessage::Repeat(_, credit)) = first else {
            panic!("the first repeat goes, with a credit")
        };
        assert_eq!(
            sent(keyboard.press(char_key('a'), a, start + Duration::from_millis(533))),
            ("nothing", String::new()),
            "a second repeat is dropped while the first is still queued"
        );
        drop(credit); // the app received it
        assert_eq!(
            sent(keyboard.press(char_key('a'), a, start + Duration::from_millis(566))).0,
            "repeat"
        );

        let repeats = lock(&keyboard.report).repeats;
        assert_eq!((repeats.delivered, repeats.dropped), (2, 1));
        assert_eq!(repeats.delay_us, Some(500_000), "press to first repeat");
        assert_eq!(repeats.interval_us, Some(33_000), "between the last two");
    }

    #[test]
    fn each_press_marks_whether_it_was_a_repeat_as_the_app_takes_it() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let repeat = keyboard.received_repeat();
        let start = Instant::now();
        let a = physical(KeyCode::KeyA);
        let Some(KeyMessage::Press(_, receipt)) = keyboard.press(char_key('a'), a, start) else {
            panic!("a fresh press, with its receipt")
        };
        drop(receipt); // the app took the press
        assert!(
            !repeat.load(Ordering::Acquire),
            "a fresh press is not a repeat"
        );
        let Some(KeyMessage::Repeat(_, credit)) = keyboard.press(char_key('a'), a, start) else {
            panic!("a repeat")
        };
        assert!(
            !repeat.load(Ordering::Acquire),
            "not marked until the app takes it: it is still folding the press"
        );
        drop(credit);
        assert!(
            repeat.load(Ordering::Acquire),
            "the app is folding a repeat"
        );

        // released, pressed again: a double tap is two fresh presses
        keyboard.release(a, Some(Key::Char('a')));
        let Some(KeyMessage::Press(_, receipt)) = keyboard.press(char_key('a'), a, start) else {
            panic!("a fresh press after the release")
        };
        drop(receipt);
        assert!(!repeat.load(Ordering::Acquire));

        // a repeat taken back at the release never reached the app, so it marks nothing
        let Some(KeyMessage::Repeat(_, credit)) = keyboard.press(char_key('a'), a, start) else {
            panic!("a repeat")
        };
        keyboard.release(a, Some(Key::Char('a')));
        drop(credit);
        assert!(
            !repeat.load(Ordering::Acquire),
            "a withdrawn repeat is not received"
        );
    }

    #[test]
    fn the_app_repeats_itself_under_key_repeat_app() {
        let mut keyboard = Keyboard::new(KeyRepeat::App);
        let start = Instant::now();
        let a = physical(KeyCode::KeyA);
        assert_eq!(sent(keyboard.press(char_key('a'), a, start)).0, "press");
        assert_eq!(sent(keyboard.press(char_key('a'), a, start)).0, "nothing");
        assert!(
            held(&keyboard) == Some(char_key('a')),
            "gui-held-key still says so"
        );
    }

    #[test]
    fn the_release_takes_back_the_queued_repeat_and_names_its_press() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let start = Instant::now();
        let a = physical(KeyCode::KeyA);
        keyboard.press(char_key('a'), a, start);
        let Some(KeyMessage::Repeat(_, credit)) = keyboard.press(char_key('a'), a, start) else {
            panic!("a repeat")
        };
        let release = keyboard.release(a, Some(Key::Char('a'))).expect("a key-up");
        assert_eq!(spelled(&release), ":key-up a");
        assert!(keyboard.gate.withdrawn(), "the queued repeat is taken back");
        assert!(held(&keyboard).is_none());
        drop(credit);
        // the next hold starts clean: its repeat is wanted
        keyboard.press(char_key('a'), a, start);
        assert!(matches!(
            keyboard.press(char_key('a'), a, start),
            Some(KeyMessage::Repeat(..))
        ));
        assert!(!keyboard.gate.withdrawn());
    }

    #[test]
    fn a_shifted_chord_released_shift_first_names_its_press() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let nine = physical(KeyCode::Digit9);
        keyboard.press(char_key('('), nine, Instant::now());
        // Shift came up first: the release reads as `9`
        let release = keyboard
            .release(nine, Some(Key::Char('9')))
            .expect("a key-up");
        assert_eq!(spelled(&release), ":key-up (");
        assert!(held(&keyboard).is_none(), "and the held key is not stuck");
    }

    #[test]
    fn overlapping_keys_each_release_as_pressed_and_only_the_last_is_held() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let start = Instant::now();
        let (nine, b) = (physical(KeyCode::Digit9), physical(KeyCode::KeyB));
        keyboard.press(char_key('('), nine, start);
        keyboard.press(char_key('b'), b, start);
        assert!(held(&keyboard) == Some(char_key('b')));
        let release = keyboard
            .release(nine, Some(Key::Char('9')))
            .expect("a key-up");
        assert_eq!(
            spelled(&release),
            ":key-up (",
            "not the held key, still its press"
        );
        assert!(
            held(&keyboard) == Some(char_key('b')),
            "b is still held: a release of another key does not end it"
        );
        let release = keyboard.release(b, Some(Key::Char('b'))).expect("a key-up");
        assert_eq!(spelled(&release), ":key-up b");
        assert!(held(&keyboard).is_none());
    }

    #[test]
    fn a_compose_sequence_is_held_and_released_as_its_text() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let e = physical(KeyCode::KeyE);
        let text = Keystroke::Text("ǘ̃".into());
        assert_eq!(
            sent(keyboard.press(text.clone(), e, Instant::now())),
            ("press", ":text ǘ̃".into())
        );
        assert!(held(&keyboard) == Some(text));
        let release = keyboard.release(e, None).expect("a key-up");
        assert_eq!(spelled(&release), ":key-up :text ǘ̃");
    }

    #[test]
    fn focus_loss_lets_everything_go() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let a = physical(KeyCode::KeyA);
        keyboard.press(char_key('('), a, Instant::now());
        keyboard.blur();
        assert!(held(&keyboard).is_none());
        assert!(keyboard.gate.withdrawn());
        // a release seen after the blur names its own key: the press is forgotten
        let release = keyboard.release(a, Some(Key::Char('a'))).expect("a key-up");
        assert_eq!(spelled(&release), ":key-up a");
        // a bare modifier names nothing
        assert!(
            keyboard
                .release(physical(KeyCode::ShiftLeft), None)
                .is_none()
        );
    }

    // Shift let go while `(` repeats: the key repeats as `9` from then on, a keystroke of its
    // own, so `(` comes up first and `9` is pressed, and the release names `9`. The interval
    // the new keystroke reports starts with its own repeats.
    #[test]
    fn a_key_that_comes_to_type_another_keystroke_ends_the_first() {
        let mut keyboard = Keyboard::new(KeyRepeat::Os);
        let start = Instant::now();
        let nine = physical(KeyCode::Digit9);
        keyboard.press(char_key('('), nine, start);
        let Some(KeyMessage::Repeat(_, credit)) =
            keyboard.press(char_key('('), nine, start + Duration::from_millis(500))
        else {
            panic!("the first repeat goes")
        };
        drop(credit);
        keyboard.press(char_key('('), nine, start + Duration::from_millis(533));
        assert_eq!(
            sent(keyboard.press(char_key('9'), nine, start + Duration::from_millis(566))),
            ("restroke", ":key-up ( then 9".into())
        );
        assert!(held(&keyboard) == Some(char_key('9')));
        keyboard.press(char_key('9'), nine, start + Duration::from_millis(1066));
        assert_eq!(
            lock(&keyboard.report).repeats.interval_us,
            None,
            "one repeat of `9` has no interval yet"
        );
        let release = keyboard
            .release(nine, Some(Key::Char('9')))
            .expect("a key-up");
        assert_eq!(spelled(&release), ":key-up 9");
    }
}
