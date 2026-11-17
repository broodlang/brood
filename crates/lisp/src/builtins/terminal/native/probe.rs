//! What the terminal can do, asked once at `term-enter` in ONE write (ADR-740).
//!
//! Each capability used to be its own question: the kitty keyboard flags through crossterm's
//! reader (waiting up to 2 s for an answer), then the kitty graphics query with DA1 behind it
//! (500 ms) — so a terminal that answered neither held startup for 2.5 s. Now every question
//! goes out in one write that ends with DA1 (`CSI c`), which every terminal answers, and
//! answers in order: when its reply comes, every other answer that is coming has come. A
//! terminal that answers nothing at all (a bare pty, a serial line) costs [`PATIENCE`] once.
//!
//! The questions: the kitty keyboard flags (`CSI ? u`), a kitty graphics query for a 1×1
//! picture (ADR-695), the terminal's foreground and background (OSC 10/11), whether it
//! paints synchronized updates (DECRQM 2026), measures grapheme clusters (DECRQM 2027) and
//! has left and right margins (DECRQM 69, DECLRMM), its name (XTVERSION), and whether it
//! keeps a true colour — a 24-bit SGR set, then asked back (DECRQSS `m`): a terminal that
//! answers with the same colour stores it as such. ssh passes none of `COLORTERM`, so the
//! answer, not the environment, is what says true colour there.
//!
//! The decoder reads the replies as [`Reply`] values (Unix); keys typed while the answers
//! come in reach the inbox as typed. Pure: the replies in, [`Answers`] out.

/// A terminal's answer to a query, as `term-enter`'s batched question reads it (ADR-740).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Reply {
    /// `CSI ? Ps ; … c` — primary device attributes (DA1), which every terminal answers, so
    /// it marks the end of the answers to the questions asked before it.
    DeviceAttributes(Vec<u16>),
    /// `CSI ? flags u` — the kitty keyboard protocol's flags: the terminal speaks it.
    KeyboardFlags(u16),
    /// `CSI ? mode ; setting $ y` — DECRPM, the answer to DECRQM: 0 not recognised, 1 set,
    /// 2 reset, 3 permanently set, 4 permanently reset.
    Mode { mode: u16, setting: u16 },
    /// `DCS > | text ST` — XTVERSION, the terminal's name and version.
    Version(String),
    /// `DCS Ps $ r text ST` — DECRQSS: the setting asked for (`valid`), or an invalid request.
    Setting { valid: bool, text: String },
    /// `OSC slot ; rgb:r/g/b ST` — a colour the terminal uses: 10 its foreground, 11 its
    /// background, 12 its cursor's.
    Colour { slot: u16, rgb: (u8, u8, u8) },
    /// `APC G controls ; message ST` — a kitty graphics protocol answer (ADR-695).
    Graphics { controls: String, message: String },
}

/// The batched question, DA1 last. The true-colour probe sets an SGR colour and resets it
/// at once; nothing is drawn between.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) const QUERY: &[u8] = concat!(
    // the kitty keyboard protocol's flags
    "\x1b[?u",
    // may image 31, one RGB pixel sent directly, be stored (`a=q` stores nothing, it checks)
    "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\",
    // the terminal's own foreground and background
    "\x1b]10;?\x1b\\",
    "\x1b]11;?\x1b\\",
    // synchronized output, grapheme clusters, left and right margins
    "\x1b[?2026$p",
    "\x1b[?2027$p",
    "\x1b[?69$p",
    // the terminal's name and version
    "\x1b[>q",
    // a true colour, asked back
    "\x1b[38;2;1;2;3m\x1bP$qm\x1b\\\x1b[0m",
    // primary device attributes: the end of the answers
    "\x1b[c",
)
.as_bytes();

/// How long the answers are waited for when DA1's never comes: a local terminal answers in a
/// millisecond, an ssh link in a round trip. Past it the environment decides, and a late
/// answer is read and dropped by the decoder.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) const PATIENCE: std::time::Duration = std::time::Duration::from_secs(1);

/// What the terminal said of itself. Everything unanswered is `None` / false: the
/// environment's reading (`COLORTERM`, `TERM`) stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Answers {
    /// Whether DA1's reply came: the terminal answered at all.
    pub(super) answered: bool,
    /// The kitty keyboard protocol's flags: the terminal speaks it.
    pub(super) keyboard_flags: Option<u16>,
    /// Whether the kitty graphics query for image 31 came back `OK`.
    pub(super) graphics: bool,
    /// The terminal's own colours (OSC 10, 11).
    pub(super) foreground: Option<(u8, u8, u8)>,
    pub(super) background: Option<(u8, u8, u8)>,
    /// Whether it knows synchronized output (mode 2026).
    pub(super) synchronized_output: Option<bool>,
    /// How it answered for grapheme clusters (mode 2027) and left and right margins (mode 69,
    /// DECLRMM): the DECRPM setting, which says whether the mode is there to be set
    /// ([`ModeSetting::available`]) and whether it is set already.
    pub(super) grapheme_clusters: Option<ModeSetting>,
    pub(super) left_right_margins: Option<ModeSetting>,
    /// Its name and version (XTVERSION), e.g. `kitty(0.36.4)`, `XTerm(390)`, `tmux 3.4`.
    pub(super) version: Option<String>,
    /// Whether it gave the true colour of the probe back as set.
    pub(super) direct_colour: bool,
}

/// Whether `replies` hold the end of the answers: DA1's.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn complete(replies: &[Reply]) -> bool {
    replies
        .iter()
        .any(|reply| matches!(reply, Reply::DeviceAttributes(_)))
}

/// Whether a DECRPM setting says the mode is known: set or reset, for now or for good —
/// except permanently reset (4), and not recognised (0).
fn mode_known(setting: u16) -> bool {
    matches!(setting, 1..=3)
}

/// A DECRPM answer for a mode the screen turns on when the terminal has it: 0 not
/// recognised, 1 set, 2 reset, 3 permanently set, 4 permanently reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModeSetting(pub(super) u16);

impl ModeSetting {
    /// Whether the mode can be on: set, or reset and settable ([`mode_known`]).
    pub(super) fn available(self) -> bool {
        mode_known(self.0)
    }

    /// Whether the mode is on already, by the terminal's own setting (1, 3): an app that sets
    /// it neither needs to nor may reset it after.
    pub(super) fn set(self) -> bool {
        matches!(self.0, 1 | 3)
    }

    /// Whether the app has to set the mode itself to have it, and reset it to give the
    /// terminal back as it was: the terminal has it, reset (2).
    pub(super) fn to_toggle(self) -> bool {
        self.0 == 2
    }
}

impl Answers {
    /// The answers `replies` give.
    pub(super) fn of(replies: &[Reply]) -> Answers {
        let mut answers = Answers::default();
        for reply in replies {
            answers.learn(reply);
        }
        answers
    }

    fn learn(&mut self, reply: &Reply) {
        match reply {
            Reply::DeviceAttributes(_) => self.answered = true,
            Reply::KeyboardFlags(flags) => self.keyboard_flags = Some(*flags),
            Reply::Graphics { controls, message } => {
                if message == "OK" && controls.split(',').any(|key| key == "i=31") {
                    self.graphics = true;
                }
            }
            Reply::Colour { slot: 10, rgb } => self.foreground = Some(*rgb),
            Reply::Colour { slot: 11, rgb } => self.background = Some(*rgb),
            Reply::Colour { .. } => {}
            Reply::Mode {
                mode: 2026,
                setting,
            } => self.synchronized_output = Some(mode_known(*setting)),
            Reply::Mode {
                mode: 2027,
                setting,
            } => self.grapheme_clusters = Some(ModeSetting(*setting)),
            Reply::Mode { mode: 69, setting } => {
                self.left_right_margins = Some(ModeSetting(*setting))
            }
            Reply::Mode { .. } => {}
            Reply::Version(version) => self.version = Some(version.clone()),
            Reply::Setting { valid, text } => {
                // `38:2::1:2:3` or `38;2;1;2;3`: the probe's colour, kept as a true colour
                let kept = text
                    .trim_end_matches('m')
                    .split([';', ':'])
                    .filter(|field| !field.is_empty())
                    .collect::<Vec<_>>()
                    .windows(5)
                    .any(|window| window == ["38", "2", "1", "2", "3"]);
                if *valid && kept {
                    self.direct_colour = true;
                }
            }
        }
    }

    /// Whether the terminal shows true colour, by its answers: it kept the probe's colour,
    /// or its XTVERSION name is one that does (`screen::names_true_colour_terminal`, the list
    /// `$TERM` is read by too).
    pub(super) fn true_colour(&self) -> bool {
        self.direct_colour
            || self
                .version
                .as_deref()
                .is_some_and(super::screen::names_true_colour_terminal)
    }

    /// Whether frames go out as synchronized updates (`CSI ? 2026 h` … `l`): unless the
    /// terminal said it does not know the mode. One that did not answer gets them — a
    /// terminal that does not know them ignores them.
    pub(super) fn synchronized(&self) -> bool {
        self.synchronized_output != Some(false)
    }

    /// Whether the terminal measures a grapheme cluster as one glyph of its Unicode width
    /// (mode 2027, on or turned on by `term-enter`): unless it said it has the mode, it is
    /// taken to advance by each code point's width, as wcwidth does.
    pub(super) fn grapheme_clusters(&self) -> bool {
        self.grapheme_clusters.is_some_and(ModeSetting::available)
    }

    /// How a scroll may be confined to a band of columns (DECSLRM): only where the terminal
    /// said it has left and right margins (DECLRMM, mode 69) — set already, or to be set
    /// around each such scroll.
    pub(super) fn column_margins(&self) -> super::screen::ColumnMargins {
        use super::screen::ColumnMargins;
        match self.left_right_margins {
            Some(setting) if setting.set() => ColumnMargins::Held,
            Some(setting) if setting.available() => ColumnMargins::Toggled,
            _ => ColumnMargins::Unavailable,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::decoder::Decoder;
    use super::*;

    fn answers_to(bytes: &[u8]) -> (Answers, bool) {
        let mut decoder = Decoder::new();
        let mut events = Vec::new();
        decoder.feed(bytes, false, &mut events);
        assert!(events.is_empty(), "{events:?}");
        let replies = decoder.take_replies();
        (Answers::of(&replies), complete(&replies))
    }

    /// kitty answers everything: its keyboard, pictures, colours, both modes, its name.
    #[test]
    fn kitty_answers_every_question() {
        let (answers, done) = answers_to(
            b"\x1b[?0u\x1b_Gi=31;OK\x1b\\\x1b]10;rgb:dddd/dddd/dddd\x1b\\\
              \x1b]11;rgb:0000/0000/0000\x1b\\\x1b[?2026;2$y\x1b[?2027;0$y\
              \x1bP>|kitty(0.36.4)\x1b\\\x1bP0$r\x1b\\\x1b[?62;c",
        );
        assert!(done);
        assert_eq!(
            answers,
            Answers {
                answered: true,
                keyboard_flags: Some(0),
                graphics: true,
                foreground: Some((0xdd, 0xdd, 0xdd)),
                background: Some((0, 0, 0)),
                synchronized_output: Some(true),
                grapheme_clusters: Some(ModeSetting(0)),
                left_right_margins: None,
                version: Some("kitty(0.36.4)".to_string()),
                direct_colour: false,
            }
        );
        assert!(answers.true_colour() && answers.synchronized());
    }

    /// xterm: no kitty keyboard or pictures, but the true colour it keeps, asked back.
    #[test]
    fn xterm_keeps_the_true_colour_it_is_asked_back() {
        let (answers, done) = answers_to(
            b"\x1b]10;rgb:0000/0000/0000\x1b\\\x1b]11;rgb:ffff/ffff/ffff\x1b\\\
              \x1b[?2026;0$y\x1b[?2027;0$y\x1b[?69;2$y\x1bP>|XTerm(390)\x1b\\\
              \x1bP1$r0;38:2::1:2:3m\x1b\\\x1b[?64;1;2;6;9;15;16;17;18;21;22;28c",
        );
        assert!(done);
        assert_eq!(answers.keyboard_flags, None);
        assert!(!answers.graphics);
        assert!(answers.true_colour());
        assert!(!answers.synchronized());
        assert_eq!(answers.foreground, Some((0, 0, 0)));
        assert_eq!(answers.background, Some((0xff, 0xff, 0xff)));
        // xterm has left and right margins, reset: set around each column scroll
        assert_eq!(
            answers.column_margins(),
            super::super::screen::ColumnMargins::Toggled
        );
        // and measures code points, not clusters
        assert!(!answers.grapheme_clusters());
        // the same probe given back in 256 colours is no true colour
        let (degraded, _) = answers_to(b"\x1bP1$r0;38;5;16m\x1b\\\x1b[?1;2c");
        assert!(!degraded.true_colour());
    }

    /// A mode's DECRPM setting: whether the terminal has it, has it on, or wants it set and
    /// reset by the app — for grapheme clusters (2027) and left and right margins (69).
    #[test]
    fn a_mode_is_available_set_or_to_toggle() {
        use super::super::screen::ColumnMargins;
        let (foot, _) = answers_to(b"\x1b[?2027;2$y\x1b[?69;1$y\x1b[?1;2c");
        assert!(foot.grapheme_clusters());
        assert!(foot.grapheme_clusters.is_some_and(ModeSetting::to_toggle));
        assert_eq!(foot.column_margins(), ColumnMargins::Held);
        let (contour, _) = answers_to(b"\x1b[?2027;3$y\x1b[?69;4$y\x1b[?1;2c");
        assert!(contour.grapheme_clusters());
        assert!(
            !contour
                .grapheme_clusters
                .is_some_and(ModeSetting::to_toggle)
        );
        assert_eq!(contour.column_margins(), ColumnMargins::Unavailable);
        let (silent, _) = answers_to(b"\x1b[?1;2c");
        assert!(!silent.grapheme_clusters());
        assert_eq!(silent.column_margins(), ColumnMargins::Unavailable);
    }

    /// A terminal that answers only DA1 (screen, an old one): done, and nothing learned.
    #[test]
    fn da1_alone_ends_the_answers() {
        let (answers, done) = answers_to(b"\x1b[?1;2c");
        assert!(done && answers.answered);
        assert!(!answers.true_colour() && answers.synchronized());
        let (_, partial) = answers_to(b"\x1b[?1u\x1b_Gi=31;OK\x1b\\");
        assert!(!partial);
    }

    /// The question asks each thing once, and DA1 last.
    #[test]
    fn the_question_ends_with_da1() {
        assert!(QUERY.ends_with(b"\x1b[c"));
        assert_eq!(
            QUERY
                .windows(3)
                .filter(|window| window == b"\x1b[c")
                .count(),
            1
        );
    }
}
