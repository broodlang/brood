//! Pictures in the terminal at PIXEL resolution (ADR-695): an `[:image …]` op drawn through
//! the kitty graphics protocol when the terminal speaks it, in half-block cells otherwise.
//!
//! - **Detected once, by asking.** `term-enter` writes a kitty graphics query for a 1×1
//!   picture and a primary device attributes request (DA1) behind it. Every terminal answers
//!   DA1, and answers in order, so the DA1 reply marks the end of the answer: a kitty `OK`
//!   before it means pictures. `BROOD_TERM_GRAPHICS` (`kitty` / `cells`) decides without
//!   asking — tmux and screen answer DA1 themselves and pass no graphics through, so they
//!   draw cells.
//! - **Transmitted once per content, placed per frame.** A picture is resampled to the
//!   pixels its cells cover (the cell size from the tty's `TIOCGWINSZ`), cropped to what
//!   shows, PNG-encoded and sent under an image id; the frame then PLACES it at its cell. A
//!   frame showing the same picture at the same place sends nothing; one that moves it
//!   deletes the old placement and places the stored image again; a picture no frame shows
//!   stays stored (a page flipped back to) until the least recently used are freed past a
//!   bound. A frame repainted whole (a resize, a continue) deletes every placement and
//!   places again, never retransmitting; leaving the screen frees every image.
//!
//! Pure but for [`detect`] and [`cell_pixels`]: the plan, the escape sequences and the
//! answer parse are tested on values.

use std::sync::Arc;

use crate::host::picture::{self, Anchor, Fit, Picture};

/// How the terminal shows a picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Protocol {
    /// Half-block cells, two pixels a cell (ADR-686): every terminal.
    Cells,
    /// The kitty graphics protocol: kitty, Ghostty, WezTerm, Konsole.
    Kitty,
}

/// The environment variable that decides without asking: `kitty` or `cells`.
pub(super) const OVERRIDE: &str = "BROOD_TERM_GRAPHICS";

/// What [`OVERRIDE`] says: a protocol to use without asking the terminal, or `None` (unset,
/// `auto`, anything else) to ask.
pub(super) fn forced(value: Option<&str>) -> Option<Protocol> {
    match value.map(str::trim) {
        Some("kitty") => Some(Protocol::Kitty),
        Some("cells" | "half-blocks" | "none" | "off" | "0") => Some(Protocol::Cells),
        _ => None,
    }
}

/// The question: may image 31, one RGB pixel sent directly, be stored (`a=q` stores nothing,
/// it only checks) — then DA1, which every terminal answers, after it.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) const QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// How long the answer is waited for: a local terminal answers in a millisecond, an ssh link
/// in a round trip. One that has not answered DA1 by then draws cells; its late answer is
/// swallowed by the decoder.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) const PATIENCE: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether `bytes` hold the end of the answer to [`QUERY`]: a DA1 reply, `CSI ? … c`.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn answered(bytes: &[u8]) -> bool {
    let mut rest = bytes;
    while let Some(at) = find(rest, b"\x1b[?") {
        let after = &rest[at + 3..];
        let end = after
            .iter()
            .position(|byte| !(byte.is_ascii_digit() || *byte == b';'));
        if let Some(end) = end
            && after[end] == b'c'
        {
            return true;
        }
        rest = after;
    }
    false
}

/// The protocol an answer to [`QUERY`] says the terminal speaks: kitty when it answered the
/// graphics query for image 31 with `OK`, cells otherwise.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn protocol_in(bytes: &[u8]) -> Protocol {
    let mut rest = bytes;
    while let Some(at) = find(rest, b"\x1b_G") {
        let after = &rest[at + 3..];
        let Some(end) = find(after, b"\x1b\\") else {
            break;
        };
        let body = &after[..end];
        let (control, message) = match body.iter().position(|byte| *byte == b';') {
            Some(semicolon) => (&body[..semicolon], &body[semicolon + 1..]),
            None => (body, &b""[..]),
        };
        if message == b"OK"
            && control
                .split(|byte| *byte == b',')
                .any(|key| key == b"i=31")
        {
            return Protocol::Kitty;
        }
        rest = &after[end..];
    }
    Protocol::Cells
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The protocol this terminal draws pictures with: [`OVERRIDE`] when it decides, else the
/// terminal's answer to [`QUERY`]. Cells when stdout is not a terminal, or on a platform
/// whose console Brood does not read itself.
pub(super) fn detect() -> Protocol {
    use std::io::IsTerminal;
    if let Some(protocol) = forced(std::env::var(OVERRIDE).ok().as_deref()) {
        return protocol;
    }
    if !std::io::stdout().is_terminal() || std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return Protocol::Cells;
    }
    #[cfg(unix)]
    {
        protocol_in(&super::keyboard::ask_terminal(QUERY, answered, PATIENCE))
    }
    #[cfg(not(unix))]
    {
        Protocol::Cells
    }
}

/// A cell's size in the terminal's pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct CellPixels {
    pub(super) width: u32,
    pub(super) height: u32,
}

/// The terminal's cell size, from the pixel size the tty reports (`TIOCGWINSZ`, which kitty,
/// foot, WezTerm and Ghostty fill in and ssh forwards); `None` when it reports none — a
/// picture is then drawn in cells, as no size could be kept.
pub(super) fn cell_pixels() -> Option<CellPixels> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 {
        return None;
    }
    let cell = CellPixels {
        width: size.width as u32 / size.columns as u32,
        height: size.height as u32 / size.rows as u32,
    };
    (cell.width > 0 && cell.height > 0).then_some(cell)
}

/// A picture as one frame shows it through the protocol: resampled to `drawn`, the part
/// `crop` (`[x, y, width, height]` in the drawn picture) placed at cell `(row, col)` shifted
/// by `offset` pixels into it. `key` names exactly those pixels, so an unchanged picture is
/// one stored image however many frames show it.
#[derive(Clone)]
pub(super) struct Shown {
    pub(super) picture: Arc<Picture>,
    pub(super) drawn: (u32, u32),
    pub(super) crop: [u32; 4],
    pub(super) row: u16,
    pub(super) col: u16,
    pub(super) offset: (u32, u32),
    pub(super) key: [u8; 16],
}

impl Shown {
    fn same_place(&self, other: &Placed) -> bool {
        self.key == other.key
            && self.row == other.row
            && self.col == other.col
            && self.offset == other.offset
    }
}

/// Where an `[:image …]` op's picture shows on a `screen` (`cols`, `rows`) of `cell`-sized
/// cells: the op's rect (`row`, `col`, `w`, `h` in cells) in pixels, the picture placed in it
/// by `fit` and `anchor` (`picture::place`, as the window places it), clipped to the rect and
/// the screen. `None` when nothing of it shows.
pub(super) fn plan(
    picture: &Arc<Picture>,
    rect: (i64, i64, i64, i64),
    fit: Fit,
    anchor: Anchor,
    cell: CellPixels,
    screen: (u16, u16),
) -> Option<Shown> {
    let (cell_w, cell_h) = (cell.width as i64, cell.height as i64);
    let (row, col, w, h) = rect;
    let box_w = u32::try_from(w.checked_mul(cell_w)?).ok()?;
    let box_h = u32::try_from(h.checked_mul(cell_h)?).ok()?;
    let (dx, dy, drawn_w, drawn_h) =
        picture::place(fit, anchor, picture.width(), picture.height(), box_w, box_h)?;
    if drawn_w as u64 * drawn_h as u64 > picture::MAX_DRAWN_PIXELS {
        return None;
    }
    let (box_left, box_top) = (col.checked_mul(cell_w)?, row.checked_mul(cell_h)?);
    let (left, top) = (box_left + dx, box_top + dy);
    let x0 = left.max(box_left).max(0);
    let y0 = top.max(box_top).max(0);
    let x1 = (left + drawn_w as i64)
        .min(box_left + box_w as i64)
        .min(screen.0 as i64 * cell_w);
    let y1 = (top + drawn_h as i64)
        .min(box_top + box_h as i64)
        .min(screen.1 as i64 * cell_h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let crop = [
        (x0 - left) as u32,
        (y0 - top) as u32,
        (x1 - x0) as u32,
        (y1 - y0) as u32,
    ];
    let mut hasher = blake3::Hasher::new();
    hasher.update(&picture.key);
    for number in [drawn_w, drawn_h, crop[0], crop[1], crop[2], crop[3]] {
        hasher.update(&number.to_le_bytes());
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    Some(Shown {
        picture: picture.clone(),
        drawn: (drawn_w, drawn_h),
        crop,
        row: (y0 / cell_h) as u16,
        col: (x0 / cell_w) as u16,
        offset: ((x0 % cell_w) as u32, (y0 % cell_h) as u32),
        key,
    })
}

/// The PNG a [`Shown`] picture is transmitted as: resampled to its drawn size, cropped, and
/// encoded fast (it is sent once and the terminal decodes it). `None` if encoding fails.
pub(super) fn encode_png(shown: &Shown) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let scaled = picture::resampled(&shown.picture, shown.drawn.0, shown.drawn.1);
    let [x, y, width, height] = shown.crop;
    let cropped = image::imageops::crop_imm(&scaled, x, y, width, height).to_image();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        cropped.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .ok()?;
    Some(png)
}

/// An image the terminal holds: its content key, id, size once decoded there, and when a
/// frame last showed it.
struct Stored {
    key: [u8; 16],
    id: u32,
    bytes: usize,
    used: u64,
}

/// A placement the screen shows.
struct Placed {
    key: [u8; 16],
    id: u32,
    placement: u32,
    row: u16,
    col: u16,
    offset: (u32, u32),
}

/// How many images the terminal is left holding at most, and how many of their decoded
/// bytes: under kitty's default storage quota (320 MB), which would otherwise evict for us.
const STORED_MAX: usize = 64;
const STORED_BUDGET: usize = 192 * 1024 * 1024;

/// How much Base64 one transmission escape carries: the protocol's chunk limit.
const CHUNK: usize = 4096;

/// What the terminal holds and shows, as this process told it.
pub(super) struct Kitty {
    stored: Vec<Stored>,
    placed: Vec<Placed>,
    next_id: u32,
    next_placement: u32,
    clock: u64,
}

impl Kitty {
    pub(super) const fn new() -> Self {
        Kitty {
            stored: Vec::new(),
            placed: Vec::new(),
            next_id: 1,
            next_placement: 1,
            clock: 0,
        }
    }

    /// Whether the terminal holds or shows anything of ours.
    pub(super) fn is_empty(&self) -> bool {
        self.stored.is_empty() && self.placed.is_empty()
    }

    /// Bring the screen's pictures to `wanted`, appending the escapes to `out`: placements
    /// no longer wanted deleted, new ones placed — a picture not yet stored transmitted
    /// first (`encode` gives its PNG) — and the least recently shown images freed past the
    /// bounds. `whole`: the frame was repainted whole, so every placement is deleted and made
    /// again. The escapes sit between a cursor save and restore, so the cursor the frame left
    /// stays where it is.
    pub(super) fn frame(
        &mut self,
        wanted: &[Shown],
        whole: bool,
        mut encode: impl FnMut(&Shown) -> Option<Vec<u8>>,
        out: &mut Vec<u8>,
    ) {
        let mut commands = Vec::new();
        if whole && !self.placed.is_empty() {
            commands.extend_from_slice(b"\x1b_Ga=d,d=a,q=2\x1b\\");
            self.placed.clear();
        }
        self.placed.retain(|placed| {
            let keep = wanted.iter().any(|shown| shown.same_place(placed));
            if !keep {
                commands.extend_from_slice(
                    format!(
                        "\x1b_Ga=d,d=i,i={},p={},q=2\x1b\\",
                        placed.id, placed.placement
                    )
                    .as_bytes(),
                );
            }
            keep
        });
        for shown in wanted {
            self.clock += 1;
            let clock = self.clock;
            if self.placed.iter().any(|placed| shown.same_place(placed)) {
                self.touch(shown.key, clock);
                continue;
            }
            let id = match self
                .stored
                .iter_mut()
                .find(|stored| stored.key == shown.key)
            {
                Some(stored) => {
                    stored.used = clock;
                    stored.id
                }
                None => {
                    let Some(png) = encode(shown) else {
                        continue;
                    };
                    let id = take_id(&mut self.next_id);
                    transmit(id, &png, &mut commands);
                    self.stored.push(Stored {
                        key: shown.key,
                        id,
                        bytes: shown.crop[2] as usize * shown.crop[3] as usize * 4,
                        used: clock,
                    });
                    id
                }
            };
            let placement = take_id(&mut self.next_placement);
            commands.extend_from_slice(
                format!(
                    "\x1b[{};{}H\x1b_Ga=p,i={id},p={placement},X={},Y={},C=1,q=2\x1b\\",
                    shown.row as u32 + 1,
                    shown.col as u32 + 1,
                    shown.offset.0,
                    shown.offset.1
                )
                .as_bytes(),
            );
            self.placed.push(Placed {
                key: shown.key,
                id,
                placement,
                row: shown.row,
                col: shown.col,
                offset: shown.offset,
            });
        }
        self.evict(&mut commands);
        if !commands.is_empty() {
            out.extend_from_slice(b"\x1b7");
            out.extend_from_slice(&commands);
            out.extend_from_slice(b"\x1b8");
        }
    }

    fn touch(&mut self, key: [u8; 16], clock: u64) {
        if let Some(stored) = self.stored.iter_mut().find(|stored| stored.key == key) {
            stored.used = clock;
        }
    }

    /// Free the least recently shown images no placement uses, until within the bounds.
    fn evict(&mut self, commands: &mut Vec<u8>) {
        loop {
            let bytes: usize = self.stored.iter().map(|stored| stored.bytes).sum();
            if self.stored.len() <= STORED_MAX && bytes <= STORED_BUDGET {
                return;
            }
            let placed = &self.placed;
            let Some(oldest) = self
                .stored
                .iter()
                .enumerate()
                .filter(|(_, stored)| !placed.iter().any(|placed| placed.id == stored.id))
                .min_by_key(|(_, stored)| stored.used)
                .map(|(index, _)| index)
            else {
                return;
            };
            let gone = self.stored.remove(oldest);
            commands.extend_from_slice(format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", gone.id).as_bytes());
        }
    }

    /// Delete every placement and free every image, appending the escapes to `out`: what
    /// leaving the screen does, so the terminal is not left holding pictures for nothing.
    pub(super) fn farewell(&mut self, out: &mut Vec<u8>) {
        if !self.placed.is_empty() {
            out.extend_from_slice(b"\x1b_Ga=d,d=a,q=2\x1b\\");
        }
        for stored in &self.stored {
            out.extend_from_slice(format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", stored.id).as_bytes());
        }
        self.placed.clear();
        self.stored.clear();
    }
}

/// The next id from `counter` — never 0, which the protocol reserves.
fn take_id(counter: &mut u32) -> u32 {
    if *counter == 0 {
        *counter = 1;
    }
    let id = *counter;
    *counter = counter.wrapping_add(1);
    id
}

/// The escapes that store `png` as image `id` without showing it: Base64 in chunks of
/// [`CHUNK`], each but the last marked `m=1`.
fn transmit(id: u32, png: &[u8], out: &mut Vec<u8>) {
    let encoded = crate::builtins::bytes::base64_standard(png);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(CHUNK).collect();
    let count = chunks.len().max(1);
    for index in 0..count {
        let more = u8::from(index + 1 < count);
        if index == 0 {
            out.extend_from_slice(format!("\x1b_Ga=t,f=100,t=d,i={id},q=2,m={more};").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        out.extend_from_slice(chunks.get(index).copied().unwrap_or(&[]));
        out.extend_from_slice(b"\x1b\\");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: u32, height: u32) -> Arc<Picture> {
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|index| [index as u8, 7, 9, 255])
            .collect();
        picture::from_rgba(width, height, &rgba).expect("a picture")
    }

    const CELL: CellPixels = CellPixels {
        width: 10,
        height: 20,
    };

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn the_environment_decides_without_asking() {
        assert_eq!(forced(Some("kitty")), Some(Protocol::Kitty));
        assert_eq!(forced(Some(" cells ")), Some(Protocol::Cells));
        assert_eq!(forced(Some("none")), Some(Protocol::Cells));
        assert_eq!(forced(Some("auto")), None);
        assert_eq!(forced(None), None);
    }

    #[test]
    fn the_answer_says_kitty_only_when_image_31_was_ok() {
        // kitty: the graphics answer, then DA1
        let kitty = b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22;52c";
        assert!(answered(kitty));
        assert_eq!(protocol_in(kitty), Protocol::Kitty);
        // a terminal that read the query and refused it
        let refused = b"\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\\x1b[?62;4c";
        assert_eq!(protocol_in(refused), Protocol::Cells);
        // xterm, tmux: DA1 alone (attribute 4, sixel, is not a kitty yes)
        let da1 = b"\x1b[?64;1;2;4;6c";
        assert!(answered(da1));
        assert_eq!(protocol_in(da1), Protocol::Cells);
        // not answered yet: the graphics reply alone, or a DA1 cut short
        assert!(!answered(b"\x1b_Gi=31;OK\x1b\\"));
        assert!(!answered(b"\x1b[?62;2"));
        // a key typed before the answer does not hide it
        assert!(answered(b"x\x1b[?1;2c"));
    }

    #[test]
    fn a_picture_is_planned_at_the_pixels_its_cells_cover() {
        // 100×100 into 4×2 cells of 10×20: a 40×40 box, the picture 40×40, centred
        let shown = plan(
            &picture(100, 100),
            (1, 2, 4, 2),
            Fit::Contain,
            Anchor::Center,
            CELL,
            (80, 24),
        )
        .expect("shows");
        assert_eq!(shown.drawn, (40, 40));
        assert_eq!(shown.crop, [0, 0, 40, 40]);
        // the box starts at (20, 20) px; the picture is the box's width, so x = 20, y = 20
        assert_eq!((shown.row, shown.col, shown.offset), (1, 2, (0, 0)));
        // a wide picture centred vertically lands mid-cell: 200×100 → 40×20, top 30 px
        let wide = plan(
            &picture(200, 100),
            (1, 2, 4, 2),
            Fit::Contain,
            Anchor::Center,
            CELL,
            (80, 24),
        )
        .expect("shows");
        assert_eq!(wide.drawn, (40, 20));
        assert_eq!((wide.row, wide.col, wide.offset), (1, 2, (0, 10)));
        // past the screen's right edge: cropped to it
        let cut = plan(
            &picture(100, 100),
            (0, 78, 4, 2),
            Fit::Fill,
            Anchor::TopLeft,
            CELL,
            (80, 24),
        )
        .expect("shows");
        assert_eq!(cut.crop, [0, 0, 20, 40]);
        // wholly off screen: nothing
        assert!(
            plan(
                &picture(10, 10),
                (30, 0, 2, 2),
                Fit::Contain,
                Anchor::Center,
                CELL,
                (80, 24)
            )
            .is_none()
        );
        // the same pixels have one key; another size another
        let again = plan(
            &picture(100, 100),
            (5, 7, 4, 2),
            Fit::Contain,
            Anchor::Center,
            CELL,
            (80, 24),
        )
        .expect("shows");
        assert_eq!(again.key, shown.key, "moved, not changed");
        assert_ne!(cut.key, shown.key);
    }

    fn shown_at(row: u16, col: u16, seed: u8) -> Shown {
        Shown {
            picture: picture(2, 2),
            drawn: (2, 2),
            crop: [0, 0, 2, 2],
            row,
            col,
            offset: (3, 4),
            key: [seed; 16],
        }
    }

    #[test]
    fn a_picture_is_transmitted_once_and_placed_per_change() {
        let mut kitty = Kitty::new();
        let mut encodes = 0;
        let mut encode = |_: &Shown| {
            encodes += 1;
            Some(b"PNG".to_vec())
        };
        let first = shown_at(2, 5, 1);
        let mut out = Vec::new();
        kitty.frame(std::slice::from_ref(&first), false, &mut encode, &mut out);
        assert_eq!(
            text(&out),
            "\x1b7\x1b_Ga=t,f=100,t=d,i=1,q=2,m=0;UE5H\x1b\\\
             \x1b[3;6H\x1b_Ga=p,i=1,p=1,X=3,Y=4,C=1,q=2\x1b\\\x1b8"
        );
        // the same frame again: nothing written
        let mut out = Vec::new();
        kitty.frame(std::slice::from_ref(&first), false, &mut encode, &mut out);
        assert!(out.is_empty());
        // moved: the old placement deleted, the stored image placed again, not resent
        let moved = shown_at(4, 5, 1);
        let mut out = Vec::new();
        kitty.frame(std::slice::from_ref(&moved), false, &mut encode, &mut out);
        assert_eq!(
            text(&out),
            "\x1b7\x1b_Ga=d,d=i,i=1,p=1,q=2\x1b\\\
             \x1b[5;6H\x1b_Ga=p,i=1,p=2,X=3,Y=4,C=1,q=2\x1b\\\x1b8"
        );
        // repainted whole: every placement deleted and made again, still not resent
        let mut out = Vec::new();
        kitty.frame(std::slice::from_ref(&moved), true, &mut encode, &mut out);
        assert_eq!(
            text(&out),
            "\x1b7\x1b_Ga=d,d=a,q=2\x1b\\\
             \x1b[5;6H\x1b_Ga=p,i=1,p=3,X=3,Y=4,C=1,q=2\x1b\\\x1b8"
        );
        // a frame with no picture deletes its placement and keeps the image
        let mut out = Vec::new();
        kitty.frame(&[], false, &mut encode, &mut out);
        assert_eq!(text(&out), "\x1b7\x1b_Ga=d,d=i,i=1,p=3,q=2\x1b\\\x1b8");
        // back again: placed from storage
        let mut out = Vec::new();
        kitty.frame(std::slice::from_ref(&moved), false, &mut encode, &mut out);
        assert!(!text(&out).contains("a=t"));
        // leaving frees it
        let mut out = Vec::new();
        kitty.farewell(&mut out);
        assert_eq!(
            text(&out),
            "\x1b_Ga=d,d=a,q=2\x1b\\\x1b_Ga=d,d=I,i=1,q=2\x1b\\"
        );
        assert!(kitty.is_empty());
        assert_eq!(encodes, 1);
    }

    #[test]
    fn a_long_picture_goes_in_chunks_and_old_images_are_freed() {
        let mut out = Vec::new();
        transmit(9, &vec![0u8; CHUNK], &mut out);
        let sent = text(&out);
        // 4096 bytes are 5464 Base64 characters: two escapes, the first marked m=1
        assert!(sent.starts_with("\x1b_Ga=t,f=100,t=d,i=9,q=2,m=1;"));
        assert_eq!(sent.matches("\x1b_Gm=0;").count(), 1);
        assert_eq!(sent.matches("\x1b\\").count(), 2);
        // past STORED_MAX images, the least recently shown not on screen are freed
        let mut kitty = Kitty::new();
        for seed in 0..=STORED_MAX as u8 {
            let mut out = Vec::new();
            kitty.frame(
                &[shown_at(0, 0, seed)],
                false,
                |_: &Shown| Some(vec![1]),
                &mut out,
            );
            if (seed as usize) < STORED_MAX {
                assert!(!text(&out).contains("d=I"));
            } else {
                assert!(
                    text(&out).contains("\x1b_Ga=d,d=I,i=1,q=2\x1b\\"),
                    "the oldest"
                );
            }
        }
        assert_eq!(kitty.stored.len(), STORED_MAX);
    }
}
