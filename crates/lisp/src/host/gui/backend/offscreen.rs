//! OFFSCREEN PAINTING (ADR-609): a frame painted into memory by the window's own CPU
//! painter, with no display — the picture a test, an agent or a person reviewing a UI
//! change can look at.
//!
//! Nothing here paints. A picture is the window's path, called without a window: the
//! frame is bled to the edges as `RedrawRequested` bleeds it (`bleed::edge_bleed` over
//! `Margins::of`), rasterised by `paint::raster_frame` — `Renderer::grid_origin`,
//! `bleed::grid_cells`, the glyph rasteriser, the inset and the background all the ones
//! the window uses — and the canvas read back as RGBA. A layout rule changed in the
//! renderer changes the picture, which is the point: a picture taken by a second
//! painter would prove nothing about the first.
//!
//! The painter lives on a thread of its own (`brood-offscreen`), as the window's lives on
//! the GUI thread: the text engine (`Families`) is not `Send`, and keeping one engine
//! keeps its glyph cache and the families registered with it across pictures. It serves
//! two things:
//!
//! - `render`: one frame painted as a window of a given size and look would paint it
//!   (`gui/render-frame`) — no window opened, so it works headless, in a test, or beside
//!   a real window.
//! - HEADLESS PAINTING windows (`BROOD_GUI_HEADLESS=paint`): a headless window that keeps
//!   what a real one keeps — its font, inset, background, line height, the frames drawn
//!   to it — and measures its grid with the real cell metrics, so an app's real loop
//!   lays out for the grid the picture is painted on. `gui/capture` paints its last
//!   frame. A plain `BROOD_GUI_HEADLESS=1` window stays the fixed nominal grid with no
//!   font engine, as before.

use super::bleed::{self, Margins};
use super::paint::raster_frame;
use super::render::{CpuTexture, Renderer, default_families};
use super::rows;
use super::{DEFAULT_FAMILY, Families, RenderDefaults, TextAa, deliver, resize_message};
use crate::core::sync::lock;
use crate::core::value;
use crate::host::gui::{FontChoice, FontInfo, FrameImage, Op, RenderSpec};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

/// What the offscreen thread is asked to do. The window requests mirror the GUI
/// thread's `UserEvent`s one for one, so a headless painting window takes the same
/// settings in the same order a real one does.
pub(super) enum Request {
    Open {
        id: u64,
        subscriber: u64,
        px: (u32, u32),
        size: Arc<Mutex<(u16, u16)>>,
        reply: Sender<()>,
    },
    Close {
        id: u64,
    },
    Draw {
        id: u64,
        ops: Vec<Op>,
    },
    Font {
        id: Option<u64>,
        family: Option<u32>,
        px: Option<f32>,
    },
    Inset(f32),
    Background(Option<[u8; 3]>),
    LineHeight(f32),
    TextAa(TextAa),
    TextContrast(f32),
    Ligatures(bool),
    RegisterFamily {
        name: u32,
        faces: Box<[Vec<u8>; 4]>,
        paths: [String; 4],
    },
    RegisterInstalled {
        name: u32,
        family_name: String,
    },
    Texture {
        id: u64,
        tex: u32,
        rgba: Vec<u8>,
        w: u32,
        h: u32,
    },
    TextureFree {
        id: u64,
        tex: u32,
    },
    Metrics {
        id: u64,
        px: Option<f32>,
        reply: Sender<Result<(u16, u16), String>>,
    },
    FontInfo {
        family: Option<u32>,
        reply: Sender<FontInfo>,
    },
    Capture {
        id: u64,
        reply: Sender<Result<FrameImage, String>>,
    },
    Render {
        ops: Vec<Op>,
        spec: RenderSpec,
        reply: Sender<FrameImage>,
    },
}

/// The offscreen thread's request channel, started on first use. Behind a `Mutex`
/// because Brood processes on several worker threads send to it.
fn thread() -> Result<&'static Mutex<Sender<Request>>, String> {
    static T: OnceLock<Result<Mutex<Sender<Request>>, String>> = OnceLock::new();
    T.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("brood-offscreen".into())
            .spawn(move || serve(receiver))
            .map(|_| Mutex::new(sender))
            .map_err(|e| format!("gui: cannot start the offscreen painter: {e}"))
    })
    .as_ref()
    .map_err(|e| e.clone())
}

/// Hand `request` to the offscreen thread.
pub(super) fn send(request: Request) -> Result<(), String> {
    lock(thread()?)
        .send(request)
        .map_err(|_| "gui: the offscreen painter is gone".to_string())
}

/// Send a request built around a reply channel and wait for the answer.
pub(super) fn ask<T>(make: impl FnOnce(Sender<T>) -> Request) -> Result<T, String> {
    let (reply, answer) = mpsc::channel();
    send(make(reply))?;
    answer
        .recv()
        .map_err(|_| "gui: the offscreen painter did not reply".to_string())
}

/// A headless painting window: what the GUI thread's `Win` keeps that a picture needs.
struct Window {
    renderer: Renderer,
    subscriber: u64,
    px: (u32, u32),
    size: Arc<Mutex<(u16, u16)>>,
    frame: Vec<Op>,
}

impl Window {
    /// Re-measure the grid after a metric change and, when it moved, tell the app — the
    /// `[:resize cols rows]` a real window delivers (`publish_size`), so its loop lays
    /// out again for the grid the picture is painted on.
    fn publish_size(&self) {
        let grid = bleed::grid_cells(&self.renderer, self.px.0 as usize, self.px.1 as usize);
        let before = std::mem::replace(&mut *lock(&self.size), grid);
        if before != grid {
            deliver(self.subscriber, resize_message(grid.0, grid.1));
        }
    }
}

/// The thread's state: one text engine, the window defaults the `gui-*!` calls set,
/// the headless painting windows, and the installed families `render` has resolved.
struct Painter {
    families: Families,
    defaults: RenderDefaults,
    windows: HashMap<u64, Window>,
    installed: HashMap<String, u32>,
}

fn serve(requests: Receiver<Request>) {
    let mut painter = Painter {
        families: default_families(),
        defaults: RenderDefaults::default(),
        windows: HashMap::new(),
        installed: HashMap::new(),
    };
    for request in requests {
        painter.handle(request);
    }
}

impl Painter {
    /// A renderer at scale `scale` with the window defaults applied — `build_window`'s.
    fn renderer(&self, scale: f64) -> Renderer {
        let mut renderer = Renderer::new(scale, self.families.clone(), self.defaults.px);
        if let Some(family) = self.defaults.family {
            renderer.set_font(Some(family), None);
        }
        renderer.set_inset(self.defaults.inset);
        renderer.set_bg(self.defaults.bg);
        renderer.set_line_height(self.defaults.line_height);
        renderer.set_text_aa(self.defaults.text_aa);
        renderer.set_text_contrast(self.defaults.text_contrast);
        renderer.set_ligatures(self.defaults.ligatures);
        renderer
    }

    /// Apply `change` to every window's renderer and re-measure each grid.
    fn each_window(&mut self, change: impl Fn(&mut Renderer)) {
        for window in self.windows.values_mut() {
            change(&mut window.renderer);
            window.publish_size();
        }
    }

    fn handle(&mut self, request: Request) {
        match request {
            Request::Open {
                id,
                subscriber,
                px,
                size,
                reply,
            } => {
                let renderer = self.renderer(1.0);
                *lock(&size) = bleed::grid_cells(&renderer, px.0 as usize, px.1 as usize);
                self.windows.insert(
                    id,
                    Window {
                        renderer,
                        subscriber,
                        px,
                        size,
                        frame: Vec::new(),
                    },
                );
                let _ = reply.send(());
            }
            Request::Close { id } => {
                self.windows.remove(&id);
            }
            Request::Draw { id, ops } => {
                if let Some(window) = self.windows.get_mut(&id) {
                    // the rows the frame lays out (ADR-610), as `UserEvent::Draw` takes them
                    if window.renderer.set_layout(rows::frame_layout(&ops)) {
                        window.publish_size();
                    }
                    window.frame = ops;
                }
            }
            Request::Font { id, family, px } => match id {
                Some(target) => {
                    if let Some(window) = self.windows.get_mut(&target) {
                        window.renderer.set_font(family, px);
                        window.publish_size();
                    }
                }
                None => {
                    if let Some(family) = family {
                        self.defaults.family = Some(family);
                    }
                    if let Some(px) = px {
                        self.defaults.px = px.max(1.0);
                    }
                    self.each_window(|renderer| renderer.set_font(family, px));
                }
            },
            Request::Inset(px) => {
                self.defaults.inset = px.max(0.0);
                self.each_window(|renderer| renderer.set_inset(px));
            }
            Request::Background(rgb) => {
                self.defaults.bg = rgb;
                self.each_window(|renderer| renderer.set_bg(rgb));
            }
            Request::LineHeight(mult) => {
                self.defaults.line_height = mult;
                self.each_window(|renderer| renderer.set_line_height(mult));
            }
            Request::TextAa(mode) => {
                self.defaults.text_aa = mode;
                self.each_window(|renderer| renderer.set_text_aa(mode));
            }
            Request::TextContrast(gamma) => {
                self.defaults.text_contrast = gamma;
                self.each_window(|renderer| renderer.set_text_contrast(gamma));
            }
            Request::Ligatures(on) => {
                self.defaults.ligatures = on;
                self.each_window(|renderer| renderer.set_ligatures(on));
            }
            Request::RegisterFamily { name, faces, paths } => {
                self.families.borrow_mut().register(name, *faces, paths);
                self.each_window(Renderer::fonts_changed);
            }
            Request::RegisterInstalled { name, family_name } => {
                self.families
                    .borrow_mut()
                    .register_installed(name, &family_name);
                self.each_window(Renderer::fonts_changed);
            }
            Request::Texture {
                id,
                tex,
                rgba,
                w,
                h,
            } => {
                if let Some(window) = self.windows.get_mut(&id) {
                    window
                        .renderer
                        .textures
                        .insert(tex, CpuTexture { w, h, rgba });
                }
            }
            Request::TextureFree { id, tex } => {
                if let Some(window) = self.windows.get_mut(&id) {
                    window.renderer.textures.remove(&tex);
                }
            }
            Request::Metrics { id, px, reply } => {
                let answer = match self.windows.get(&id) {
                    Some(window) => {
                        let metrics = match px {
                            Some(px) => window.renderer.metrics_at(px),
                            None => window.renderer.metrics(),
                        };
                        Ok((
                            metrics.cell_w.min(u16::MAX as usize) as u16,
                            metrics.cell_h.min(u16::MAX as usize) as u16,
                        ))
                    }
                    None => Err("gui window not open".to_string()),
                };
                let _ = reply.send(answer);
            }
            Request::FontInfo { family, reply } => {
                let family = family
                    .or(self.defaults.family)
                    .unwrap_or_else(|| value::intern(DEFAULT_FAMILY));
                let info = self.families.borrow().info(family, self.defaults.ligatures);
                let _ = reply.send(info);
            }
            Request::Capture { id, reply } => {
                let answer = match self.windows.get_mut(&id) {
                    Some(window) => {
                        let (w, h) = (window.px.0 as usize, window.px.1 as usize);
                        Ok(picture(&mut window.renderer, &window.frame, w, h))
                    }
                    None => Err("gui window not open, or not a headless painting window \
                         (BROOD_GUI_HEADLESS=paint)"
                        .to_string()),
                };
                let _ = reply.send(answer);
            }
            Request::Render { ops, spec, reply } => {
                let mut renderer = self.spec_renderer(&spec);
                let (w, h) = spec.size.unwrap_or((840, 560));
                let image = picture(
                    &mut renderer,
                    &ops,
                    w.clamp(1, MAX_EDGE) as usize,
                    h.clamp(1, MAX_EDGE) as usize,
                );
                let _ = reply.send(image);
            }
        }
    }

    /// A renderer set up as `spec` describes the window: the window defaults, then each
    /// setting `spec` names over them.
    fn spec_renderer(&mut self, spec: &RenderSpec) -> Renderer {
        let family = match &spec.font {
            Some(FontChoice::Registered(id)) => Some(*id),
            Some(FontChoice::Installed(name)) => Some(self.installed_family(name)),
            None => None,
        };
        let mut renderer = self.renderer(spec.scale.unwrap_or(1.0).clamp(0.25, 8.0));
        renderer.set_font(family, spec.px);
        if let Some(px) = spec.inset {
            renderer.set_inset(px);
        }
        if let Some(rgb) = spec.bg {
            renderer.set_bg(Some(rgb));
        }
        if let Some(mult) = spec.line_height {
            renderer.set_line_height(mult);
        }
        if let Some(mode) = spec.text_aa {
            renderer.set_text_aa(mode);
        }
        if let Some(on) = spec.ligatures {
            renderer.set_ligatures(on);
        }
        renderer
    }

    /// The family keyword an installed family `name` is registered under here, by name
    /// as `gui-font-register`'s `:family-name` resolves one (the bundled mono standing
    /// in for a name not installed). Registered once per name.
    fn installed_family(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.installed.get(name) {
            return id;
        }
        let id = value::intern(&format!("%offscreen/{name}"));
        self.families.borrow_mut().register_installed(id, name);
        self.installed.insert(name.to_string(), id);
        id
    }
}

/// The largest picture edge, in pixels: a 16K frame is already past any screen, and the
/// bound keeps a wild `:width` from asking for a canvas the size of memory.
const MAX_EDGE: u32 = 16_384;

/// `frame` painted by `r` as a `w`×`h` window paints it: its rows laid out as the frame
/// declares (`Op::Seam`, ADR-610 — as `UserEvent::Draw` takes them), bled to the window's
/// edges as `RedrawRequested` bleeds it, then [`raster`].
pub(super) fn picture(r: &mut Renderer, frame: &[Op], w: usize, h: usize) -> FrameImage {
    r.set_layout(rows::frame_layout(frame));
    let margins = Margins::of(r, w, h);
    let bled = bleed::edge_bleed(frame, &margins);
    raster(r, bled.as_deref().unwrap_or(frame), w, h)
}

/// An already-bled `frame` rasterised whole by `r` into its canvas and read back as
/// RGBA, the caret shown. The retained frame is forgotten afterwards, so a window whose
/// canvas this was repaints everything at its next paint rather than trusting a canvas
/// the picture may have moved past what its surface shows.
pub(super) fn raster(r: &mut Renderer, frame: &[Op], w: usize, h: usize) -> FrameImage {
    let caret_hidden = std::mem::replace(&mut r.caret_hidden, false);
    raster_frame(r, frame, w, h, true);
    r.caret_hidden = caret_hidden;
    r.invalidate();
    let mut rgba = Vec::with_capacity(w * h * 4);
    for &pixel in &r.canvas[..w * h] {
        rgba.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8, 255]);
    }
    let (origin_x, origin_y) = r.grid_origin(w, h);
    let grid = bleed::grid_cells(r, w, h);
    let map = r.rows(w, h);
    FrameImage {
        width: w as u32,
        height: h as u32,
        rgba,
        cell: (r.cell_w as u32, r.cell_h as u32),
        grid,
        origin: (origin_x as u32, origin_y as u32),
        inset: r.inset() as u32,
        seam: r.seam().filter(|&seam| seam < grid.1),
        row_tops: (0..=grid.1 as isize)
            .map(|row| map.y(row).clamp(0, u32::MAX as isize) as u32)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::gui::{Face, Op};

    const BACKGROUND: [u8; 3] = [10, 20, 30];
    const HEADER: [u8; 3] = [200, 40, 40];
    const BODY: [u8; 3] = [40, 200, 40];

    fn pixel(image: &FrameImage, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * image.width + x) * 4) as usize;
        [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
    }

    fn band(row: u16, h: u16, bg: [u8; 3], cols: u16) -> Op {
        Op::Rect {
            row,
            col: 0,
            w: cols,
            h,
            face: Face {
                bg: Some(bg),
                ..Face::default()
            },
            radius: 0.0,
        }
    }

    fn render(ops: Vec<Op>, inset: f32) -> FrameImage {
        let spec = RenderSpec {
            size: Some((400, 203)),
            inset: Some(inset),
            bg: Some(BACKGROUND),
            px: Some(15.0),
            ..RenderSpec::default()
        };
        ask(|reply| Request::Render { ops, spec, reply }).expect("the offscreen painter answers")
    }

    /// Row 0 starts at the window's top edge: a band on row 0 has its colour at pixel
    /// y = 0, with no strip of window background above it (ADR-508's 2026-11-05 rule).
    #[test]
    fn row_zero_paints_from_the_top_edge() {
        let cols = render(vec![Op::Clear], 6.0).grid.0;
        let image = render(vec![Op::Clear, band(0, 1, HEADER, cols)], 6.0);
        assert_eq!(image.origin.1, 0);
        assert_eq!(pixel(&image, image.width / 2, 0), HEADER);
        assert_eq!(pixel(&image, image.width / 2, image.cell.1 - 1), HEADER);
        assert_eq!(pixel(&image, image.width / 2, image.cell.1), BACKGROUND);
    }

    /// The inset is a margin on the left, right and bottom: a band that does not reach
    /// a grid edge leaves the margin in the window background, and the leftover height
    /// sits under the last row.
    #[test]
    fn the_inset_is_a_margin_left_right_and_below() {
        let rows = render(vec![Op::Clear], 6.0).grid.1;
        let inner = Op::Rect {
            row: 1,
            col: 1,
            w: 3,
            h: rows - 2,
            face: Face {
                bg: Some(BODY),
                ..Face::default()
            },
            radius: 0.0,
        };
        let image = render(vec![Op::Clear, inner], 6.0);
        assert_eq!(image.inset, 6);
        assert_eq!(image.origin, (6, 0));
        let (cell_w, cell_h) = image.cell;
        let grid_bottom = image.grid.1 as u32 * cell_h;
        assert!(image.height - grid_bottom >= image.inset, "{image:?}");
        // left margin and the first column are background, the band starts at column 1
        assert_eq!(pixel(&image, 2, cell_h + 2), BACKGROUND);
        assert_eq!(pixel(&image, 6 + cell_w - 1, cell_h + 2), BACKGROUND);
        assert_eq!(pixel(&image, 6 + cell_w, cell_h + 2), BODY);
        // below the grid: background all the way to the bottom edge
        assert_eq!(pixel(&image, 6 + cell_w, image.height - 1), BACKGROUND);
    }

    /// A band that reaches the grid's edges continues through the margin to the window's
    /// (edge bleed, ADR-508): the picture went through the window's bleed, not around it.
    #[test]
    fn a_full_width_band_bleeds_into_the_margins() {
        let (cols, rows) = render(vec![Op::Clear], 6.0).grid;
        let last = rows - 1;
        let image = render(vec![Op::Clear, band(last, 1, BODY, cols)], 6.0);
        let y = image.cell.1 * last as u32 + 2;
        assert_eq!(pixel(&image, 0, y), BODY);
        assert_eq!(pixel(&image, image.width - 1, y), BODY);
        assert_eq!(pixel(&image, image.width / 2, image.height - 1), BODY);
    }

    /// A glyph's ink lands inside its cell: an `M` on row 1, column 2 darkens pixels in
    /// that cell and nowhere in the cells around it.
    #[test]
    fn a_glyph_inks_inside_its_cell() {
        let text = Op::Text {
            row: 1,
            col: 2,
            s: "M".into(),
            face: Face {
                fg: Some([255, 255, 255]),
                ..Face::default()
            },
        };
        let image = render(vec![Op::Clear, text], 6.0);
        let (cell_w, cell_h) = image.cell;
        let inked = |x0: u32, y0: u32| {
            (y0..y0 + cell_h)
                .flat_map(|y| (x0..x0 + cell_w).map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(&image, x, y) != BACKGROUND)
                .count()
        };
        let (x, y) = (6 + 2 * cell_w, cell_h);
        assert!(inked(x, y) > 10, "the M left no ink in its cell");
        assert_eq!(inked(x - cell_w, y), 0, "ink in the cell before");
        assert_eq!(inked(x + cell_w, y), 0, "ink in the cell after");
        assert_eq!(inked(x, y - cell_h), 0, "ink in the cell above");
        assert_eq!(inked(x, y + cell_h), 0, "ink in the cell below");
    }

    // ---- the seam (ADR-610) ---------------------------------------------------------

    const ABOVE: [u8; 3] = [40, 40, 200];
    const MODE: [u8; 3] = [200, 200, 40];
    const ECHO: [u8; 3] = [40, 200, 200];

    /// A `w`×`h` picture at font size `px` (line height 1.4, inset 6) of `ops`.
    fn render_at(ops: Vec<Op>, w: u32, h: u32, px: f32) -> FrameImage {
        let spec = RenderSpec {
            size: Some((w, h)),
            inset: Some(6.0),
            bg: Some(BACKGROUND),
            px: Some(px),
            line_height: Some(1.4),
            ..RenderSpec::default()
        };
        ask(|reply| Request::Render { ops, spec, reply }).expect("the offscreen painter answers")
    }

    /// An editor's frame on a `cols`×`rows` grid with the seam at its mode line: a header
    /// band on row 0 (columns 1..4 only, so it does not bleed and its top shows), the body
    /// rows, the body's last row in its own colour (what the seam's gap must carry), the
    /// mode line on the seam row and the echo area on the last row.
    fn editor_frame(cols: u16, rows: u16, top_band: bool) -> Vec<Op> {
        let seam = rows - 2;
        vec![
            Op::Clear,
            Op::Seam {
                row: Some(seam),
                top_band,
            },
            Op::Rect {
                row: 0,
                col: 1,
                w: 3,
                h: 1,
                face: Face {
                    bg: Some(HEADER),
                    ..Face::default()
                },
                radius: 0.0,
            },
            band(1, seam - 2, BODY, cols),
            band(seam - 1, 1, ABOVE, cols),
            band(seam, 1, MODE, cols),
            band(rows - 1, 1, ECHO, cols),
        ]
    }

    /// THE LAYOUT PROPERTY, over every window height in one row's span and two font sizes,
    /// with row 0 a band and not: the last row ends exactly at the bottom inset; row 0 starts
    /// at y = 0 with the band and at the inset without; the leftover is painted by the row
    /// above the seam, so no pixel row of the window is left bare; and the hit-test gives the
    /// centre of every row back to that row, on both sides of the seam.
    #[test]
    fn the_seam_holds_the_leftover_at_every_height() {
        let x = 200u32;
        for px in [15.0f32, 21.0] {
            let cell_h = render_at(vec![Op::Clear], 400, 300, px).cell.1;
            for h in 300..300 + cell_h {
                for top_band in [true, false] {
                    // the grid for this layout: the seam op decides the top margin
                    let probe = vec![
                        Op::Clear,
                        Op::Seam {
                            row: None,
                            top_band,
                        },
                    ];
                    let (cols, rows) = render_at(probe, 400, h, px).grid;
                    let image = render_at(editor_frame(cols, rows, top_band), 400, h, px);
                    let at = |y: u32| pixel(&image, x, y);
                    let what = format!("px {px}, h {h}, top band {top_band}");
                    let tops = &image.row_tops;
                    let seam = rows - 2;
                    assert_eq!(image.seam, Some(seam), "{what}");
                    assert_eq!(tops.len(), rows as usize + 1, "{what}");
                    // the bottom: the last row ends on the inset, the inset is the echo's
                    assert_eq!(tops[rows as usize], h - 6, "{what}");
                    assert_eq!(at(h - 7), ECHO, "{what}");
                    assert_eq!(at(h - 6), ECHO, "{what}: the inset is bled echo");
                    assert_eq!(at(h - 1), ECHO, "{what}");
                    assert_eq!(at(h - 7 - cell_h), MODE, "{what}");
                    // the top: row 0 at y = 0 with a band, at the inset without
                    let top = if top_band { 0 } else { 6 };
                    assert_eq!(tops[0], top, "{what}");
                    assert_eq!(image.origin.1, top, "{what}");
                    let header_x = 6 + 2 * image.cell.0;
                    assert_eq!(pixel(&image, header_x, top), HEADER, "{what}");
                    if !top_band {
                        // the margin is the window's beside the header (column 0), and the
                        // header's own colour above it (a band on row 0 bleeds up, as one on
                        // the last row bleeds down)
                        assert_eq!(pixel(&image, 6, top - 1), BACKGROUND, "{what}");
                        assert_eq!(pixel(&image, 6, top), BACKGROUND, "{what}");
                        assert_eq!(pixel(&image, header_x, 0), HEADER, "{what}");
                    }
                    // rows above the seam are top-anchored, the gap sits at the seam
                    for k in 0..seam as usize {
                        assert_eq!(tops[k], top + k as u32 * cell_h, "{what}: row {k}");
                    }
                    let gap_from = tops[seam as usize - 1] + cell_h;
                    let gap_to = tops[seam as usize];
                    assert!(gap_to - gap_from < cell_h, "{what}");
                    for y in gap_from..gap_to {
                        assert_eq!(at(y), ABOVE, "{what}: the gap at y {y}");
                    }
                    // no bare pixel row anywhere down the middle of the window below the
                    // header (which is three columns wide, so that its top shows)
                    for y in tops[1]..h {
                        assert_ne!(at(y), BACKGROUND, "{what}: bare window at y {y}");
                    }
                    // the hit-test inverts the same map
                    let mut renderer = Renderer::new(1.0, default_families(), px);
                    renderer.set_line_height(1.4);
                    renderer.set_inset(6.0);
                    renderer.set_layout((Some(seam), top_band));
                    for k in 0..rows as usize {
                        let centre = (tops[k] + tops[k + 1]) as f64 / 2.0;
                        let position = winit::dpi::PhysicalPosition::new(x as f64, centre);
                        let (_, row) =
                            super::super::px_to_cell(position, &renderer, 400, h as usize);
                        assert_eq!(row as usize, k, "{what}: the centre of row {k}");
                    }
                }
            }
        }
    }

    /// No seam is the old layout exactly: rows from the top edge, the leftover under the
    /// last row (other programs are untouched by ADR-610).
    #[test]
    fn without_a_seam_the_leftover_stays_under_the_last_row() {
        let image = render_at(vec![Op::Clear], 400, 307, 15.0);
        let cell_h = image.cell.1;
        let rows = image.grid.1 as u32;
        assert_eq!(image.seam, None);
        assert_eq!(
            image.row_tops,
            (0..=rows).map(|k| k * cell_h).collect::<Vec<_>>()
        );
    }
}
