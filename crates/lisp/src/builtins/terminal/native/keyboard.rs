//! The terminal's keyboard (ADR-441, amended 2026-10-25): a held key as a native app has it,
//! in every terminal.
//!
//! A legacy terminal sends a held key as a run of presses queued in the tty, so behind a slow
//! frame they back up and keep playing after the key came up. A terminal that speaks the
//! kitty protocol (kitty, foot, WezTerm, Ghostty, Alacritty 0.13+) can say which events are
//! repeats and when a key is released. `term-enter` asks for that when the terminal says it
//! can, and this module reads it:
//!
//! - **A repeat that is already stale is dropped.** Everything the tty has buffered is read
//!   ahead into the [`Inbox`]; a repeat whose key's next event in that input is another repeat or
//!   its release is superseded, so it never reaches the app ([`repeat_superseded`]). That is
//!   the terminal's version of the window's gate and withdraw: the backlog is gone the moment
//!   the key is up. A press is never dropped, and neither is typed text.
//! - **A release is `[:key-up press]`**, `press` the event its press delivered, found by the
//!   key, not its modifiers, so Ctrl or Shift let go first still names the chord.
//! - **The held key is known**, for `term-held-key` and the display's `:key-held?`.
//!
//! Every other key is inferred ([`Inbox`]): a legacy terminal's and tmux's, and under the
//! protocol the keys it still sends as legacy bytes (text, Enter, Tab, Backspace). A held key
//! there is a run of the same press arriving faster than a hand can press one key
//! ([`REPEAT_GAP`]), timed by the tty reads around the arrivals, so a backlog read all at once
//! behind a slow frame is still seen for what it was. Once a run is held only its latest
//! repeat is live, and when the repeats stop for [`RELEASE_AFTER`] (or another key is
//! pressed) it comes up: `[:key-up press]`, and the repeat still waiting is dropped. Typed text
//! is held and comes up the same way but is never dropped, because a terminal sends a paste
//! as keys too. A key tapped and let go has no key-up here: nothing says when it came up.
//! A press and its next arrival in one read, closer together than any desktop's repeat delay
//! ([`FIRST_REPEAT_MIN`]), are a burst — a script, `tmux send-keys`, a terminal macro — and
//! every key of it is delivered: a held key's first repeat never comes that soon.
//!
//! The inline REPL (`term-raw-enter`, no `term-enter`) reads keys as a terminal always sent
//! them: every key a press, nothing held.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent,
};

/// What `term-enter` asks a kitty-protocol terminal for. Disambiguation makes a chord a
/// `CSI … u` sequence (the only form that can carry an event type); event types add the
/// repeats and releases; alternate keys give a shifted chord its layout's glyph, so
/// Alt+Shift+`,` is still `:alt-<` as it is in a legacy terminal (without them the terminal
/// sends the unshifted `,` with Shift). Keys that type text are still sent as text, so plain
/// typing reads exactly as it did.
pub(super) const FLAGS: KeyboardEnhancementFlags =
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        .union(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        .union(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS);

/// How many events one poll reads ahead at most: a bound on the work, far above what a held
/// key buffers behind one slow frame.
const READ_AHEAD: usize = 256;

/// How many keys are remembered as down. A terminal that reports no release for some key
/// (kitty sends a text key's release only when every key is an escape code) would otherwise
/// grow the list for as long as the program runs.
const DOWN_LIMIT: usize = 16;

/// The key an event is about, whatever its modifiers: a character in lower case (a chord's
/// release with Shift already up sends the lower-case key) and Shift+Tab as Tab.
pub(super) fn key_identity(code: KeyCode) -> KeyCode {
    match code {
        KeyCode::Char(character) => {
            let mut lower = character.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(only), None) => KeyCode::Char(only),
                _ => code,
            }
        }
        KeyCode::BackTab => KeyCode::Tab,
        other => other,
    }
}

/// Whether a key is one that can be held: a bare modifier, a lock key or a media key is
/// delivered (or not) as before but never tracked.
fn trackable(code: KeyCode) -> bool {
    !matches!(
        code,
        KeyCode::Modifier(_)
            | KeyCode::Media(_)
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::NumLock
            | KeyCode::ScrollLock
            | KeyCode::PrintScreen
            | KeyCode::KeypadBegin
    )
}

/// The coalescing rule: a repeat of `identity` is superseded when that key's next event in
/// the input already buffered behind it (`ahead`) is another repeat or its release. Only a
/// repeat is ever judged; a press later in the input does not supersede one (it is a fresh
/// keystroke, which is never dropped, and the repeat before it is real).
pub(super) fn repeat_superseded<'a>(
    identity: KeyCode,
    ahead: impl IntoIterator<Item = &'a Event>,
) -> bool {
    ahead
        .into_iter()
        .find_map(|event| match event {
            Event::Key(later) if key_identity(later.code) == identity => {
                Some(later.kind != KeyEventKind::Press)
            }
            _ => None,
        })
        .unwrap_or(false)
}

/// What a key event becomes for the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum KeyDelivery {
    /// A press, or a repeat let through: the key this event spells.
    Press(KeyEvent),
    /// A release: `[:key-up …]` of the key this press event spelled.
    Release(KeyEvent),
}

/// The keys down and the one held, folded from the events in order. Pure: no terminal, no
/// heap, so the rules are tested over a sequence of crossterm events.
#[derive(Debug, Default)]
pub(super) struct KeyTracker {
    /// Each key down, by [`key_identity`], with the event its press delivered.
    down: Vec<(KeyCode, KeyEvent)>,
    /// The held key: the last one pressed, until its release.
    held: Option<KeyCode>,
    /// The keys this terminal has reported a release (or a repeat) of. A press makes its key
    /// held only once it is here, so a key whose release never arrives cannot stay held.
    reported: Vec<KeyCode>,
}

impl KeyTracker {
    pub(super) const fn new() -> Self {
        KeyTracker {
            down: Vec::new(),
            held: None,
            reported: Vec::new(),
        }
    }

    /// The press of the key held now, if one is.
    pub(super) fn held(&self) -> Option<KeyEvent> {
        let held = self.held?;
        self.down
            .iter()
            .find(|(identity, _)| *identity == held)
            .map(|(_, press)| *press)
    }

    /// Fold one key event, `ahead` the input already buffered behind it: what to deliver,
    /// if anything.
    pub(super) fn take<'a>(
        &mut self,
        event: KeyEvent,
        ahead: impl IntoIterator<Item = &'a Event>,
    ) -> Option<KeyDelivery> {
        if !trackable(event.code) {
            return match event.kind {
                KeyEventKind::Release => None,
                _ => Some(KeyDelivery::Press(event)),
            };
        }
        let identity = key_identity(event.code);
        match event.kind {
            KeyEventKind::Press => {
                self.remember(identity, event);
                self.held = self.reported.contains(&identity).then_some(identity);
                Some(KeyDelivery::Press(event))
            }
            KeyEventKind::Repeat => {
                self.learn(identity);
                // a key pressed before the terminal was taken over has no press here
                let press = match self.press_of(identity) {
                    Some(press) => press,
                    None => {
                        self.remember(identity, event);
                        event
                    }
                };
                self.held = Some(identity);
                (!repeat_superseded(identity, ahead)).then_some(KeyDelivery::Press(press))
            }
            KeyEventKind::Release => {
                self.learn(identity);
                match self.down_index(identity, event.code) {
                    Some(at) => {
                        let (released, press) = self.down.remove(at);
                        if self.held == Some(released) {
                            self.held = None;
                        }
                        Some(KeyDelivery::Release(press))
                    }
                    // a key pressed before the terminal was taken over: its own key
                    None => Some(KeyDelivery::Release(event)),
                }
            }
        }
    }

    /// Where the press a release of `identity` names is in `down`. Its own key first; failing
    /// that, a release of a character with Shift already up (`1` for a `!`, `,` for a `<`)
    /// names the latest symbol down, the one its Shift typed. A terminal sends the base key
    /// on such a release and nothing ties it to the glyph without the keyboard layout, so a
    /// symbol is the best guess, and a wrong one only ends a hold early.
    fn down_index(&self, identity: KeyCode, code: KeyCode) -> Option<usize> {
        self.down
            .iter()
            .position(|(down, _)| *down == identity)
            .or_else(|| match code {
                KeyCode::Char(_) => self.down.iter().rposition(
                    |(down, _)| matches!(down, KeyCode::Char(symbol) if !symbol.is_alphanumeric()),
                ),
                _ => None,
            })
    }

    fn press_of(&self, identity: KeyCode) -> Option<KeyEvent> {
        self.down
            .iter()
            .find(|(down, _)| *down == identity)
            .map(|(_, press)| *press)
    }

    fn remember(&mut self, identity: KeyCode, press: KeyEvent) {
        self.down.retain(|(down, _)| *down != identity);
        if self.down.len() >= DOWN_LIMIT {
            let forgotten = self.down.remove(0).0;
            if self.held == Some(forgotten) {
                self.held = None;
            }
        }
        self.down.push((identity, press));
    }

    /// Whether the terminal has reported a repeat or a release of this key.
    pub(super) fn reports(&self, identity: KeyCode) -> bool {
        self.reported.contains(&identity)
    }

    pub(super) fn learn(&mut self, identity: KeyCode) {
        if !self.reported.contains(&identity) {
            self.reported.push(identity);
        }
    }
}

/// How far apart, on average, a key's arrivals are at most for the run of them to be a held
/// key repeating rather than a hand pressing it again: 70 ms, about 14 a second, which is
/// beyond a finger tapping one key and below every desktop's default repeat rate (25 to 33 a
/// second). A desktop set to repeat slower than this is read as presses, as before.
pub(super) const REPEAT_GAP: Duration = Duration::from_millis(70);

/// The shortest wait any desktop makes between a key going down and its first repeat (GNOME,
/// KDE and X all bottom out at 100 ms or more). A press and another of the same key that
/// arrived closer together than this are not a key held: they are a burst — a script or
/// a terminal macro typing the key again, `tmux send-keys`, a pasted control sequence —
/// and every one of them is delivered.
pub(super) const FIRST_REPEAT_MIN: Duration = Duration::from_millis(100);

/// How long an inferred hold's repeats may stop before the key counts as released. Twice
/// [`REPEAT_GAP`]: a stall in the terminal or the link longer than this ends the hold early,
/// and the next repeat starts a new one.
pub(super) const RELEASE_AFTER: Duration = Duration::from_millis(140);

/// Whether a key event types text: a character with no Ctrl, Alt or Super, Enter or Tab. A
/// terminal sends a paste as exactly these, at once, so a run of them is never dropped,
/// however fast it came: a held letter may play on, but a pasted `====` arrives whole.
fn types_text(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(_) => !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER),
        KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => true,
        _ => false,
    }
}

/// What one poll yields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Polled {
    Key(KeyDelivery),
    Mouse(MouseEvent),
    /// The terminal gained (`true`) or lost the keyboard — reported once `term-enter` asked
    /// for focus reports (`CSI ?1004h`): the app sees `:focus-in` / `:blur`, as a window's.
    Focus(bool),
    /// An event a poll does not report (a resize, a paste): the poll returns nil at once, so
    /// the app looks again.
    Other,
}

/// One thing read from the tty, waiting in [`Inbox::ahead`].
#[derive(Clone, Debug)]
enum Input {
    Event(Event),
    /// An inferred hold ended here: `[:key-up press]`.
    Released(KeyEvent),
}

#[derive(Clone, Debug)]
struct Entry {
    input: Input,
    /// The inferred run this key event belongs to, and its place in it (0 is the press).
    run: Option<(u64, usize)>,
    /// A repeat known to be stale: superseded by a later one, or the key has come up.
    stale: bool,
}

/// The key a terminal reports no repeat or release of, arriving again and again: a legacy
/// terminal's every key, tmux's, and under the kitty protocol the keys it keeps as legacy
/// bytes (text, Enter, Tab, Backspace). Each arrival is timed by the tty reads around it, so
/// whether the run is a held key repeating is decided from when the events could have come,
/// never from when the app got round to them.
#[derive(Clone, Copy, Debug)]
struct Run {
    id: u64,
    /// The run's first event, as delivered: what its key-up names.
    press: KeyEvent,
    count: usize,
    /// Where the tightest stretch of repeats starts: the read before that arrival (it came
    /// after this) and its place in the run. Never the first arrival, whose gap to the next
    /// is the desktop's delay, and never an arrival with no read before it to bound it.
    paced_from: Option<(Instant, usize)>,
    /// The read that brought the latest event: it arrived by then.
    last_read: Instant,
    held: bool,
    /// Its press and the next arrival came in one read, closer than any desktop's repeat
    /// delay ([`FIRST_REPEAT_MIN`]): a burst of presses, never held.
    burst: bool,
}

/// The input read ahead of the app and what it says about the keys: the tracker for keys
/// the terminal reports, the run for those it does not. Pure over its inputs (the events
/// and the instants of the reads that brought them), so the rules are tested without a tty.
#[derive(Debug)]
pub(super) struct Inbox {
    ahead: VecDeque<Entry>,
    keys: KeyTracker,
    run: Option<Run>,
    runs_started: u64,
    /// When the tty was last read: the next events read arrived after it.
    previous_read: Option<Instant>,
}

impl Inbox {
    pub(super) const fn new() -> Self {
        Inbox {
            ahead: VecDeque::new(),
            keys: KeyTracker::new(),
            run: None,
            runs_started: 0,
            previous_read: None,
        }
    }

    /// Start over at `now`, the terminal just taken over or given back: no key down, no
    /// run, and the next events read arrived after `now`. Input already read stays.
    fn reset(&mut self, now: Option<Instant>) {
        self.keys = KeyTracker::new();
        self.run = None;
        self.previous_read = now;
    }

    /// The app was waiting on the tty and it became readable at `at`: whatever the next read
    /// brings arrived from then on, since the read before the wait emptied it. A tighter bound
    /// than that read, which is what tells a burst typed at once (read the moment it came)
    /// from a backlog that piled up behind a busy frame (read when the frame ended).
    pub(super) fn woke(&mut self, at: Instant) {
        self.previous_read = Some(self.previous_read.map_or(at, |read| read.max(at)));
    }

    /// The events one read of the tty brought, at `now`. `drained` says the read emptied the
    /// tty, so a quiet held key really has stopped repeating (a read cut short by
    /// [`READ_AHEAD`] proves nothing).
    pub(super) fn arrive(&mut self, events: Vec<Event>, now: Instant, drained: bool) {
        let after = self.previous_read;
        for event in events {
            match event {
                Event::Key(key) => self.arrive_key(key, after, now),
                other => self.push(Input::Event(other), None),
            }
        }
        if drained
            && let Some(run) = self.run
            && now.saturating_duration_since(run.last_read) > RELEASE_AFTER
        {
            self.end_run();
        }
        self.previous_read = Some(now);
    }

    fn arrive_key(&mut self, key: KeyEvent, after: Option<Instant>, now: Instant) {
        let identity = key_identity(key.code);
        if !trackable(key.code) {
            return self.push(Input::Event(Event::Key(key)), None);
        }
        if key.kind != KeyEventKind::Press {
            // the terminal reports this key: the tracker has it from here, and a run of it
            // was its legacy press, not a hold to end
            self.keys.learn(identity);
            if self
                .run
                .is_some_and(|run| key_identity(run.press.code) == identity)
            {
                self.run = None;
            }
            return self.push(Input::Event(Event::Key(key)), None);
        }
        let reported = self.keys.reports(identity);
        let same = |run: &Run| run.press.code == key.code && run.press.modifiers == key.modifiers;
        match self.run {
            Some(run) if !reported && same(&run) => self.continue_run(key, after, now),
            _ => {
                // another key pressed: the desktop stops repeating the one before it
                self.end_run();
                if reported {
                    self.push(Input::Event(Event::Key(key)), None);
                } else {
                    self.runs_started += 1;
                    self.run = Some(Run {
                        id: self.runs_started,
                        press: key,
                        count: 1,
                        paced_from: None,
                        last_read: now,
                        held: false,
                        burst: false,
                    });
                    self.push(Input::Event(Event::Key(key)), Some((self.runs_started, 0)));
                }
            }
        }
    }

    /// The run's key again. It is held once some stretch of its repeats (the arrivals after
    /// the first, whose gap is the desktop's delay) came at least as fast as [`REPEAT_GAP`]
    /// on average, bounded from the reads around them; from then on only its latest repeat
    /// is live.
    fn continue_run(&mut self, key: KeyEvent, after: Option<Instant>, now: Instant) {
        let Some(mut run) = self.run else { return };
        // the press and this, its first follower, in one read that began less than a
        // desktop's repeat delay before: they came together, so they are presses
        if run.count == 1
            && run.last_read == now
            && after.is_some_and(|from| now.saturating_duration_since(from) < FIRST_REPEAT_MIN)
        {
            run.burst = true;
        }
        run.count += 1;
        run.last_read = now;
        let place = run.count - 1;
        if run.burst {
            self.run = Some(run);
            return self.push(Input::Event(Event::Key(key)), Some((run.id, place)));
        }
        // the stretch from `start` (which arrived after `from`) to this arrival (by `now`)
        // took at most `now - from`, over `place - start` gaps
        run.held = run.held
            || run.paced_from.is_some_and(|(from, start)| {
                now.saturating_duration_since(from) <= REPEAT_GAP * (place - start) as u32
            });
        // this arrival starts the tightest stretch when its bound less a gap per place is
        // later than the best one's
        if let Some(from) = after
            && run.paced_from.is_none_or(|(best, best_start)| {
                from + REPEAT_GAP * best_start as u32 > best + REPEAT_GAP * place as u32
            })
        {
            run.paced_from = Some((from, place));
        }
        self.run = Some(run);
        if run.held {
            self.mark_stale(run);
        }
        self.push(Input::Event(Event::Key(key)), Some((run.id, place)));
    }

    /// The run is over (another key, a protocol event of its key, or quiet): a held run's
    /// key-up is queued behind its events, and its repeats still waiting are stale.
    fn end_run(&mut self) {
        if let Some(run) = self.run.take()
            && run.held
        {
            self.mark_stale(run);
            self.push(Input::Released(run.press), None);
        }
    }

    /// Every repeat of `run` still waiting is stale (the one arriving next, or the release,
    /// supersedes them) — unless the key types text, which is never dropped.
    fn mark_stale(&mut self, run: Run) {
        if types_text(&run.press) {
            return;
        }
        for entry in &mut self.ahead {
            if matches!(entry.run, Some((id, place)) if id == run.id && place > 0) {
                entry.stale = true;
            }
        }
    }

    fn push(&mut self, input: Input, run: Option<(u64, usize)>) {
        self.ahead.push_back(Entry {
            input,
            run,
            stale: false,
        });
    }

    /// The next thing to hand the app, if anything waiting is still live.
    pub(super) fn deliver(&mut self) -> Option<Polled> {
        while let Some(entry) = self.ahead.pop_front() {
            if entry.stale {
                continue;
            }
            match entry.input {
                Input::Released(press) => {
                    return Some(Polled::Key(KeyDelivery::Release(press)));
                }
                Input::Event(Event::Key(key)) => {
                    let later =
                        self.ahead
                            .iter()
                            .filter(|entry| !entry.stale)
                            .filter_map(|entry| match &entry.input {
                                Input::Event(event) => Some(event),
                                Input::Released(_) => None,
                            });
                    if let Some(delivery) = self.keys.take(key, later) {
                        return Some(Polled::Key(delivery));
                    }
                }
                Input::Event(Event::Mouse(mouse)) => return Some(Polled::Mouse(mouse)),
                Input::Event(Event::FocusGained) => return Some(Polled::Focus(true)),
                Input::Event(Event::FocusLost) => return Some(Polled::Focus(false)),
                Input::Event(_) => return Some(Polled::Other),
            }
        }
        None
    }

    /// When an inferred hold will count as released if no repeat comes first: the poll waits
    /// no longer than this, so the key-up wakes the app on time.
    pub(super) fn release_due(&self) -> Option<Instant> {
        self.run
            .filter(|run| run.held)
            .map(|run| run.last_read + RELEASE_AFTER + Duration::from_millis(1))
    }

    /// The press of the key held now: one the terminal reports, or an inferred hold.
    pub(super) fn held(&self) -> Option<KeyEvent> {
        self.keys
            .held()
            .or_else(|| self.run.filter(|run| run.held).map(|run| run.press))
    }

    fn len(&self) -> usize {
        self.ahead.len()
    }
}

/// The terminal's keyboard state: one terminal per OS process, so one of these.
pub(super) struct TerminalKeyboard {
    /// Whether `term-enter` pushed [`FLAGS`], so the leave pops them, once.
    enhanced: bool,
    /// Whether the full-screen UI has the terminal (`term-enter` to `term-leave`): only then
    /// is a hold inferred and a key-up delivered. The inline REPL reads keys as it always did.
    taken: bool,
    inbox: Inbox,
}

/// The process's terminal keyboard (`process_wide_statics.rs`: one terminal per process).
static TERMINAL_KEYBOARD: Mutex<TerminalKeyboard> = Mutex::new(TerminalKeyboard {
    enhanced: false,
    taken: false,
    inbox: Inbox::new(),
});

fn keyboard() -> MutexGuard<'static, TerminalKeyboard> {
    TERMINAL_KEYBOARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// After the alternate screen is entered (kitty keeps one flag stack per screen): take the
/// keyboard, and push [`FLAGS`] when stdout is a terminal that says it speaks the protocol.
/// Best effort; a terminal that does not answer is left as it was.
pub(super) fn enhance() {
    use std::io::IsTerminal;
    let mut keyboard = keyboard();
    keyboard.taken = true;
    keyboard.inbox.reset(Some(Instant::now()));
    if keyboard.enhanced || !std::io::stdout().is_terminal() {
        return;
    }
    if matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    ) && crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PushKeyboardEnhancementFlags(FLAGS)
    )
    .is_ok()
    {
        keyboard.enhanced = true;
    }
}

/// Before the alternate screen is left, on every path out: pop what [`enhance`] pushed, and
/// forget the keys. Idempotent; errors are swallowed.
pub(super) fn restore() {
    let mut keyboard = keyboard();
    keyboard.taken = false;
    keyboard.inbox.reset(None);
    if std::mem::take(&mut keyboard.enhanced) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
    }
}

/// Read everything the tty has buffered into the inbox, without waiting.
fn absorb(keyboard: &mut TerminalKeyboard) -> std::io::Result<()> {
    use crossterm::event::{poll, read};
    let mut events = Vec::new();
    let mut drained = false;
    while keyboard.inbox.len() + events.len() < READ_AHEAD {
        if !poll(Duration::ZERO)? {
            drained = true;
            break;
        }
        events.push(read()?);
    }
    if keyboard.taken {
        keyboard.inbox.arrive(events, Instant::now(), drained);
    } else {
        // the inline REPL: every key is a press, as a terminal always sent it
        for event in events.into_iter().filter_map(inline_event) {
            keyboard.inbox.push(Input::Event(event), None);
        }
    }
    Ok(())
}

/// What the INLINE seam (the REPL's line editor, `term-raw-enter`) takes of an event read
/// from the tty: a key release is dropped (it reads presses, as a terminal always sent them),
/// and so is a focus report — a full-screen app that died without `term-leave` can leave
/// the terminal sending them, and a line editor has nothing to do with one.
fn inline_event(event: Event) -> Option<Event> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Release => None,
        Event::FocusGained | Event::FocusLost => None,
        event => Some(event),
    }
}

/// The press of the key held now, if one is, from everything the tty has buffered.
pub(super) fn held() -> std::io::Result<Option<KeyEvent>> {
    let mut keyboard = keyboard();
    absorb(&mut keyboard)?;
    Ok(if keyboard.taken {
        keyboard.inbox.held()
    } else {
        None
    })
}

/// Wait up to `timeout` for input and return the next thing to deliver, `None` on timeout.
/// An inferred hold's key-up is due when its repeats stop, so the wait ends then too. The
/// wait happens without the lock held.
pub(super) fn next(timeout: Duration) -> std::io::Result<Option<Polled>> {
    let deadline = Instant::now() + timeout;
    loop {
        let wait = {
            let mut keyboard = keyboard();
            absorb(&mut keyboard)?;
            if let Some(polled) = keyboard.inbox.deliver() {
                return Ok(Some(polled));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let until = match keyboard.inbox.release_due() {
                Some(due) => due.min(deadline),
                None => deadline,
            };
            until.saturating_duration_since(now)
        };
        if crossterm::event::poll(wait)? {
            keyboard().inbox.woke(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Inbox, KeyDelivery, KeyTracker, Polled, RELEASE_AFTER, key_identity, repeat_superseded,
    };
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::time::{Duration, Instant};

    fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind_and_state(code, modifiers, kind, KeyEventState::NONE)
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        key(code, modifiers, KeyEventKind::Press)
    }

    fn repeat(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        key(code, modifiers, KeyEventKind::Repeat)
    }

    fn release(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        key(code, modifiers, KeyEventKind::Release)
    }

    /// Feed `events` through a tracker the way `next` does — each with the rest of the
    /// sequence as its read-ahead — and collect what is delivered.
    fn deliver(tracker: &mut KeyTracker, events: &[KeyEvent]) -> Vec<KeyDelivery> {
        let buffered: Vec<Event> = events.iter().map(|event| Event::Key(*event)).collect();
        events
            .iter()
            .enumerate()
            .filter_map(|(at, event)| tracker.take(*event, &buffered[at + 1..]))
            .collect()
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CONTROL: KeyModifiers = KeyModifiers::CONTROL;

    // A backlog of repeats behind a slow frame collapses: every repeat followed by another
    // repeat of its key is dropped, and the last one before the release is dropped too, so
    // nothing plays after the key came up. The press and the release are both delivered.
    #[test]
    fn a_buffered_run_of_repeats_is_dropped_up_to_the_release() {
        let n = KeyCode::Char('n');
        let events = [
            press(n, CONTROL),
            repeat(n, CONTROL),
            repeat(n, CONTROL),
            repeat(n, CONTROL),
            release(n, CONTROL),
        ];
        let delivered = deliver(&mut KeyTracker::new(), &events);
        assert_eq!(
            delivered,
            vec![
                KeyDelivery::Press(press(n, CONTROL)),
                KeyDelivery::Release(press(n, CONTROL)),
            ]
        );
    }

    // A repeat with nothing of its key behind it is the live one: it is delivered, as a
    // press of the held key.
    #[test]
    fn the_last_repeat_with_the_key_still_down_is_delivered() {
        let events = [
            press(KeyCode::Down, NONE),
            repeat(KeyCode::Down, NONE),
            repeat(KeyCode::Down, NONE),
        ];
        let delivered = deliver(&mut KeyTracker::new(), &events);
        assert_eq!(
            delivered,
            vec![
                KeyDelivery::Press(press(KeyCode::Down, NONE)),
                KeyDelivery::Press(press(KeyCode::Down, NONE)),
            ]
        );
    }

    // Presses are never dropped, however many of them are buffered: typed text and a
    // terminal that sends repeats as presses read exactly as before.
    #[test]
    fn presses_are_never_dropped() {
        let a = KeyCode::Char('a');
        let events = [press(a, NONE), press(a, NONE), press(a, NONE)];
        let delivered = deliver(&mut KeyTracker::new(), &events);
        assert_eq!(delivered.len(), 3);
        assert!(
            delivered
                .iter()
                .all(|delivery| *delivery == KeyDelivery::Press(press(a, NONE)))
        );
    }

    // Another key's events do not supersede a repeat, and a fresh press of the same key
    // after it does not either: only a later repeat or the release of that key does.
    #[test]
    fn only_a_later_repeat_or_release_of_the_same_key_supersedes() {
        let up = Event::Key(release(KeyCode::Up, NONE));
        let down_repeat = Event::Key(repeat(KeyCode::Down, NONE));
        let down_press = Event::Key(press(KeyCode::Down, NONE));
        assert!(!repeat_superseded(KeyCode::Down, [&up]));
        assert!(repeat_superseded(KeyCode::Down, [&up, &down_repeat]));
        assert!(!repeat_superseded(
            KeyCode::Down,
            [&down_press, &down_repeat]
        ));
        assert!(!repeat_superseded(KeyCode::Down, []));
    }

    // A release names its press by the key, not its modifiers: Ctrl let go before `n` is
    // still `[:key-up :ctrl-n]`, and a shifted letter released after Shift is its press.
    #[test]
    fn a_release_names_its_press_though_the_modifier_came_up_first() {
        let mut tracker = KeyTracker::new();
        let n = KeyCode::Char('n');
        let delivered = deliver(&mut tracker, &[press(n, CONTROL), release(n, NONE)]);
        assert_eq!(delivered[1], KeyDelivery::Release(press(n, CONTROL)));

        let capital = press(KeyCode::Char('A'), KeyModifiers::SHIFT);
        let delivered = deliver(&mut tracker, &[capital, release(KeyCode::Char('a'), NONE)]);
        assert_eq!(delivered[1], KeyDelivery::Release(capital));

        let back_tab = press(KeyCode::BackTab, KeyModifiers::SHIFT);
        let delivered = deliver(&mut tracker, &[back_tab, release(KeyCode::Tab, NONE)]);
        assert_eq!(delivered[1], KeyDelivery::Release(back_tab));
    }

    // A symbol's release with Shift already up arrives as its base key (`,` for `<`), which
    // nothing down matches: it names the latest symbol down.
    #[test]
    fn a_symbols_release_after_shift_names_the_symbol() {
        let angle = press(KeyCode::Char('<'), KeyModifiers::ALT);
        let delivered = deliver(
            &mut KeyTracker::new(),
            &[angle, release(KeyCode::Char(','), NONE)],
        );
        assert_eq!(delivered[1], KeyDelivery::Release(angle));
    }

    // A release with no press seen (the key went down before the terminal was taken over)
    // relays its own key; a bare modifier's release relays nothing.
    #[test]
    fn a_release_with_no_press_relays_its_own_key_and_a_modifier_nothing() {
        let mut tracker = KeyTracker::new();
        let up = release(KeyCode::Up, NONE);
        assert_eq!(tracker.take(up, []), Some(KeyDelivery::Release(up)));
        let shift = release(
            KeyCode::Modifier(crossterm::event::ModifierKeyCode::LeftShift),
            KeyModifiers::SHIFT,
        );
        assert_eq!(tracker.take(shift, []), None);
    }

    // The held key: a repeat makes its key held (the terminal reports its events), the
    // release ends it, and from then on that key is held from its press. A key whose release
    // was never reported is not held from a press, so it cannot stay held.
    #[test]
    fn a_key_is_held_from_its_first_repeat_then_from_its_press() {
        let mut tracker = KeyTracker::new();
        let down = KeyCode::Down;
        tracker.take(press(down, NONE), []);
        assert_eq!(tracker.held(), None, "no release of it reported yet");
        tracker.take(repeat(down, NONE), []);
        assert_eq!(tracker.held(), Some(press(down, NONE)));
        tracker.take(release(down, NONE), []);
        assert_eq!(tracker.held(), None);
        tracker.take(press(down, NONE), []);
        assert_eq!(tracker.held(), Some(press(down, NONE)));

        tracker.take(press(KeyCode::Char('a'), NONE), []);
        assert_eq!(tracker.held(), None, "a's release was never reported");
    }

    // Pressing another key moves the hold to it; releasing the earlier one keeps it there.
    #[test]
    fn the_held_key_is_the_last_pressed() {
        let mut tracker = KeyTracker::new();
        let (left, right) = (KeyCode::Left, KeyCode::Right);
        deliver(
            &mut tracker,
            &[
                press(left, NONE),
                release(left, NONE),
                press(right, NONE),
                release(right, NONE),
            ],
        );
        tracker.take(press(left, NONE), []);
        tracker.take(press(right, NONE), []);
        assert_eq!(tracker.held(), Some(press(right, NONE)));
        tracker.take(release(left, NONE), []);
        assert_eq!(tracker.held(), Some(press(right, NONE)));
        tracker.take(release(right, NONE), []);
        assert_eq!(tracker.held(), None);
    }

    #[test]
    fn a_keys_identity_ignores_case_and_shift_tab() {
        assert_eq!(key_identity(KeyCode::Char('A')), KeyCode::Char('a'));
        assert_eq!(key_identity(KeyCode::BackTab), KeyCode::Tab);
        assert_eq!(key_identity(KeyCode::Up), KeyCode::Up);
    }

    /// An inbox fed reads at given milliseconds after `start`, as `absorb` feeds it.
    struct Reads {
        inbox: Inbox,
        start: Instant,
    }

    impl Reads {
        fn new() -> Self {
            let start = Instant::now();
            let mut inbox = Inbox::new();
            inbox.reset(Some(start));
            Reads { inbox, start }
        }

        fn at(&self, milliseconds: u64) -> Instant {
            self.start + Duration::from_millis(milliseconds)
        }

        /// One read at `milliseconds` that brought `keys` and emptied the tty.
        fn read(&mut self, milliseconds: u64, keys: &[KeyEvent]) {
            let events = keys.iter().map(|key| Event::Key(*key)).collect();
            self.inbox.arrive(events, self.at(milliseconds), true);
        }

        /// Everything the app would be handed now.
        fn drain(&mut self) -> Vec<Polled> {
            std::iter::from_fn(|| self.inbox.deliver()).collect()
        }
    }

    fn pressed(event: KeyEvent) -> Polled {
        Polled::Key(KeyDelivery::Press(event))
    }

    // The inline REPL never sees a focus report — one a crashed full-screen app left the
    // terminal sending — nor a key release; a press is kept.
    #[test]
    fn the_inline_seam_drops_focus_reports_and_releases() {
        let a = KeyCode::Char('a');
        assert!(super::inline_event(Event::FocusGained).is_none());
        assert!(super::inline_event(Event::FocusLost).is_none());
        assert!(super::inline_event(Event::Key(release(a, NONE))).is_none());
        assert!(matches!(
            super::inline_event(Event::Key(press(a, NONE))),
            Some(Event::Key(_))
        ));
    }

    // A terminal asked for focus reports (`CSI ?1004h`) sends `CSI I` / `CSI O`, which
    // crossterm reads as FocusGained / FocusLost: each is handed to the app in its place
    // among the keys, not dropped as an event a poll does not report.
    #[test]
    fn focus_reports_are_delivered_in_order_with_the_keys() {
        let mut reads = Reads::new();
        let a = KeyCode::Char('a');
        reads.inbox.arrive(
            vec![
                Event::FocusLost,
                Event::FocusGained,
                Event::Key(press(a, NONE)),
            ],
            reads.at(10),
            true,
        );
        assert_eq!(
            reads.drain(),
            vec![
                Polled::Focus(false),
                Polled::Focus(true),
                pressed(press(a, NONE))
            ]
        );
    }

    fn released(event: KeyEvent) -> Polled {
        Polled::Key(KeyDelivery::Release(event))
    }

    // An app keeping up: each repeat arrives in a read of its own. The desktop's delay
    // before the first repeat is no evidence; two repeats 33 ms apart are, so the key is
    // held from the third repeat, every repeat is delivered as it comes, and once they stop
    // the key-up follows.
    #[test]
    fn a_key_repeating_at_the_desktop_rate_is_held_and_comes_up_when_it_stops() {
        let mut reads = Reads::new();
        let down = press(KeyCode::Down, NONE);
        reads.read(10, &[down]);
        assert_eq!(reads.drain(), vec![pressed(down)]);
        let mut delivered = Vec::new();
        for (read_before, arrival) in [(20, 500), (510, 533), (543, 566), (576, 599)] {
            reads.read(read_before, &[]);
            reads.read(arrival, &[down]);
            delivered.extend(reads.drain());
        }
        assert_eq!(delivered, vec![pressed(down); 4]);
        assert_eq!(reads.inbox.held(), Some(down));
        assert_eq!(
            reads.inbox.release_due(),
            Some(reads.at(599) + RELEASE_AFTER + Duration::from_millis(1))
        );
        reads.read(700, &[]);
        assert_eq!(
            reads.inbox.held(),
            Some(down),
            "a gap shorter than the release wait"
        );
        reads.read(760, &[]);
        assert_eq!(reads.drain(), vec![released(down)]);
        assert_eq!(reads.inbox.held(), None);
    }

    // An app a second behind: the press and twenty repeats come in one read. They came
    // faster than any hand, so only the press and the latest repeat are live, and when the
    // repeats have stopped that one goes too — nothing plays on after the key is up.
    #[test]
    fn a_backlog_behind_a_slow_frame_collapses_and_its_last_repeat_goes_at_the_release() {
        let mut reads = Reads::new();
        let next_line = press(KeyCode::Char('n'), CONTROL);
        reads.read(1000, &[next_line; 21]);
        assert_eq!(reads.inbox.held(), Some(next_line));
        assert_eq!(reads.inbox.deliver(), Some(pressed(next_line)));
        reads.read(1300, &[]);
        assert_eq!(reads.drain(), vec![released(next_line)]);
    }

    // Three taps of one key while the app was busy are a hand, not a repeat: 300 ms bounds
    // them, slower than the repeat gap, so all three are delivered and nothing is held.
    #[test]
    fn taps_buffered_behind_a_slow_frame_are_all_delivered() {
        let mut reads = Reads::new();
        reads.read(5, &[]);
        let next_line = press(KeyCode::Char('n'), CONTROL);
        reads.read(305, &[next_line; 3]);
        assert_eq!(reads.inbox.held(), None);
        reads.read(800, &[]);
        assert_eq!(reads.drain(), vec![pressed(next_line); 3]);
    }

    // Four C-b written at once (a script, `tmux send-keys`, bedit's `drive-mspc`) and read
    // straight after the read before: the press and its followers came closer together than
    // any desktop's repeat delay, so they are presses — all four delivered, nothing held. As
    // a hold the middle two were dropped, and point stopped two characters short.
    #[test]
    fn a_burst_of_one_control_key_is_presses_not_a_hold() {
        let mut reads = Reads::new();
        let back = press(KeyCode::Char('b'), CONTROL);
        reads.read(500, &[]);
        reads.read(503, &[back; 4]);
        assert_eq!(reads.inbox.held(), None);
        reads.read(900, &[]);
        assert_eq!(reads.drain(), vec![pressed(back); 4]);
    }

    // The same burst after the app waited half a second on the tty: the read before the wait
    // bounds nothing useful, but the wake does — the burst arrived when the tty became
    // readable, so it is still four presses.
    #[test]
    fn a_burst_read_on_waking_is_presses_however_long_the_wait() {
        let mut reads = Reads::new();
        let back = press(KeyCode::Char('b'), CONTROL);
        reads.read(0, &[]);
        reads.inbox.woke(reads.at(500));
        reads.read(501, &[back; 4]);
        assert_eq!(reads.inbox.held(), None);
        reads.read(900, &[]);
        assert_eq!(reads.drain(), vec![pressed(back); 4]);
    }

    // A held letter is held (idle work waits, the key-up comes), but none of it is dropped:
    // a paste arrives the same way, and a pasted `aaaa` must arrive whole.
    #[test]
    fn typed_text_is_held_but_never_dropped() {
        let mut reads = Reads::new();
        let letter = press(KeyCode::Char('a'), NONE);
        reads.read(100, &[letter; 10]);
        assert_eq!(reads.inbox.held(), Some(letter));
        reads.read(400, &[]);
        let mut expected = vec![pressed(letter); 10];
        expected.push(released(letter));
        assert_eq!(reads.drain(), expected);
    }

    // Another key pressed ends the hold: the desktop stops repeating the first key, so its
    // key-up comes before the new press, and its waiting repeats are dropped.
    #[test]
    fn another_key_ends_the_hold_before_its_press() {
        let mut reads = Reads::new();
        let down = press(KeyCode::Down, NONE);
        let enter = press(KeyCode::Enter, NONE);
        let mut keys = vec![down; 12];
        keys.push(enter);
        reads.read(200, &keys);
        assert_eq!(
            reads.drain(),
            vec![pressed(down), released(down), pressed(enter)]
        );
    }

    // A read cut short with input still in the tty proves nothing about a quiet key.
    #[test]
    fn a_read_that_left_input_behind_does_not_release() {
        let mut reads = Reads::new();
        let down = press(KeyCode::Down, NONE);
        reads.read(200, &[down; 12]);
        reads.inbox.arrive(Vec::new(), reads.at(1500), false);
        assert_eq!(reads.inbox.held(), Some(down));
    }

    // A key the terminal reports (the kitty protocol's repeat) leaves inference: the run of
    // its legacy press ends without a key-up, and the tracker holds it from there.
    #[test]
    fn a_reported_repeat_hands_the_key_to_the_tracker() {
        let mut reads = Reads::new();
        let down = press(KeyCode::Down, NONE);
        reads.read(10, &[down]);
        reads.read(500, &[repeat(KeyCode::Down, NONE)]);
        assert_eq!(reads.drain(), vec![pressed(down), pressed(down)]);
        assert_eq!(reads.inbox.held(), Some(down));
        assert_eq!(reads.inbox.release_due(), None);
        reads.read(900, &[release(KeyCode::Down, NONE)]);
        assert_eq!(reads.drain(), vec![released(down)]);
        assert_eq!(reads.inbox.held(), None);
    }
}
