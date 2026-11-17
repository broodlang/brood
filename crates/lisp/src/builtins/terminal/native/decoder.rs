//! The terminal's input bytes as events — Brood's own decoder (KI-360).
//!
//! crossterm reads and parses the tty itself, and it has no parser for xterm's
//! `modifyOtherKeys` (`CSI 27 ; mods ; code ~`), the only way xterm, tmux (`extended-keys`)
//! and the terminals that copy xterm send `C-;`, `C-.`, `C-'`, `C-S-a`, `C-<return>`:
//! asked for and unparsed, every such chord would be dropped. So on Unix the bytes are read
//! here and decoded here, into crossterm's own [`Event`] values — everything downstream (the
//! key tracker, `key_to_value`) is unchanged. The decoding follows crossterm 0.29's parser
//! for everything crossterm reads (the legacy keys, `SS3`, the cursor and function keys with
//! modifiers, the kitty keyboard protocol and `CSI u`, SGR and legacy mouse, focus, bracketed
//! paste), and adds:
//!
//! - `CSI 27 ; mods ; code ~` — xterm's `modifyOtherKeys` (modes 1 and 2);
//! - the legacy bytes 0x1C–0x1F as the keys a terminal sends them for — `C-\`, `C-]`, `C-^`,
//!   `C-_` (and `C-/`, which shares `C-_`'s byte) — where crossterm reads `C-4` … `C-7`;
//! - a terminal's answers to a query read as [`Reply`] values, never typed, whenever they
//!   arrive (ADR-740): device attributes, the kitty keyboard flags, a mode report (DECRPM),
//!   XTVERSION and DECRQSS (DCS), the terminal's colours (OSC 10/11/12), a kitty graphics
//!   answer (APC). `ESC ]`, `ESC P` and `ESC _` are also what Meta sends on `]`, `P` and `_`:
//!   a string sequence is an answer only while its bytes can still be one of
//!   [`REPLY_SHAPES`] (ADR-747). The first byte no answer has there, a terminator before an
//!   answer's head is whole, or its patience run out part-read, and it never was one: its
//!   introducer is the Meta key and every byte after it is read again, as typed.
//!
//! Pure: bytes in, events and replies out, so the decoder is tested on byte strings.

use super::probe::Reply;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};

/// What one complete sequence decodes to.
#[derive(Debug, PartialEq, Eq)]
enum Decoded {
    Event(Event),
    /// A terminal's answer to a query: kept for whoever asked, never an event.
    Reply(Reply),
    /// The Meta key the first `consumed` bytes make — a string sequence's introducer that the
    /// bytes after it showed to be no answer (`ESC ] x` is M-] and an `x`): the event, and
    /// the bytes after them read again.
    KeyThenRest(Event, usize),
    /// A sequence that names nothing: dropped.
    Nothing,
}

/// A step of the decoder over the bytes collected so far: wait for more, a complete
/// sequence, or bytes that are no sequence at all (dropped).
type Step = Result<Option<Decoded>, ()>;

/// The most replies kept unread: a terminal answers the one batch `term-enter` asks, so only
/// answers nobody asks for (a late one, after the batch gave up) pile up — the oldest go.
const REPLIES_KEPT: usize = 32;

/// The decoder: the bytes of a sequence not yet complete, carried between reads, the
/// replies read and not yet taken, and whether a question to the terminal is outstanding.
#[derive(Debug, Default)]
pub(super) struct Decoder {
    pending: Vec<u8>,
    replies: Vec<Reply>,
    awaiting_replies: bool,
}

impl Decoder {
    pub(super) const fn new() -> Self {
        Decoder {
            pending: Vec::new(),
            replies: Vec::new(),
            awaiting_replies: false,
        }
    }

    /// Say whether a question to the terminal is outstanding: while one is, an answer cut
    /// between reads waits the patience of a sequence for its rest; while none is, an answer
    /// comes whole, and a part-read one is a key time decides ([`Decoder::key_undecided`]).
    pub(super) fn await_replies(&mut self, awaiting: bool) {
        self.awaiting_replies = awaiting;
    }

    /// The terminal's answers read since the last take, oldest first.
    pub(super) fn take_replies(&mut self) -> Vec<Reply> {
        std::mem::take(&mut self.replies)
    }

    /// Decode `bytes`, one read of the tty, appending the events to `out` (and the replies
    /// to [`Decoder::take_replies`]). `more_follows` says the tty has more bytes waiting right
    /// now: a lone ESC at the end of the read is then the start of a sequence, else it waits
    /// [`ESCAPE_PATIENCE`] to be the Escape key.
    pub(super) fn feed(&mut self, bytes: &[u8], more_follows: bool, out: &mut Vec<Event>) {
        for (index, byte) in bytes.iter().enumerate() {
            self.pending.push(*byte);
            let input_available = more_follows || index + 1 < bytes.len();
            self.decode_pending(input_available, out);
        }
    }
    /// Decode what is pending after a byte came: complete sequences out, a part-read one
    /// kept. A Meta key whose bytes began like a string sequence has the bytes after it fed
    /// again, each read as typed.
    fn decode_pending(&mut self, input_available: bool, out: &mut Vec<Event>) {
        // ESC ESC: the first is the Escape key, the second starts whatever follows (an
        // Alt chord, a sequence, another Escape) — `\x1b\x1b[A` is Escape then Up
        if self.pending == [0x1B, 0x1B] {
            out.push(Event::Key(KeyCode::Esc.into()));
            self.pending.remove(0);
        }
        if self.in_paste() {
            self.feed_paste(out);
            return;
        }
        match parse(&self.pending, input_available) {
            Ok(None) => {}
            Ok(Some(Decoded::KeyThenRest(event, consumed))) => {
                out.push(event);
                let rest = self.pending.split_off(consumed);
                self.pending.clear();
                self.feed(&rest, input_available, out);
            }
            Ok(Some(decoded)) => {
                self.pending.clear();
                match decoded {
                    Decoded::Event(event) => out.push(event),
                    Decoded::Reply(reply) => {
                        if self.replies.len() >= REPLIES_KEPT {
                            self.replies.remove(0);
                        }
                        self.replies.push(reply);
                    }
                    _ => {}
                }
            }
            Err(()) => self.pending.clear(),
        }
    }

    /// Whether a bracketed paste has begun (`ESC [ 2 0 0 ~`) and not yet ended. A paste is
    /// never timed out — over ssh it arrives in pieces as slowly as the link likes — and
    /// ends only at `ESC [ 2 0 1 ~`.
    pub(super) fn in_paste(&self) -> bool {
        self.pending.starts_with(PASTE_START)
    }

    /// One more byte of a paste: the whole paste once its end has come, else wait — handing
    /// the text over in pieces of [`PASTE_PIECE`] bytes when it is that long, so a runaway
    /// paste (its end lost) costs memory up to a bound, never more.
    fn feed_paste(&mut self, out: &mut Vec<Event>) {
        if self.pending.ends_with(PASTE_END) {
            let body = &self.pending[PASTE_START.len()..self.pending.len() - PASTE_END.len()];
            out.push(Event::Paste(String::from_utf8_lossy(body).to_string()));
            self.pending.clear();
        } else if self.pending.len() >= PASTE_START.len() + PASTE_PIECE + PASTE_END.len() {
            // keep the bytes that may begin the end marker, and a character cut in two
            let keep = PASTE_END.len() - 1;
            let mut cut = self.pending.len() - keep;
            while cut > PASTE_START.len() && (self.pending[cut] & 0b1100_0000) == 0b1000_0000 {
                cut -= 1;
            }
            let body = &self.pending[PASTE_START.len()..cut];
            out.push(Event::Paste(String::from_utf8_lossy(body).to_string()));
            let rest = self.pending[cut..].to_vec();
            self.pending = PASTE_START.to_vec();
            self.pending.extend(rest);
        }
    }

    /// A paste whose end marker never came, given up — the tty silent for
    /// [`PASTE_SILENCE`] ([`paste_stalled`]): the text so far handed over as the paste, and
    /// keys are keys again. Without it a lost end marker swallowed every key, C-g included,
    /// until [`PASTE_PIECE`] bytes had been typed.
    pub(super) fn give_up_paste(&mut self, out: &mut Vec<Event>) {
        if self.in_paste() {
            let body = &self.pending[PASTE_START.len()..];
            out.push(Event::Paste(String::from_utf8_lossy(body).to_string()));
            self.pending.clear();
        }
    }

    /// Whether a sequence is part-read, waiting for the rest of its bytes.
    pub(super) fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Whether what is part-read is a key only time decides, which [`ESCAPE_PATIENCE`] does: a
    /// lone ESC (the Escape key, or Meta on a key still to come), or a string sequence while
    /// no question is outstanding (Meta on `]`, `P` or `_` and keys typed after it, unless the
    /// rest of an answer comes at once). While a question is outstanding, a part-read answer
    /// waits as any other sequence does.
    pub(super) fn key_undecided(&self) -> bool {
        match self.pending[..] {
            [0x1B] => true,
            [0x1B, introducer, ..] if STRING_INTRODUCERS.contains(&introducer) => {
                !self.awaiting_replies
            }
            _ => false,
        }
    }

    /// A part-read sequence that will not be completed — the read ended and nothing more
    /// came, past its patience: a lone ESC is the Escape key; a string sequence never was a
    /// terminal's answer, so its introducer is the Meta key and the bytes after it are read
    /// again, as typed; anything else is dropped. A paste is never given up this way
    /// ([`Decoder::in_paste`]).
    pub(super) fn flush(&mut self, out: &mut Vec<Event>) {
        if self.in_paste() {
            return;
        }
        if self.pending == [0x1B] {
            out.push(Event::Key(KeyCode::Esc.into()));
        }
        if let [0x1B, introducer, ..] = self.pending[..]
            && STRING_INTRODUCERS.contains(&introducer)
        {
            if let Ok(Some(Decoded::Event(event))) = meta(&[introducer], false) {
                out.push(event);
            }
            let rest = self.pending.split_off(2);
            self.pending.clear();
            self.feed(&rest, false, out);
            return self.flush(out);
        }
        self.pending.clear();
    }
}

/// The bytes after ESC that begin a string sequence: OSC (`]`), DCS (`P`), APC (`_`).
const STRING_INTRODUCERS: [u8; 3] = *b"]P_";

/// One shape a terminal's answer comes in as a string sequence: its introducer, the bytes
/// every such answer begins with, and which bytes may follow them up to its terminator.
struct ReplyShape {
    introducer: u8,
    head: &'static [u8],
    body: fn(u8) -> bool,
}

/// The answers a terminal sends as string sequences to what `term-enter` asks (ADR-740), by
/// shape: a string sequence is one of them while its bytes can still be ([`reply_shape`]).
const REPLY_SHAPES: &[ReplyShape] = &[
    // OSC 10, 11, 12: the foreground, background, cursor colour
    ReplyShape {
        introducer: b']',
        head: b"10;rgb:",
        body: colour_byte,
    },
    ReplyShape {
        introducer: b']',
        head: b"11;rgb:",
        body: colour_byte,
    },
    ReplyShape {
        introducer: b']',
        head: b"12;rgb:",
        body: colour_byte,
    },
    // XTVERSION, and DECRQSS valid or not
    ReplyShape {
        introducer: b'P',
        head: b">|",
        body: text_byte,
    },
    ReplyShape {
        introducer: b'P',
        head: b"1$r",
        body: text_byte,
    },
    ReplyShape {
        introducer: b'P',
        head: b"0$r",
        body: text_byte,
    },
    // a kitty graphics answer, which names the image asked about
    ReplyShape {
        introducer: b'_',
        head: b"Gi=",
        body: text_byte,
    },
];

/// The longest answer body waited for: an answer is a few dozen bytes.
const REPLY_MAX: usize = 512;

fn colour_byte(byte: u8) -> bool {
    byte.is_ascii_hexdigit() || byte == b'/'
}

fn text_byte(byte: u8) -> bool {
    byte >= 0x20 && byte != 0x7F
}

/// How far a string sequence's bytes after its introducer go toward an answer.
#[derive(Debug, PartialEq, Eq)]
enum Shape {
    /// They begin an answer, not yet ended.
    Partial,
    /// An answer whole, ended by ST or BEL: its text, without the terminator.
    Complete(usize),
    /// No answer has these bytes: the introducer was the Meta key.
    Impossible,
}

/// The one rule string sequences are read by: `body` (after the introducer) is an answer
/// while it is a prefix of one of [`REPLY_SHAPES`] — the head, then bytes the shape's body
/// allows, within [`REPLY_MAX`] — and complete when ST (`ESC \`) or BEL ends it after a
/// whole head.
fn reply_shape(introducer: u8, body: &[u8]) -> Shape {
    let (text, terminated) = match body {
        [text @ .., 0x07] => (text, true),
        [text @ .., 0x1B, b'\\'] => (text, true),
        [text @ .., 0x1B] => (text, false),
        text => (text, false),
    };
    let fits = |shape: &ReplyShape| {
        shape.introducer == introducer
            && if text.len() < shape.head.len() {
                !terminated && shape.head.starts_with(text)
            } else {
                text.starts_with(shape.head)
                    && text[shape.head.len()..]
                        .iter()
                        .all(|&byte| (shape.body)(byte))
            }
    };
    if text.len() > REPLY_MAX || !REPLY_SHAPES.iter().any(fits) {
        Shape::Impossible
    } else if terminated {
        Shape::Complete(text.len())
    } else {
        Shape::Partial
    }
}

/// Meta (Alt) on the key `rest` decodes to.
fn meta(rest: &[u8], input_available: bool) -> Step {
    parse(rest, input_available).map(|step| {
        step.map(|decoded| match decoded {
            Decoded::Event(Event::Key(mut event)) => {
                event.modifiers |= KeyModifiers::ALT;
                Decoded::Event(Event::Key(event))
            }
            other => other,
        })
    })
}

/// How long the tty may be completely silent in the middle of a paste before the paste is
/// taken as having lost its end marker. A slow link delivers a paste slowly but steadily —
/// bytes keep coming — so silence this long is a marker that will not come, not a slow paste.
pub(super) const PASTE_SILENCE: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether a paste in progress has stalled: no byte at all for [`PASTE_SILENCE`] since the
/// last one came (`silent_for`).
pub(super) fn paste_stalled(in_paste: bool, silent_for: std::time::Duration) -> bool {
    in_paste && silent_for >= PASTE_SILENCE
}

const PASTE_START: &[u8] = b"\x1B[200~";
const PASTE_END: &[u8] = b"\x1B[201~";

/// The most of a paste held before it is handed over in pieces: a paste whose end marker
/// was lost must not grow without bound (bytes, never time — ssh delivers at its own pace).
const PASTE_PIECE: usize = 16 * 1024 * 1024;

/// Decode a whole byte string, as one read with nothing following it — for the tests.
#[cfg(test)]
pub(super) fn decode(bytes: &[u8]) -> Vec<Event> {
    let mut decoder = Decoder::new();
    let mut out = Vec::new();
    decoder.feed(bytes, false, &mut out);
    decoder.flush(&mut out);
    out
}

fn key(code: KeyCode) -> Step {
    Ok(Some(Decoded::Event(Event::Key(code.into()))))
}

fn key_with(code: KeyCode, modifiers: KeyModifiers) -> Step {
    Ok(Some(Decoded::Event(Event::Key(KeyEvent::new(
        code, modifiers,
    )))))
}

fn parse(buffer: &[u8], input_available: bool) -> Step {
    let Some(&first) = buffer.first() else {
        return Ok(None);
    };
    match first {
        0x1B => parse_escape(buffer, input_available),
        b'\r' => key(KeyCode::Enter),
        b'\t' => key(KeyCode::Tab),
        0x7F => key(KeyCode::Backspace),
        0x00 => key_with(KeyCode::Char(' '), KeyModifiers::CONTROL),
        // 0x0A is C-j in raw mode (the terminal no longer turns RET into it)
        c @ 0x01..=0x1A => key_with(
            KeyCode::Char((c - 0x01 + b'a') as char),
            KeyModifiers::CONTROL,
        ),
        c @ 0x1C..=0x1F => key_with(
            KeyCode::Char(b"\\]^_"[(c - 0x1C) as usize] as char),
            KeyModifiers::CONTROL,
        ),
        _ => parse_utf8_char(buffer).map(|maybe| {
            maybe.map(|c| {
                let modifiers = if c.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                Decoded::Event(Event::Key(KeyEvent::new(KeyCode::Char(c), modifiers)))
            })
        }),
    }
}

/// How long a lone ESC waits for a key after it before it is the Escape key: what Neovim's
/// `ttimeoutlen` is for. A terminal speaking the kitty protocol sends Escape as `CSI 27 u`,
/// so it never waits.
pub(super) const ESCAPE_PATIENCE: std::time::Duration = std::time::Duration::from_millis(30);

fn parse_escape(buffer: &[u8], input_available: bool) -> Step {
    // a lone ESC waits for what follows it: the key it makes Meta (`ESC f` is M-f) can arrive
    // in the next read (a split ssh packet, a person typing ESC then f), so it is the Escape
    // key only once [`ESCAPE_PATIENCE`] passes with nothing after it (`TtyInput`)
    if buffer.len() == 1 {
        return Ok(None);
    }
    match buffer[1] {
        b'O' => {
            if buffer.len() == 2 {
                return Ok(None);
            }
            match buffer[2] {
                b'A' => key(KeyCode::Up),
                b'B' => key(KeyCode::Down),
                b'C' => key(KeyCode::Right),
                b'D' => key(KeyCode::Left),
                b'H' => key(KeyCode::Home),
                b'F' => key(KeyCode::End),
                f @ b'P'..=b'S' => key(KeyCode::F(1 + f - b'P')),
                _ => Err(()),
            }
        }
        b'[' => parse_csi(buffer),
        // a string sequence — OSC `ESC ]`, DCS `ESC P`, APC `ESC _` — up to its terminator: a
        // terminal's answer, while its bytes can still be one ([`reply_shape`]). Meta on `]`,
        // `P`, `_` sends the same two bytes, so the bytes after them decide: one that no
        // answer has there makes the introducer the Meta key, and the rest is read again;
        // nothing after it yet, or the start of an answer, waits — an answer split across
        // reads is still an answer, and Meta comes when its patience passes ([`Decoder::flush`])
        introducer if STRING_INTRODUCERS.contains(&introducer) => {
            match reply_shape(introducer, &buffer[2..]) {
                _ if buffer.len() == 2 => Ok(None),
                Shape::Partial => Ok(None),
                Shape::Complete(length) => {
                    Ok(Some(string_reply(introducer, &buffer[2..2 + length])))
                }
                Shape::Impossible => match meta(&buffer[1..2], true)? {
                    Some(Decoded::Event(event)) => Ok(Some(Decoded::KeyThenRest(event, 2))),
                    _ => Err(()),
                },
            }
        }
        0x1B => key(KeyCode::Esc),
        // ESC before a key is Meta (Alt) on that key
        _ => meta(&buffer[1..], input_available),
    }
}

/// The [`Reply`] a complete answer's text (between introducer and terminator) is, or
/// nothing when its values are garbled (a colour channel of nine digits): dropped, not typed.
fn string_reply(introducer: u8, body: &[u8]) -> Decoded {
    let text = String::from_utf8_lossy(body);
    let reply = match introducer {
        b']' => osc_reply(&text),
        b'P' => dcs_reply(&text),
        _ => text.strip_prefix('G').map(|graphics| {
            let (controls, message) = graphics.split_once(';').unwrap_or((graphics, ""));
            Reply::Graphics {
                controls: controls.to_string(),
                message: message.to_string(),
            }
        }),
    };
    reply.map_or(Decoded::Nothing, Decoded::Reply)
}

/// `slot ; rgb:r/g/b` — an OSC colour answer (10, 11, 12), each channel one to four hex
/// digits scaled to a byte.
fn osc_reply(text: &str) -> Option<Reply> {
    let (slot, spec) = text.split_once(';')?;
    let slot: u16 = slot.parse().ok()?;
    let mut channels = spec.strip_prefix("rgb:")?.split('/').map(|channel| {
        if !(1..=4).contains(&channel.len()) {
            return None;
        }
        let value = u32::from_str_radix(channel, 16).ok()?;
        let most = (1u32 << (4 * channel.len())) - 1;
        Some(((value * 255 + most / 2) / most) as u8)
    });
    let rgb = (channels.next()??, channels.next()??, channels.next()??);
    Some(Reply::Colour { slot, rgb })
}

/// `> | text` (XTVERSION) or `Ps $ r text` (DECRQSS) — a DCS answer.
fn dcs_reply(text: &str) -> Option<Reply> {
    if let Some(version) = text.strip_prefix(">|") {
        return Some(Reply::Version(version.to_string()));
    }
    let (validity, setting) = text.split_once("$r")?;
    Some(Reply::Setting {
        valid: validity == "1",
        text: setting.to_string(),
    })
}

fn parse_csi(buffer: &[u8]) -> Step {
    if buffer.len() == 2 {
        return Ok(None);
    }
    // bracketed paste: everything up to the closing ESC [ 2 0 1 ~ is the text
    if buffer.starts_with(b"\x1B[200~") {
        return if buffer.ends_with(b"\x1B[201~") {
            let text = String::from_utf8_lossy(&buffer[6..buffer.len() - 6]).to_string();
            Ok(Some(Decoded::Event(Event::Paste(text))))
        } else {
            Ok(None)
        };
    }
    if b"\x1B[200~".starts_with(buffer) {
        return Ok(None);
    }
    match buffer[2] {
        b'M' => {
            // legacy X10 mouse: three bytes follow
            if buffer.len() < 6 {
                return Ok(None);
            }
            let (kind, modifiers) = mouse_button(buffer[3].checked_sub(32).ok_or(())?)?;
            let column = u16::from(buffer[4].saturating_sub(32)).saturating_sub(1);
            let row = u16::from(buffer[5].saturating_sub(32)).saturating_sub(1);
            return mouse(kind, column, row, modifiers);
        }
        b'<' => {
            let last = buffer[buffer.len() - 1];
            if last != b'm' && last != b'M' {
                return if buffer.len() > 64 { Err(()) } else { Ok(None) };
            }
            return parse_sgr_mouse(buffer);
        }
        b'[' => {
            // the Linux console's F1-F5
            if buffer.len() == 3 {
                return Ok(None);
            }
            return match buffer[3] {
                f @ b'A'..=b'E' => key(KeyCode::F(1 + f - b'A')),
                _ => Err(()),
            };
        }
        _ => {}
    }
    let last = buffer[buffer.len() - 1];
    if !(0x40..=0x7E).contains(&last) || buffer.len() == 2 {
        // parameter and intermediate bytes: keep reading, but not forever
        return if buffer.len() > 64 { Err(()) } else { Ok(None) };
    }
    let body = std::str::from_utf8(&buffer[2..buffer.len() - 1]).map_err(|_| ())?;
    if let Some(private) = body.strip_prefix('?') {
        // `CSI ? flags u` (the kitty flags), `CSI ? … c` (device attributes), `CSI ? mode ;
        // setting $ y` (a mode report): answers, never keys
        let numbers = |text: &str| -> Vec<u16> {
            text.split(';')
                .filter_map(|field| field.parse().ok())
                .collect()
        };
        let reply = match last {
            b'c' => Some(Reply::DeviceAttributes(numbers(private))),
            b'u' => private.parse().ok().map(Reply::KeyboardFlags),
            b'y' => match numbers(private.strip_suffix('$').unwrap_or(""))[..] {
                [mode, setting] => Some(Reply::Mode { mode, setting }),
                _ => None,
            },
            _ => None,
        };
        return Ok(Some(reply.map_or(Decoded::Nothing, Decoded::Reply)));
    }
    match last {
        b'I' if body.is_empty() => Ok(Some(Decoded::Event(Event::FocusGained))),
        b'O' if body.is_empty() => Ok(Some(Decoded::Event(Event::FocusLost))),
        b'Z' if body.is_empty() => Ok(Some(Decoded::Event(Event::Key(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
        ))))),
        b'~' => parse_tilde(body),
        b'u' => parse_csi_u(body),
        b'M' => parse_rxvt_mouse(body),
        // `CSI row ; col R`, a cursor position report — what crossterm reads it as too, so a
        // modified F3 (`CSI 1 ; mods R`) is lost to it, as in crossterm
        b'R' if !body.is_empty() => Ok(Some(Decoded::Nothing)),
        b'A' | b'B' | b'C' | b'D' | b'F' | b'H' | b'P' | b'Q' | b'R' | b'S' => {
            let code = match last {
                b'A' => KeyCode::Up,
                b'B' => KeyCode::Down,
                b'C' => KeyCode::Right,
                b'D' => KeyCode::Left,
                b'F' => KeyCode::End,
                b'H' => KeyCode::Home,
                b'P' => KeyCode::F(1),
                b'Q' => KeyCode::F(2),
                b'R' => KeyCode::F(3),
                _ => KeyCode::F(4),
            };
            // `CSI 1 ; mods[:kind] X`, or a bare `CSI X`
            let mut fields = body.split(';');
            let _ = fields.next();
            let (modifiers, kind, state) = match fields.next() {
                Some(field) => modifiers_and_kind(field)?,
                None => (KeyModifiers::NONE, KeyEventKind::Press, KeyEventState::NONE),
            };
            Ok(Some(Decoded::Event(Event::Key(
                KeyEvent::new_with_kind_and_state(code, modifiers, kind, state),
            ))))
        }
        // a sequence we do not know: dropped whole, never typed
        _ => Ok(Some(Decoded::Nothing)),
    }
}

/// `mods[:kind]` — the modifier field of a key sequence: 1 + the modifier bits, and the
/// kitty protocol's event kind (1 press, 2 repeat, 3 release).
fn modifiers_and_kind(field: &str) -> Result<(KeyModifiers, KeyEventKind, KeyEventState), ()> {
    let mut parts = field.split(':');
    let mask: u16 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let kind = match parts.next().map(str::parse::<u8>) {
        Some(Ok(2)) => KeyEventKind::Repeat,
        Some(Ok(3)) => KeyEventKind::Release,
        _ => KeyEventKind::Press,
    };
    let bits = mask.saturating_sub(1);
    let mut modifiers = KeyModifiers::NONE;
    for (bit, modifier) in [
        (1, KeyModifiers::SHIFT),
        (2, KeyModifiers::ALT),
        (4, KeyModifiers::CONTROL),
        (8, KeyModifiers::SUPER),
        (16, KeyModifiers::HYPER),
        (32, KeyModifiers::META),
    ] {
        if bits & bit != 0 {
            modifiers |= modifier;
        }
    }
    let mut state = KeyEventState::NONE;
    if bits & 64 != 0 {
        state |= KeyEventState::CAPS_LOCK;
    }
    if bits & 128 != 0 {
        state |= KeyEventState::NUM_LOCK;
    }
    Ok((modifiers, kind, state))
}

/// The key a codepoint names in `modifyOtherKeys` and `CSI u`: the control characters as
/// their keys, the rest as the character.
fn codepoint_key(codepoint: u32, modifiers: KeyModifiers) -> Result<KeyCode, ()> {
    Ok(match char::from_u32(codepoint).ok_or(())? {
        '\x1B' => KeyCode::Esc,
        '\r' | '\n' => KeyCode::Enter,
        '\t' if modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        '\t' => KeyCode::Tab,
        '\x7F' | '\x08' => KeyCode::Backspace,
        c => KeyCode::Char(c),
    })
}

/// `CSI n [; mods[:kind]] ~` — the editing and function keys, and xterm's `modifyOtherKeys`:
/// `CSI 27 ; mods ; code ~`, a key with modifiers the legacy encoding has no bytes for.
fn parse_tilde(body: &str) -> Step {
    let mut fields = body.split(';');
    let first: u16 = fields.next().ok_or(())?.parse().map_err(|_| ())?;
    let modifier_field = fields.next();
    if first == 27 {
        let (modifiers, kind, state) = modifiers_and_kind(modifier_field.ok_or(())?)?;
        let codepoint: u32 = fields.next().ok_or(())?.parse().map_err(|_| ())?;
        let code = codepoint_key(codepoint, modifiers)?;
        // a capital names Shift itself, as crossterm reads a capital
        let modifiers = match code {
            KeyCode::Char(c) if c.is_uppercase() => modifiers | KeyModifiers::SHIFT,
            KeyCode::BackTab => modifiers | KeyModifiers::SHIFT,
            _ => modifiers,
        };
        return Ok(Some(Decoded::Event(Event::Key(
            KeyEvent::new_with_kind_and_state(code, modifiers, kind, state),
        ))));
    }
    let code = match first {
        1 | 7 => KeyCode::Home,
        2 => KeyCode::Insert,
        3 => KeyCode::Delete,
        4 | 8 => KeyCode::End,
        5 => KeyCode::PageUp,
        6 => KeyCode::PageDown,
        n @ 11..=15 => KeyCode::F((n - 10) as u8),
        n @ 17..=21 => KeyCode::F((n - 11) as u8),
        n @ 23..=26 => KeyCode::F((n - 12) as u8),
        n @ 28..=29 => KeyCode::F((n - 15) as u8),
        n @ 31..=34 => KeyCode::F((n - 17) as u8),
        _ => return Ok(Some(Decoded::Nothing)),
    };
    let (modifiers, kind, state) = match modifier_field {
        Some(field) => modifiers_and_kind(field)?,
        None => (KeyModifiers::NONE, KeyEventKind::Press, KeyEventState::NONE),
    };
    Ok(Some(Decoded::Event(Event::Key(
        KeyEvent::new_with_kind_and_state(code, modifiers, kind, state),
    ))))
}

/// The kitty protocol's functional keys, in Unicode's private use area.
fn functional_key(codepoint: u32) -> Option<(KeyCode, KeyEventState)> {
    let keypad = |code| Some((code, KeyEventState::KEYPAD));
    let plain = |code| Some((code, KeyEventState::NONE));
    match codepoint {
        57399..=57408 => keypad(KeyCode::Char(char::from_digit(codepoint - 57399, 10)?)),
        57409 => keypad(KeyCode::Char('.')),
        57410 => keypad(KeyCode::Char('/')),
        57411 => keypad(KeyCode::Char('*')),
        57412 => keypad(KeyCode::Char('-')),
        57413 => keypad(KeyCode::Char('+')),
        57414 => keypad(KeyCode::Enter),
        57415 => keypad(KeyCode::Char('=')),
        57416 => keypad(KeyCode::Char(',')),
        57417 => keypad(KeyCode::Left),
        57418 => keypad(KeyCode::Right),
        57419 => keypad(KeyCode::Up),
        57420 => keypad(KeyCode::Down),
        57421 => keypad(KeyCode::PageUp),
        57422 => keypad(KeyCode::PageDown),
        57423 => keypad(KeyCode::Home),
        57424 => keypad(KeyCode::End),
        57425 => keypad(KeyCode::Insert),
        57426 => keypad(KeyCode::Delete),
        57427 => keypad(KeyCode::KeypadBegin),
        57358 => plain(KeyCode::CapsLock),
        57359 => plain(KeyCode::ScrollLock),
        57360 => plain(KeyCode::NumLock),
        57361 => plain(KeyCode::PrintScreen),
        57362 => plain(KeyCode::Pause),
        57363 => plain(KeyCode::Menu),
        57376..=57398 => plain(KeyCode::F((codepoint - 57376 + 13) as u8)),
        57428 => plain(KeyCode::Media(MediaKeyCode::Play)),
        57429 => plain(KeyCode::Media(MediaKeyCode::Pause)),
        57430 => plain(KeyCode::Media(MediaKeyCode::PlayPause)),
        57431 => plain(KeyCode::Media(MediaKeyCode::Reverse)),
        57432 => plain(KeyCode::Media(MediaKeyCode::Stop)),
        57433 => plain(KeyCode::Media(MediaKeyCode::FastForward)),
        57434 => plain(KeyCode::Media(MediaKeyCode::Rewind)),
        57435 => plain(KeyCode::Media(MediaKeyCode::TrackNext)),
        57436 => plain(KeyCode::Media(MediaKeyCode::TrackPrevious)),
        57437 => plain(KeyCode::Media(MediaKeyCode::Record)),
        57438 => plain(KeyCode::Media(MediaKeyCode::LowerVolume)),
        57439 => plain(KeyCode::Media(MediaKeyCode::RaiseVolume)),
        57440 => plain(KeyCode::Media(MediaKeyCode::MuteVolume)),
        57441 => plain(KeyCode::Modifier(ModifierKeyCode::LeftShift)),
        57442 => plain(KeyCode::Modifier(ModifierKeyCode::LeftControl)),
        57443 => plain(KeyCode::Modifier(ModifierKeyCode::LeftAlt)),
        57444 => plain(KeyCode::Modifier(ModifierKeyCode::LeftSuper)),
        57445 => plain(KeyCode::Modifier(ModifierKeyCode::LeftHyper)),
        57446 => plain(KeyCode::Modifier(ModifierKeyCode::LeftMeta)),
        57447 => plain(KeyCode::Modifier(ModifierKeyCode::RightShift)),
        57448 => plain(KeyCode::Modifier(ModifierKeyCode::RightControl)),
        57449 => plain(KeyCode::Modifier(ModifierKeyCode::RightAlt)),
        57450 => plain(KeyCode::Modifier(ModifierKeyCode::RightSuper)),
        57451 => plain(KeyCode::Modifier(ModifierKeyCode::RightHyper)),
        57452 => plain(KeyCode::Modifier(ModifierKeyCode::RightMeta)),
        57453 => plain(KeyCode::Modifier(ModifierKeyCode::IsoLevel3Shift)),
        57454 => plain(KeyCode::Modifier(ModifierKeyCode::IsoLevel5Shift)),
        _ => None,
    }
}

/// `CSI code[:shifted] [; mods[:kind]] [; text] u` — the kitty keyboard protocol, and the
/// `CSI u` form ("fixterms", xterm's `formatOtherKeys: 1`, tmux's `extended-keys-format
/// csi-u`) of the same chords `modifyOtherKeys` sends.
fn parse_csi_u(body: &str) -> Step {
    let mut fields = body.split(';');
    let mut codepoints = fields.next().ok_or(())?.split(':');
    let codepoint: u32 = codepoints.next().ok_or(())?.parse().map_err(|_| ())?;
    let (mut modifiers, kind, modifier_state) = match fields.next() {
        Some(field) if !field.is_empty() => modifiers_and_kind(field)?,
        _ => (KeyModifiers::NONE, KeyEventKind::Press, KeyEventState::NONE),
    };
    let (mut code, code_state) = match functional_key(codepoint) {
        Some(found) => found,
        None => (codepoint_key(codepoint, modifiers)?, KeyEventState::NONE),
    };
    if let KeyCode::Modifier(modifier) = code {
        use ModifierKeyCode as M;
        let held = match modifier {
            M::LeftAlt | M::RightAlt => KeyModifiers::ALT,
            M::LeftControl | M::RightControl => KeyModifiers::CONTROL,
            M::LeftShift | M::RightShift => KeyModifiers::SHIFT,
            M::LeftSuper | M::RightSuper => KeyModifiers::SUPER,
            M::LeftHyper | M::RightHyper => KeyModifiers::HYPER,
            M::LeftMeta | M::RightMeta => KeyModifiers::META,
            _ => KeyModifiers::NONE,
        };
        modifiers |= held;
    }
    // the shifted key the layout gives, when the terminal reports it (kitty's alternates)
    if modifiers.contains(KeyModifiers::SHIFT)
        && let Some(shifted) = codepoints
            .next()
            .and_then(|text| text.parse::<u32>().ok())
            .and_then(char::from_u32)
    {
        code = KeyCode::Char(shifted);
        modifiers.remove(KeyModifiers::SHIFT);
    }
    Ok(Some(Decoded::Event(Event::Key(
        KeyEvent::new_with_kind_and_state(code, modifiers, kind, code_state | modifier_state),
    ))))
}

/// The button byte of a mouse report: what happened, and the modifiers held.
fn mouse_button(cb: u8) -> Result<(MouseEventKind, KeyModifiers), ()> {
    let button = (cb & 0b0000_0011) | ((cb & 0b1100_0000) >> 4);
    let dragging = cb & 0b0010_0000 != 0;
    let kind = match (button, dragging) {
        (0, false) => MouseEventKind::Down(MouseButton::Left),
        (1, false) => MouseEventKind::Down(MouseButton::Middle),
        (2, false) => MouseEventKind::Down(MouseButton::Right),
        (0, true) => MouseEventKind::Drag(MouseButton::Left),
        (1, true) => MouseEventKind::Drag(MouseButton::Middle),
        (2, true) => MouseEventKind::Drag(MouseButton::Right),
        (3, false) => MouseEventKind::Up(MouseButton::Left),
        (3, true) | (4, true) | (5, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return Err(()),
    };
    let mut modifiers = KeyModifiers::NONE;
    if cb & 0b0000_0100 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if cb & 0b0000_1000 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if cb & 0b0001_0000 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    Ok((kind, modifiers))
}

fn mouse(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> Step {
    Ok(Some(Decoded::Event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }))))
}

/// `CSI < cb ; x ; y M|m` — the SGR mouse (1006): `m` is a release.
fn parse_sgr_mouse(buffer: &[u8]) -> Step {
    let body = std::str::from_utf8(&buffer[3..buffer.len() - 1]).map_err(|_| ())?;
    let mut fields = body.split(';');
    let cb: u8 = fields.next().ok_or(())?.parse().map_err(|_| ())?;
    let (kind, modifiers) = mouse_button(cb)?;
    let column = fields
        .next()
        .ok_or(())?
        .parse::<u16>()
        .map_err(|_| ())?
        .saturating_sub(1);
    let row = fields
        .next()
        .ok_or(())?
        .parse::<u16>()
        .map_err(|_| ())?
        .saturating_sub(1);
    let kind = match (buffer[buffer.len() - 1], kind) {
        (b'm', MouseEventKind::Down(button)) => MouseEventKind::Up(button),
        (_, kind) => kind,
    };
    mouse(kind, column, row, modifiers)
}

/// `CSI cb ; x ; y M` — the rxvt mouse (1015).
fn parse_rxvt_mouse(body: &str) -> Step {
    let mut fields = body.split(';');
    let cb: u8 = fields.next().ok_or(())?.parse().map_err(|_| ())?;
    let (kind, modifiers) = mouse_button(cb.checked_sub(32).ok_or(())?)?;
    let column = fields
        .next()
        .ok_or(())?
        .parse::<u16>()
        .map_err(|_| ())?
        .saturating_sub(1);
    let row = fields
        .next()
        .ok_or(())?
        .parse::<u16>()
        .map_err(|_| ())?
        .saturating_sub(1);
    mouse(kind, column, row, modifiers)
}

/// One UTF-8 character from the start of `buffer`: wait while its bytes are still coming,
/// drop bytes that are no UTF-8.
fn parse_utf8_char(buffer: &[u8]) -> Result<Option<char>, ()> {
    if let Ok(text) = std::str::from_utf8(buffer) {
        return text.chars().next().map(Some).ok_or(());
    }
    let required = match buffer[0] {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => return Err(()),
    };
    if buffer[1..]
        .iter()
        .any(|byte| byte & 0b1100_0000 != 0b1000_0000)
    {
        return Err(());
    }
    if buffer.len() < required {
        Ok(None)
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    const CONTROL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
    const NONE: KeyModifiers = KeyModifiers::NONE;

    #[test]
    fn modify_other_keys_sends_the_chords_the_legacy_bytes_cannot() {
        // C-; C-. C-' — xterm's modifyOtherKeys=2, `CSI 27 ; 5 ; code ~`
        assert_eq!(
            decode(b"\x1b[27;5;59~"),
            vec![pressed(KeyCode::Char(';'), CONTROL)]
        );
        assert_eq!(
            decode(b"\x1b[27;5;46~"),
            vec![pressed(KeyCode::Char('.'), CONTROL)]
        );
        assert_eq!(
            decode(b"\x1b[27;5;39~"),
            vec![pressed(KeyCode::Char('\''), CONTROL)]
        );
        // C-M-f and C-S-a
        assert_eq!(
            decode(b"\x1b[27;7;102~"),
            vec![pressed(KeyCode::Char('f'), CONTROL | ALT)]
        );
        assert_eq!(
            decode(b"\x1b[27;6;65~"),
            vec![pressed(KeyCode::Char('A'), CONTROL | SHIFT)]
        );
        // C-<return>, S-<tab>, C-<backspace>
        assert_eq!(
            decode(b"\x1b[27;5;13~"),
            vec![pressed(KeyCode::Enter, CONTROL)]
        );
        assert_eq!(
            decode(b"\x1b[27;2;9~"),
            vec![pressed(KeyCode::BackTab, SHIFT)]
        );
        assert_eq!(
            decode(b"\x1b[27;5;127~"),
            vec![pressed(KeyCode::Backspace, CONTROL)]
        );
        // C-<tab> and C-S-<tab>, which the legacy bytes send as a bare Tab
        assert_eq!(
            decode(b"\x1b[27;5;9~"),
            vec![pressed(KeyCode::Tab, CONTROL)]
        );
        assert_eq!(
            decode(b"\x1b[27;6;9~"),
            vec![pressed(KeyCode::BackTab, CONTROL | SHIFT)]
        );
        // and under the kitty keyboard protocol
        assert_eq!(decode(b"\x1b[9;5u"), vec![pressed(KeyCode::Tab, CONTROL)]);
    }

    #[test]
    fn the_csi_u_form_of_the_same_chords() {
        assert_eq!(
            decode(b"\x1b[59;5u"),
            vec![pressed(KeyCode::Char(';'), CONTROL)]
        );
        assert_eq!(
            decode(b"\x1b[13;5u"),
            vec![pressed(KeyCode::Enter, CONTROL)]
        );
        // kitty: a release, and a shifted key with its alternate
        assert_eq!(
            decode(b"\x1b[110;5:3u"),
            vec![Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('n'),
                CONTROL,
                KeyEventKind::Release
            ))]
        );
        assert_eq!(
            decode(b"\x1b[120:88;4u"),
            vec![pressed(KeyCode::Char('X'), ALT)]
        );
    }

    #[test]
    fn the_legacy_bytes() {
        assert_eq!(
            decode(b"a\r\t\x7f"),
            vec![
                pressed(KeyCode::Char('a'), NONE),
                pressed(KeyCode::Enter, NONE),
                pressed(KeyCode::Tab, NONE),
                pressed(KeyCode::Backspace, NONE),
            ]
        );
        assert_eq!(decode(b"\x01"), vec![pressed(KeyCode::Char('a'), CONTROL)]);
        assert_eq!(decode(b"\x00"), vec![pressed(KeyCode::Char(' '), CONTROL)]);
        // 0x1F is C-_ (C-/ sends it), not crossterm's C-7; 0x1C is C-\
        assert_eq!(decode(b"\x1f"), vec![pressed(KeyCode::Char('_'), CONTROL)]);
        assert_eq!(decode(b"\x1c"), vec![pressed(KeyCode::Char('\\'), CONTROL)]);
        assert_eq!(decode(b"A"), vec![pressed(KeyCode::Char('A'), SHIFT)]);
        assert_eq!(
            decode("é日".as_bytes()),
            vec![
                pressed(KeyCode::Char('é'), NONE),
                pressed(KeyCode::Char('日'), NONE)
            ]
        );
    }

    /// An ESC whose key arrives in the next read is still Meta on it: a lone ESC at a read's
    /// end waits (the tty lets it go as Escape after `ESCAPE_PATIENCE`), it is not Escape at
    /// once and its key a plain one.
    #[test]
    fn an_escape_split_from_its_key_is_still_meta() {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b", false, &mut out);
        assert!(out.is_empty(), "{out:?}");
        assert!(decoder.key_undecided());
        decoder.feed(b"f", false, &mut out);
        assert_eq!(out, vec![pressed(KeyCode::Char('f'), ALT)]);
        assert!(!decoder.waiting());
    }

    #[test]
    fn escape_is_meta_or_the_escape_key() {
        assert_eq!(decode(b"\x1bf"), vec![pressed(KeyCode::Char('f'), ALT)]);
        assert_eq!(
            decode(b"\x1b\x01"),
            vec![pressed(KeyCode::Char('a'), CONTROL | ALT)]
        );
        assert_eq!(decode(b"\x1b"), vec![pressed(KeyCode::Esc, NONE)]);
        // ESC ESC is Escape, and the second ESC goes on to start what follows
        assert_eq!(
            decode(b"\x1b\x1b"),
            vec![pressed(KeyCode::Esc, NONE), pressed(KeyCode::Esc, NONE)]
        );
        assert_eq!(
            decode(b"\x1b\x1b\x1b"),
            vec![pressed(KeyCode::Esc, NONE); 3]
        );
        assert_eq!(
            decode(b"\x1b\x1b[A"),
            vec![pressed(KeyCode::Esc, NONE), pressed(KeyCode::Up, NONE)]
        );
        assert_eq!(
            decode(b"\x1b\x1bf"),
            vec![
                pressed(KeyCode::Esc, NONE),
                pressed(KeyCode::Char('f'), ALT)
            ]
        );
    }

    #[test]
    fn a_paste_is_never_timed_out_and_ends_only_at_its_end_marker() {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        // a stall right after the start marker, before any of the text
        decoder.feed(b"\x1b[200~", false, &mut out);
        decoder.flush(&mut out);
        assert!(decoder.in_paste());
        decoder.feed(b"first line\n", false, &mut out);
        // the link went quiet: whatever gives up on a stalled sequence must not end a paste
        assert!(decoder.in_paste());
        decoder.flush(&mut out);
        assert!(out.is_empty());
        decoder.feed(b"\x1bnot a key", false, &mut out);
        decoder.flush(&mut out);
        assert!(out.is_empty());
        decoder.feed(b" end\x1b[201~q", false, &mut out);
        assert_eq!(
            out,
            vec![
                Event::Paste("first line\n\x1bnot a key end".to_string()),
                pressed(KeyCode::Char('q'), NONE)
            ]
        );
    }

    #[test]
    fn a_paste_whose_end_never_comes_is_given_up_after_silence_not_while_bytes_trickle() {
        use std::time::Duration;
        // bytes arriving, however slowly, never stall a paste…
        assert!(!paste_stalled(true, Duration::from_millis(2900)));
        assert!(!paste_stalled(false, Duration::from_secs(60)));
        // …complete silence does
        assert!(paste_stalled(true, PASTE_SILENCE));
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b[200~half a pas", false, &mut out);
        decoder.give_up_paste(&mut out);
        assert_eq!(out, vec![Event::Paste("half a pas".to_string())]);
        // and keys are keys again: C-g reaches the app
        let mut out = Vec::new();
        decoder.feed(b"\x07", false, &mut out);
        assert_eq!(out, vec![pressed(KeyCode::Char('g'), CONTROL)]);
    }

    #[test]
    fn a_runaway_paste_is_handed_over_in_pieces() {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b[200~", true, &mut out);
        let piece = vec![b'x'; PASTE_PIECE + 10];
        decoder.feed(&piece, true, &mut out);
        assert_eq!(out.len(), 1);
        decoder.feed(b"\x1b[201~", false, &mut out);
        let total: usize = out
            .iter()
            .map(|event| match event {
                Event::Paste(text) => text.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(total, PASTE_PIECE + 10);
    }

    #[test]
    fn cursor_function_and_editing_keys() {
        assert_eq!(
            decode(b"\x1b[A\x1bOB"),
            vec![pressed(KeyCode::Up, NONE), pressed(KeyCode::Down, NONE)]
        );
        assert_eq!(decode(b"\x1b[1;5C"), vec![pressed(KeyCode::Right, CONTROL)]);
        assert_eq!(
            decode(b"\x1b[1;6A"),
            vec![pressed(KeyCode::Up, CONTROL | SHIFT)]
        );
        assert_eq!(decode(b"\x1bOP"), vec![pressed(KeyCode::F(1), NONE)]);
        assert_eq!(decode(b"\x1b[15~"), vec![pressed(KeyCode::F(5), NONE)]);
        assert_eq!(
            decode(b"\x1b[3;5~"),
            vec![pressed(KeyCode::Delete, CONTROL)]
        );
        assert_eq!(decode(b"\x1b[5~"), vec![pressed(KeyCode::PageUp, NONE)]);
        assert_eq!(decode(b"\x1b[Z"), vec![pressed(KeyCode::BackTab, SHIFT)]);
    }

    #[test]
    fn mouse_focus_and_paste() {
        assert_eq!(
            decode(b"\x1b[<0;10;5M\x1b[<0;10;5m"),
            vec![
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 9,
                    row: 4,
                    modifiers: NONE
                }),
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Up(MouseButton::Left),
                    column: 9,
                    row: 4,
                    modifiers: NONE
                }),
            ]
        );
        assert_eq!(
            decode(b"\x1b[I\x1b[O"),
            vec![Event::FocusGained, Event::FocusLost]
        );
        assert_eq!(
            decode(b"\x1b[200~one\ntwo\x1b[201~x"),
            vec![
                Event::Paste("one\ntwo".to_string()),
                pressed(KeyCode::Char('x'), NONE)
            ]
        );
    }

    #[test]
    fn a_terminals_answers_are_never_typed() {
        // cursor position, device attributes, the kitty flags
        assert_eq!(decode(b"\x1b[12;40R\x1b[?64;1;2c\x1b[?1u"), vec![]);
        // and an unknown sequence is dropped whole
        assert_eq!(decode(b"\x1b[99q"), vec![]);
        // a kitty graphics answer (ADR-695), ended by ST or BEL, and a key after it still a key
        assert_eq!(
            decode(b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=31;ENOENT:no such image\x07x"),
            vec![pressed(KeyCode::Char('x'), NONE)]
        );
    }

    /// Decode `bytes` as one read; the events and the replies.
    fn decode_with_replies(bytes: &[u8]) -> (Vec<Event>, Vec<Reply>) {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(bytes, false, &mut out);
        decoder.flush(&mut out);
        (out, decoder.take_replies())
    }

    /// The answers to `term-enter`'s batched question (ADR-740), each read as the reply it
    /// is, ended by ST or BEL, with the keys typed among them still keys.
    #[test]
    fn a_terminals_answers_are_read_as_replies() {
        let (events, replies) = decode_with_replies(
            b"\x1b[?1u\x1b_Gi=31;OK\x1b\\\x1b]10;rgb:cdcd/d6d6/f4f4\x1b\\a\
              \x1b]11;rgb:1e/1e/2e\x07\x1b[?2026;2$y\x1b[?2027;0$y\
              \x1bP>|foot(1.16.2)\x1b\\\x1bP1$r0;38:2::1:2:3m\x1b\\\x1bP0$r\x1b\\\
              \x1b[?62;22c",
        );
        assert_eq!(events, vec![pressed(KeyCode::Char('a'), NONE)]);
        assert_eq!(
            replies,
            vec![
                Reply::KeyboardFlags(1),
                Reply::Graphics {
                    controls: "i=31".to_string(),
                    message: "OK".to_string()
                },
                Reply::Colour {
                    slot: 10,
                    rgb: (0xcd, 0xd6, 0xf4)
                },
                Reply::Colour {
                    slot: 11,
                    rgb: (0x1e, 0x1e, 0x2e)
                },
                Reply::Mode {
                    mode: 2026,
                    setting: 2
                },
                Reply::Mode {
                    mode: 2027,
                    setting: 0
                },
                Reply::Version("foot(1.16.2)".to_string()),
                Reply::Setting {
                    valid: true,
                    text: "0;38:2::1:2:3m".to_string()
                },
                Reply::Setting {
                    valid: false,
                    text: String::new()
                },
                Reply::DeviceAttributes(vec![62, 22]),
            ]
        );
    }

    /// An answer that arrives in pieces — over ssh, after the batch stopped waiting — is put
    /// back together and is still no key.
    #[test]
    fn an_answer_split_across_reads_is_still_an_answer() {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b]", true, &mut out);
        decoder.feed(b"11;rgb:00", true, &mut out);
        decoder.feed(b"00/0000/0000\x1b", true, &mut out);
        decoder.feed(b"\\x", false, &mut out);
        assert_eq!(out, vec![pressed(KeyCode::Char('x'), NONE)]);
        assert_eq!(
            decoder.take_replies(),
            vec![Reply::Colour {
                slot: 11,
                rgb: (0, 0, 0)
            }]
        );
        // a colour the terminal garbled is no reply, and nothing typed
        assert_eq!(
            decode_with_replies(b"\x1b]11;rgb:123456789/0/0\x1b\\"),
            (vec![], vec![])
        );
    }

    /// `ESC ]`, `ESC P` and `ESC _` are Meta on `]`, `P` and `_` unless an answer follows.
    #[test]
    fn meta_on_a_string_introducer_is_still_a_key() {
        assert_eq!(decode(b"\x1b]"), vec![pressed(KeyCode::Char(']'), ALT)]);
        assert_eq!(decode(b"\x1b_"), vec![pressed(KeyCode::Char('_'), ALT)]);
        assert_eq!(
            decode(b"\x1bP"),
            vec![pressed(KeyCode::Char('P'), ALT | SHIFT)]
        );
        // the key after it is read again, as itself
        assert_eq!(
            decode(b"\x1b]x\x1b_\x1b[A"),
            vec![
                pressed(KeyCode::Char(']'), ALT),
                pressed(KeyCode::Char('x'), NONE),
                pressed(KeyCode::Char('_'), ALT),
                pressed(KeyCode::Up, NONE),
            ]
        );
        // and an introducer that ends a read, nothing more coming, waits as a lone ESC does:
        // the Meta key once its patience passes (the flush)
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b]", false, &mut out);
        assert!(out.is_empty() && decoder.key_undecided());
        decoder.flush(&mut out);
        assert_eq!(out, vec![pressed(KeyCode::Char(']'), ALT)]);
    }

    /// The whole batch of answers, cut into two reads at every byte — each read the tty's
    /// last for now, as a late answer over ssh arrives — is read as the same replies and
    /// types nothing. (An answer cut right after `ESC ]` was once M-] and its text typed.)
    #[test]
    fn the_answers_cut_anywhere_are_still_answers() {
        let answers: &[u8] = b"\x1b[?1u\x1b_Gi=31;OK\x1b\\\x1b]10;rgb:cdcd/d6d6/f4f4\x1b\\\
              \x1b]11;rgb:1e/1e/2e\x07\x1b[?2026;2$y\x1bP>|foot(1.16.2)\x1b\\\
              \x1bP1$r0;38:2::1:2:3m\x1b\\\x1b[?62;22c";
        let (_, whole) = decode_with_replies(answers);
        assert_eq!(whole.len(), 8);
        for cut in 1..answers.len() {
            let mut decoder = Decoder::new();
            let mut out = Vec::new();
            decoder.feed(&answers[..cut], false, &mut out);
            decoder.feed(&answers[cut..], false, &mut out);
            assert!(out.is_empty(), "cut at {cut}: {out:?}");
            assert_eq!(decoder.take_replies(), whole, "cut at {cut}");
        }
    }

    #[test]
    fn a_sequence_split_across_reads_is_put_back_together() {
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b[27;5", true, &mut out);
        assert!(out.is_empty());
        assert!(decoder.waiting());
        decoder.feed(b";59~", false, &mut out);
        assert_eq!(out, vec![pressed(KeyCode::Char(';'), CONTROL)]);
        // a lone ESC that nothing follows is the Escape key, once flushed
        let mut out = Vec::new();
        decoder.feed(b"\x1b", true, &mut out);
        assert!(out.is_empty());
        decoder.flush(&mut out);
        assert_eq!(out, vec![pressed(KeyCode::Esc, NONE)]);
        // a multi-byte character split across reads
        let mut out = Vec::new();
        decoder.feed(&"日".as_bytes()[..1], true, &mut out);
        decoder.feed(&"日".as_bytes()[1..], false, &mut out);
        assert_eq!(out, vec![pressed(KeyCode::Char('日'), NONE)]);
    }

    /// The keys `text` types, one event each, no modifier (but Shift on a capital).
    fn typed(text: &str) -> Vec<Event> {
        text.chars()
            .map(|character| {
                let modifiers = if character.is_uppercase() {
                    SHIFT
                } else {
                    NONE
                };
                pressed(KeyCode::Char(character), modifiers)
            })
            .collect()
    }

    /// `prefix` then `rest`: the events in order.
    fn then(prefix: Vec<Event>, rest: Vec<Event>) -> Vec<Event> {
        prefix.into_iter().chain(rest).collect()
    }

    /// Alt-] then a digit, in one read: the start of an OSC answer, until it is not one (ADR-747).
    #[test]
    fn alt_bracket_then_a_digit_is_typed() {
        let meta_bracket = || vec![pressed(KeyCode::Char(']'), ALT)];
        // the read ends after the digit: Meta and the digit, once its patience passes
        assert_eq!(decode(b"\x1b]1"), then(meta_bracket(), typed("1")));
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b]1", false, &mut out);
        assert!(out.is_empty() && decoder.key_undecided());
        decoder.flush(&mut out);
        assert_eq!(out, then(meta_bracket(), typed("1")));
        // while a question is outstanding, it waits as any sequence does, and then is typed
        decoder.await_replies(true);
        let mut out = Vec::new();
        decoder.feed(b"\x1b]1", false, &mut out);
        assert!(out.is_empty() && !decoder.key_undecided() && decoder.waiting());
        decoder.flush(&mut out);
        assert_eq!(out, then(meta_bracket(), typed("1")));
        // the first byte no answer has there: typed at once, nothing lost
        assert_eq!(
            decode(b"\x1b]11;text"),
            then(meta_bracket(), typed("11;text"))
        );
        let mut decoder = Decoder::new();
        let mut out = Vec::new();
        decoder.feed(b"\x1b]11;t", true, &mut out);
        assert_eq!(out, then(meta_bracket(), typed("11;t")));
        assert!(!decoder.waiting());
        // a number no answer has, a terminator before the head is whole, a key's sequence
        assert_eq!(decode(b"\x1b]13"), then(meta_bracket(), typed("13")));
        assert_eq!(
            decode(b"\x1b]11\x07"),
            then(
                meta_bracket(),
                then(typed("11"), vec![pressed(KeyCode::Char('g'), CONTROL)])
            )
        );
        assert_eq!(
            decode(b"\x1b]1\x1b[A"),
            then(
                meta_bracket(),
                then(typed("1"), vec![pressed(KeyCode::Up, NONE)])
            )
        );
        // a part-read answer then Escape: Meta, the digit, Escape
        assert_eq!(
            decode(b"\x1b]1\x1b"),
            then(
                meta_bracket(),
                then(typed("1"), vec![pressed(KeyCode::Esc, NONE)])
            )
        );
        // an answer longer than any is no answer
        let mut long = b"\x1b]11;rgb:".to_vec();
        long.extend([b'0'; REPLY_MAX]);
        let events = decode(&long);
        assert_eq!(events[0], pressed(KeyCode::Char(']'), ALT));
        assert_eq!(events.len(), 1 + long.len() - 2);
    }

    /// A real OSC 11 answer cut at every byte, with and without a question outstanding,
    /// the read before the cut the tty's last for now: still the answer, nothing typed.
    #[test]
    fn an_osc_answer_cut_at_every_byte_is_still_an_answer() {
        for answer in [
            &b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\x"[..],
            &b"\x1b]11;rgb:1e/1e/2e\x07x"[..],
        ] {
            for awaiting in [false, true] {
                for cut in 1..answer.len() {
                    let mut decoder = Decoder::new();
                    decoder.await_replies(awaiting);
                    let mut out = Vec::new();
                    decoder.feed(&answer[..cut], false, &mut out);
                    decoder.feed(&answer[cut..], false, &mut out);
                    assert_eq!(out, typed("x"), "cut at {cut}");
                    assert_eq!(
                        decoder.take_replies(),
                        vec![Reply::Colour {
                            slot: 11,
                            rgb: (0x1e, 0x1e, 0x2e)
                        }],
                        "cut at {cut}"
                    );
                }
            }
        }
    }

    /// The same for DCS and APC: Meta on `P` and `_` with the keys after them typed, and an
    /// answer of each read whole wherever it is cut.
    #[test]
    fn dcs_and_apc_are_answers_only_in_an_answers_shape() {
        let meta_p = || vec![pressed(KeyCode::Char('P'), ALT | SHIFT)];
        let meta_underscore = || vec![pressed(KeyCode::Char('_'), ALT)];
        assert_eq!(decode(b"\x1bP1x"), then(meta_p(), typed("1x")));
        assert_eq!(decode(b"\x1bP>x"), then(meta_p(), typed(">x")));
        assert_eq!(decode(b"\x1bP1$"), then(meta_p(), typed("1$")));
        assert_eq!(decode(b"\x1b_Go"), then(meta_underscore(), typed("Go")));
        assert_eq!(decode(b"\x1b_G"), then(meta_underscore(), typed("G")));
        assert_eq!(decode(b"\x1b_Gi"), then(meta_underscore(), typed("Gi")));
        for (answer, reply) in [
            (
                &b"\x1bP>|XTerm(390)\x1b\\"[..],
                Reply::Version("XTerm(390)".to_string()),
            ),
            (
                &b"\x1bP1$r0;38:2::1:2:3m\x1b\\"[..],
                Reply::Setting {
                    valid: true,
                    text: "0;38:2::1:2:3m".to_string(),
                },
            ),
            (
                &b"\x1b_Gi=31;OK\x07"[..],
                Reply::Graphics {
                    controls: "i=31".to_string(),
                    message: "OK".to_string(),
                },
            ),
        ] {
            for cut in 1..answer.len() {
                let mut decoder = Decoder::new();
                let mut out = Vec::new();
                decoder.feed(&answer[..cut], false, &mut out);
                decoder.feed(&answer[cut..], false, &mut out);
                assert!(out.is_empty(), "cut at {cut}: {out:?}");
                assert_eq!(decoder.take_replies(), vec![reply.clone()], "cut at {cut}");
            }
        }
    }
}
