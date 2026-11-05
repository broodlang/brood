//! The text renderer: a shared `cosmic-text` engine, the glyph/cluster cache, and the
//! software rasteriser that paints one cell (or one cluster, or a rounded rect, or the
//! cursor) into the frame buffer.

use super::rows::RowMap;
use super::*;

use swash::scale::ScaleContext;

// ---- rasterising the cell grid ------------------------------------------

/// A rasterised grapheme cluster, baked into a small RGBA canvas sized to its
/// cell span (`width`×`height` px, the cluster's `string/display-width` cells wide). For
/// a `color` cluster (emoji) the RGBA is the glyph's own colors; for a monochrome
/// cluster the RGB is white and only the alpha carries coverage, so the caller
/// recolors it with the face `fg` at blit time (syntax colors vary per op). A
/// `subpixel` cluster is monochrome too, but its R/G/B bytes are the coverage of each
/// colour channel's third of the pixel (LCD text) — recoloured per channel at blit time.
pub(crate) struct CachedGlyph {
    pub(crate) color: bool,
    pub(crate) subpixel: bool,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>, // width*height*4, straight (non-premultiplied) alpha
}

/// How monochrome text is anti-aliased (behind `gui-text-aa!`). `Gray` is one coverage
/// value per pixel; `Subpixel` renders three — one per colour channel, each shifted a
/// third of a pixel — tripling the horizontal resolution of every stem on an LCD panel
/// whose subpixels run R-G-B left to right (`Bgr` for the other order). `Auto` (the
/// default) is subpixel at a 1× scale, where text has the fewest pixels to spend and the
/// gain is plainest, and gray on HiDPI, where grayscale is already sharp and the
/// compositor may scale or rotate the surface (which turns subpixel fringes into colour
/// noise).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAa {
    Auto,
    Gray,
    Subpixel,
    Bgr,
}

/// The grapheme-cluster part of a glyph-cache key. The vast majority of probes
/// are single chars (one per cell, one repaint per keystroke), so they key on a
/// `Char` and allocate nothing; only the rare multi-char cluster (ZWJ emoji,
/// flag, accented base+mark) takes the `Str` path and allocates a `Box<str>`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum ClusterKey {
    Char(char),
    Str(Box<str>),
}

impl ClusterKey {
    /// The cheapest key for a cluster: a lone char allocates nothing.
    fn of(g: &str) -> ClusterKey {
        let mut it = g.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => ClusterKey::Char(c),
            _ => ClusterKey::Str(g.into()),
        }
    }
}

/// The raster target: a `w`×`h` framebuffer plus the half-open pixel rect
/// `[x0, x1) × [y0, y1)` the primitives may touch. Pixels outside it are left exactly
/// as they were — that is what lets a paint re-rasterise only the cell rows whose ops
/// changed (`paint::strip_diff`), or only the columns of a row a change reached
/// (`paint::strip_columns`), and trust the rest of the retained canvas.
pub(super) struct Canvas<'a> {
    pub(super) buf: &'a mut [u32],
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) x0: usize,
    pub(super) x1: usize,
    pub(super) y0: usize,
    pub(super) y1: usize,
}

impl<'a> Canvas<'a> {
    /// The whole framebuffer (the fuzz tests' target; a paint always goes by band).
    #[cfg(test)]
    pub(super) fn full(buf: &'a mut [u32], w: usize, h: usize) -> Self {
        Canvas::band(buf, w, h, 0, h)
    }

    /// The framebuffer with painting confined to rows `[y0, y1)` (clamped to it).
    pub(super) fn band(buf: &'a mut [u32], w: usize, h: usize, y0: usize, y1: usize) -> Self {
        Canvas::rect(buf, w, h, (0, y0), (w, y1))
    }

    /// The framebuffer with painting confined to the rect from `(x0, y0)` to `(x1, y1)`
    /// (clamped to it).
    pub(super) fn rect(
        buf: &'a mut [u32],
        w: usize,
        h: usize,
        (x0, y0): (usize, usize),
        (x1, y1): (usize, usize),
    ) -> Self {
        Canvas {
            buf,
            w,
            h,
            x0: x0.min(w),
            x1: x1.min(w),
            y0: y0.min(h),
            y1: y1.min(h),
        }
    }

    /// The same columns, with painting further confined to rows `[y0, y1)` — a cell
    /// region's band inside whatever rect the paint is confined to.
    pub(super) fn narrowed(&mut self, y0: usize, y1: usize) -> Canvas<'_> {
        Canvas {
            buf: self.buf,
            w: self.w,
            h: self.h,
            x0: self.x0,
            x1: self.x1,
            y0: y0.max(self.y0).min(self.y1),
            y1: y1.min(self.y1),
        }
    }

    /// The rows of `[top, top + h)` that fall inside the band — the loop bounds every
    /// primitive clips to.
    #[inline]
    pub(super) fn rows(&self, top: usize, h: usize) -> std::ops::Range<usize> {
        let lo = top.max(self.y0);
        let hi = top.saturating_add(h).min(self.y1);
        lo..hi.max(lo)
    }

    /// The columns of `[left, left + w)` that fall inside the rect — `rows`' twin.
    #[inline]
    pub(super) fn cols(&self, left: usize, w: usize) -> std::ops::Range<usize> {
        let lo = left.max(self.x0);
        let hi = left.saturating_add(w).min(self.x1);
        lo..hi.max(lo)
    }

    /// Fill rows `[top, top + h)` of the rect with `color` — the clear.
    pub(super) fn fill_rows(&mut self, top: usize, h: usize, color: u32) {
        let w = self.w;
        let (x0, x1) = (self.x0, self.x1.max(self.x0));
        for y in self.rows(top, h) {
            self.buf[y * w + x0..y * w + x1].fill(color);
        }
    }
}

/// The shared text engine on the single GUI thread: cosmic-text's `FontSystem`
/// (font database + shaping + fallback) and `SwashCache` (glyph rasterisation,
/// color and mono), plus the family-keyword → family-name map a `:family` resolves
/// through. Shared by every window's renderer (so `gui-font-register` reaches them
/// all), like the old family registry.
///
/// Two font databases, on purpose (ADR-442). The SHAPING one (`fs`) holds only the faces
/// in use: the bundled DejaVu and emoji, the families registered, and the fallback faces
/// a missing glyph pulled in. The SYSTEM one (`system`) is every installed face, scanned
/// from the fontconfig directories the first time something needs it — a family asked
/// for by name, or a glyph nothing loaded can draw — and never before, so a window that
/// only draws ASCII in the bundled font opens without touching the disk. Keeping the
/// system faces out of the shaping database is what keeps cosmic-text's own fallback
/// walk (which visits every face in its database for a glyph it cannot place) short.
pub(super) struct FontShared {
    fs: FontSystem,
    swash: SwashCache,
    /// swash's own scaler, for the subpixel (LCD) raster path: cosmic-text's
    /// `SwashCache` renders alpha masks only, so a per-channel mask is rendered here
    /// from the same font + glyph id + hinting.
    scaler: ScaleContext,
    /// interned family keyword id → fontdb family name (`:mono` → "DejaVu Sans Mono").
    names: HashMap<u32, String>,
    /// interned family keyword id → where its faces came from (`gui-font-info`).
    resolutions: HashMap<u32, FontResolution>,
    /// The bundled faces a missing glyph is looked for in before the system: the
    /// regular DejaVu and the colour emoji. cosmic-text's fallback already reaches both,
    /// so a glyph either covers is left to it — what the renderer did before system
    /// fallback existed.
    bundled: Vec<fontdb::ID>,
    /// Every installed face and its block index, scanned on first need (see the type's
    /// doc, and `SystemFonts`).
    system: Option<SystemFonts>,
    /// The system faces loaded into the shaping database as fallbacks, `(family,
    /// file)`, in the order they were first needed — the first place the next missing
    /// glyph is looked for, so a screen of CJK scans the system once, not per char.
    fallbacks: Vec<(String, String)>,
    /// cluster → the fallback family that draws it (`None`: nothing installed does).
    /// Keyed by the WHOLE cluster: `क्ष` and `कि` share a first char but not their marks.
    fallback_for: HashMap<String, Option<String>>,
    /// Font files already loaded into the shaping database.
    loaded_files: std::collections::HashSet<std::path::PathBuf>,
    /// Bumped by every registration: a family keyword can now name different faces, so
    /// a glyph cached under it (the GPU atlas keys by family keyword) is stale.
    generation: u64,
}

/// Where a registered family's faces came from — the backend side of
/// `super::super::FontInfo`.
#[derive(Clone, Debug)]
struct FontResolution {
    requested: Option<String>,
    origin: &'static str,
    faces: [String; 4],
}

/// The four styles a family registers, in `FontInfo::faces` order.
const STYLES: [(Weight, Style); 4] = [
    (Weight::NORMAL, Style::Normal),
    (Weight::BOLD, Style::Normal),
    (Weight::NORMAL, Style::Italic),
    (Weight::BOLD, Style::Italic),
];

/// A codepoint that rides on the glyph before it rather than needing its own: the
/// zero-width joiner and the variation selectors. A font need not cover these for the
/// cluster they sit in to be its.
fn rides_along(c: char) -> bool {
    matches!(c, '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// The file behind a face source, for diagnostics (`None` for in-memory bytes).
fn source_path(source: &fontdb::Source) -> Option<std::path::PathBuf> {
    match source {
        fontdb::Source::File(path) => Some(path.clone()),
        fontdb::Source::SharedFile(path, _) => Some(path.clone()),
        fontdb::Source::Binary(_) => None,
    }
}

impl FontShared {
    pub(super) fn new() -> Self {
        let src = |b: &'static [u8]| fontdb::Source::Binary(Rc2::new(b));
        // Load the bundled mono faces + the emoji fallback; system fonts only on demand
        // (`ensure_system`), so the default editor renders identically everywhere
        // (self-contained, ADR-046) and opens without a font scan.
        let fs = FontSystem::new_with_fonts([
            src(FONT_REGULAR),
            src(FONT_BOLD),
            src(FONT_ITALIC),
            src(FONT_BOLD_ITALIC),
            src(FONT_EMOJI),
        ]);
        let bundled = [MONO_FAMILY, EMOJI_FAMILY]
            .iter()
            .filter_map(|family| query_face(fs.db(), family, Weight::NORMAL, Style::Normal))
            .collect();
        let mono = value::intern(DEFAULT_FAMILY);
        let mut names = HashMap::new();
        names.insert(mono, MONO_FAMILY.to_string());
        let mut resolutions = HashMap::new();
        resolutions.insert(mono, bundled_resolution());
        FontShared {
            fs,
            swash: SwashCache::new(),
            scaler: ScaleContext::new(),
            names,
            resolutions,
            bundled,
            system: None,
            fallbacks: Vec::new(),
            fallback_for: HashMap::new(),
            loaded_files: Default::default(),
            generation: 0,
        }
    }

    /// The fontdb family name for keyword id `id` (the bundled mono if unknown).
    fn name_of(&self, id: u32) -> String {
        self.names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| MONO_FAMILY.to_string())
    }

    /// Register a family from raw TTF bytes per style (behind `gui-font-register`):
    /// load all four faces into the db, and map `id` to the regular face's family
    /// name so an `Attrs` built for it picks the right faces (weight/style matched).
    /// `paths` are the files the bytes came from, kept for `gui-font-info`.
    pub(super) fn register(&mut self, id: u32, faces: [Vec<u8>; 4], paths: [String; 4]) {
        let [regular, bold, italic, bold_italic] = faces;
        let ids = self
            .fs
            .db_mut()
            .load_font_source(fontdb::Source::Binary(Rc2::new(regular)));
        let family = ids
            .first()
            .and_then(|face_id| self.fs.db().face(*face_id))
            .and_then(|face| face.families.first().map(|(name, _)| name.clone()));
        for bytes in [bold, italic, bold_italic] {
            self.fs.db_mut().load_font_data(bytes);
        }
        self.generation += 1;
        if let Some(family) = family {
            self.names.insert(id, family);
            self.resolutions.insert(
                id,
                FontResolution {
                    requested: None,
                    origin: "files",
                    faces: paths,
                },
            );
        }
    }

    /// Register a family by its INSTALLED name (behind `gui-font-register`'s
    /// `:family-name`): find it in the system database (case-insensitively, scanning the
    /// system on first use), pick each style's face by weight and slant, and load those
    /// files into the shaping database. A name nothing installed answers to maps `id` to
    /// the bundled mono, and `gui-font-info` says so (`"not-found"`).
    pub(super) fn register_installed(&mut self, id: u32, requested: &str) {
        self.generation += 1;
        let system = &self.system().db;
        let wanted = requested.trim();
        let family = system.faces().find_map(|face| {
            face.families
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(name, _)| name.clone())
        });
        let Some(family) = family else {
            self.names.insert(id, MONO_FAMILY.to_string());
            let mut resolution = bundled_resolution();
            resolution.requested = Some(wanted.to_string());
            resolution.origin = "not-found";
            self.resolutions.insert(id, resolution);
            return;
        };
        // what `gui-font-info` reports: the file each style resolves to, by the same CSS
        // matching shaping will run over the family's faces
        let mut faces: [String; 4] = Default::default();
        for (slot, (weight, style)) in STYLES.iter().enumerate() {
            if let Some(face_id) = query_face(system, &family, *weight, *style)
                && let Some((source, _)) = system.face_source(face_id)
            {
                faces[slot] = source_path(&source)
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
            }
        }
        self.load_family(&family);
        self.names.insert(id, family);
        self.resolutions.insert(
            id,
            FontResolution {
                requested: Some(wanted.to_string()),
                origin: "system",
                faces,
            },
        );
    }

    /// What `gui-font-info` reports for family keyword `id` (the bundled mono for one
    /// never registered, as `name_of` resolves it).
    pub(super) fn info(&self, id: u32, ligatures: bool) -> super::super::FontInfo {
        let resolution = self
            .resolutions
            .get(&id)
            .cloned()
            .unwrap_or_else(bundled_resolution);
        super::super::FontInfo {
            name: self.name_of(id),
            requested: resolution.requested,
            origin: resolution.origin,
            faces: resolution.faces,
            fallbacks: self.fallbacks.clone(),
            system_scanned: self.system.is_some(),
            ligatures,
        }
    }

    /// The installed fonts, scanned the first time anything asks — never at startup.
    fn system(&mut self) -> &SystemFonts {
        self.system.get_or_insert_with(SystemFonts::scan)
    }

    /// Load every installed face of `family` into the shaping database, so each weight and
    /// slant a face asks for finds its own file.
    fn load_family(&mut self, family: &str) {
        let sources: Vec<fontdb::Source> = self
            .system()
            .db
            .faces()
            .filter(|face| face.families.iter().any(|(name, _)| name == family))
            .map(|face| face.source.clone())
            .collect();
        for source in sources {
            self.load_into_shaping(source);
        }
    }

    /// Load one font file into the shaping database, once per file.
    fn load_into_shaping(&mut self, source: fontdb::Source) {
        if let Some(path) = source_path(&source)
            && !self.loaded_files.insert(path)
        {
            return;
        }
        self.fs.db_mut().load_font_source(source);
    }

    /// Whether face `id` of the shaping database has a glyph for every char of `chars`.
    fn covers(&mut self, id: fontdb::ID, chars: &[char]) -> bool {
        let Some(font) = self.fs.get_font(id, Weight::NORMAL) else {
            return false;
        };
        draws_all(&font.as_swash(), chars)
    }

    /// The family to shape cluster `text` in, when it is not `family`: `Some` only when
    /// neither `family` (in this weight and slant) nor a bundled face can draw it and an
    /// installed font can — a CJK ideograph under DejaVu, a symbol a programming font
    /// lacks. `None` leaves the cluster to cosmic-text exactly as before, which is what
    /// keeps every glyph the bundled fonts already drew (emoji included) unchanged.
    pub(super) fn fallback_family(
        &mut self,
        text: &str,
        family: &str,
        bold: bool,
        italic: bool,
    ) -> Option<String> {
        if text.is_ascii() && family == MONO_FAMILY {
            return None; // DejaVu covers all of printable ASCII: the hot path asks nothing
        }
        let chars: Vec<char> = text.chars().filter(|c| !rides_along(*c)).collect();
        if chars.is_empty() {
            return None;
        }
        let weight = if bold { Weight::BOLD } else { Weight::NORMAL };
        let style = if italic { Style::Italic } else { Style::Normal };
        if let Some(primary) = query_face(self.fs.db(), family, weight, style)
            && self.covers(primary, &chars)
        {
            return None;
        }
        let bundled = self.bundled.clone();
        if bundled.into_iter().any(|id| self.covers(id, &chars)) {
            return None;
        }
        self.system_fallback(&chars)
    }

    /// An installed family that draws `chars` (one cluster), loading it into the shaping
    /// database on first use: the fallbacks already loaded first, then the system's
    /// regular faces that touch the cluster's block, monospaced ones first.
    fn system_fallback(&mut self, chars: &[char]) -> Option<String> {
        let key: String = chars.iter().collect();
        if let Some(known) = self.fallback_for.get(&key) {
            return known.clone();
        }
        let loaded: Vec<String> = self
            .fallbacks
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        for name in loaded {
            if let Some(id) = query_face(self.fs.db(), &name, Weight::NORMAL, Style::Normal)
                && self.covers(id, chars)
            {
                self.fallback_for.insert(key, Some(name.clone()));
                return Some(name);
            }
        }
        let system = self.system();
        let candidates = system
            .blocks
            .get(&(chars[0] as u32 >> 7))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let found = scan_system(&system.db, candidates, chars);
        let Some((family, path)) = found else {
            self.fallback_for.insert(key, None);
            return None;
        };
        self.load_family(&family);
        self.fallbacks.push((family.clone(), path));
        self.fallback_for.insert(key, Some(family.clone()));
        Some(family)
    }
}

/// The face of `family` closest to `weight`/`style` in `db`, by fontdb's CSS matching.
fn query_face(
    db: &fontdb::Database,
    family: &str,
    weight: Weight,
    style: Style,
) -> Option<fontdb::ID> {
    db.query(&fontdb::Query {
        families: &[Family::Name(family)],
        weight,
        stretch: fontdb::Stretch::Normal,
        style,
    })
}

/// Every installed face (fontconfig's directories on Linux), with `blocks`: each
/// 128-codepoint block → the regular faces that have any glyph in it, in fallback preference
/// — monospaced first (a fallback glyph sits in a cell), then the Noto and DejaVu families a
/// desktop usually has for coverage, then the rest. A lookup then opens only the faces that
/// can draw its block, and a block no face touches (a Nerd-font icon range without a Nerd
/// font) costs nothing.
struct SystemFonts {
    db: fontdb::Database,
    blocks: HashMap<u32, Vec<fontdb::ID>>,
}

impl SystemFonts {
    /// Read every font file's header and every candidate's character map, once
    /// (memory-mapped) — the cost the first lookup that needs it pays.
    fn scan() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let blocks = block_index(&db);
        SystemFonts { db, blocks }
    }
}

fn block_index(system: &fontdb::Database) -> HashMap<u32, Vec<fontdb::ID>> {
    let rank = |face: &fontdb::FaceInfo| {
        let name = face
            .families
            .first()
            .map(|(name, _)| name.as_str())
            .unwrap_or("");
        if face.monospaced {
            0
        } else if name.starts_with("Noto Sans") || name.starts_with("DejaVu") {
            1
        } else {
            2
        }
    };
    let mut candidates: Vec<&fontdb::FaceInfo> = system
        .faces()
        .filter(|face| face.style == Style::Normal && (350..=500).contains(&face.weight.0))
        .collect();
    candidates.sort_by_key(|face| rank(face));
    let mut index: HashMap<u32, Vec<fontdb::ID>> = HashMap::new();
    for face in candidates {
        let mut blocks = std::collections::BTreeSet::new();
        system.with_face_data(face.id, |data, face_index| {
            if let Some(font) = swash::FontRef::from_index(data, face_index as usize) {
                font.charmap()
                    .enumerate(|codepoint, _| _ = blocks.insert(codepoint >> 7));
            }
        });
        for block in blocks {
            index.entry(block).or_default().push(face.id);
        }
    }
    index
}

/// The first of `candidates` (faces of `system`, in preference order) that draws every
/// char of `chars`, as its family and the file it was found in.
fn scan_system(
    system: &fontdb::Database,
    candidates: &[fontdb::ID],
    chars: &[char],
) -> Option<(String, String)> {
    candidates.iter().find_map(|&id| {
        let covered = system
            .with_face_data(id, |data, index| {
                swash::FontRef::from_index(data, index as usize)
                    .is_some_and(|font| draws_all(&font, chars))
            })
            .unwrap_or(false);
        let face = system.face(id)?;
        let family = face.families.first().map(|(name, _)| name.clone())?;
        covered.then(|| {
            let path = source_path(&face.source)
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            (family, path)
        })
    })
}

/// Whether `font` has a glyph for every char of `chars`.
fn draws_all(font: &swash::FontRef, chars: &[char]) -> bool {
    let charmap = font.charmap();
    chars.iter().all(|&c| charmap.map(c) != 0)
}

/// The bundled DejaVu's resolution — the `:mono` family, and what a family name that is
/// not installed falls back to.
fn bundled_resolution() -> FontResolution {
    FontResolution {
        requested: None,
        origin: "bundled",
        faces: [
            "<bundled DejaVuSansMono.ttf>".to_string(),
            "<bundled DejaVuSansMono-Bold.ttf>".to_string(),
            "<bundled DejaVuSansMono-Oblique.ttf>".to_string(),
            "<bundled DejaVuSansMono-BoldOblique.ttf>".to_string(),
        ],
    }
}

// fontdb's `Source::Binary` wants an `Arc<dyn AsRef<[u8]> + Send + Sync>`; alias it
// so the bundled `&'static [u8]` and the registered `Vec<u8>` both drop straight in.
use std::sync::Arc as Rc2;

/// Build the shared text engine seeded with the bundled `:mono` family + emoji.
pub(super) fn default_families() -> Families {
    Rc::new(RefCell::new(FontShared::new()))
}

/// The largest font size the renderer will measure or paint with, in physical px. A cell
/// that big already fills a 4K screen; the bound is what keeps a wild `px` in a render op
/// from asking the shaper for a line box the size of the address space.
pub(super) const MAX_FONT_PX: f32 = 512.0;

/// The cell metrics one font size produces. The renderer holds the window's set, and
/// `Op::CellRegion` swaps in another for the span of its ops — which is how ONE FRAME
/// carries two text sizes (a per-buffer zoom).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct CellMetrics {
    pub(crate) px: f32,
    pub(crate) cell_w: usize,
    pub(crate) cell_h: usize,
    pub(super) baseline: i32,
}

pub(crate) struct Renderer {
    families: Families,
    default_family: u32,
    base_px: f32,
    scale: f64,
    px: f32,
    pub(crate) cell_w: usize,
    pub(crate) cell_h: usize,
    baseline: i32,   // pixels from a cell's top to the text baseline
    base_inset: f32, // logical-px content margin before the grid (ADR-079); 0 = flush
    /// The seam row the frame being painted declared (`Op::Seam`, ADR-610): rows from it
    /// down are anchored to the bottom ([`Renderer::rows`]). Set from each frame that
    /// arrives (`set_layout`).
    seam: Option<u16>,
    /// Whether row 0 is a band (no margin above it) — `Op::Seam`'s `top_band`, true for a
    /// frame without one.
    top_band: bool,
    bg: Option<[u8; 3]>, // window background (clear/inset-margin fill); None = DEFAULT_BG
    line_height: f32,    // cell height as a multiple of the font px (`gui-line-height!`)
    text_aa: TextAa,     // how monochrome text is anti-aliased (`gui-text-aa!`)
    /// The text contrast exponent γ (`gui-text-contrast!`): a glyph's partial coverage
    /// is lifted to `cov^(1/γ)` where the text is lighter than what it is drawn over.
    /// 1.0 is the plain linear-light blend.
    text_contrast: f32,
    /// `cov -> lifted cov` for the current `text_contrast`, 256 entries; identity at 1.0.
    cov_lut: Box<[u8; 256]>,
    /// Whether runs of punctuation are shaped as one unit, so a font's programming
    /// ligatures (`->`, `!=`, `===`) form (`gui-ligatures!`, ADR-442). Off by default.
    ligatures: bool,

    // Keyed by (cluster, family id, bold, italic, scale, subpixel, PX BITS): the same
    // cluster at a different family/style/scale/AA/size rasterises to a different baked
    // canvas. The px is in the key (as `f32::to_bits`, since f32 is not `Eq`) so entries for
    // DIFFERENT SIZES coexist. They did not before, which had two costs: a size change had
    // to clear the whole cache and re-rasterise every glyph on screen (the jank in a
    // continuous zoom), and two regions could never be drawn at two sizes in one frame —
    // which is what per-buffer zoom needs.
    pub(super) cache: HashMap<(ClusterKey, u32, bool, bool, u16, bool, u32), CachedGlyph>,

    /// `px bits -> (cell_w, cell_h, baseline)`: shaping the reference glyph is not free and a
    /// frame with per-buffer zoom asks for the same handful of sizes on every paint. Dropped
    /// whenever the inputs to that shaping change (`recompute` clears it).
    metrics_memo: RefCell<HashMap<u32, (usize, usize, i32)>>,

    // The retained frame (see `paint`): `canvas` holds the pixels of the last frame
    // rasterised, `prev_ops` the ops that produced it. A new frame is diffed against
    // `prev_ops` per cell row and only the rows that changed are re-rasterised into
    // `canvas`; `damage_ring` is the per-frame list of changed pixel rects of recent
    // frames (oldest→newest), so a present can cover the last `buffer.age()` frames.
    pub(super) canvas: Vec<u32>,
    /// `canvas`'s `(width, height)` — compared as a pair, not a length: a rotation of
    /// the window (800×600 → 600×800) keeps the pixel count and changes every row.
    pub(super) canvas_size: (usize, usize),
    pub(super) prev_ops: Vec<Op>,
    /// Whether the frame's blinking caret is in its hidden half for THIS paint — set by the
    /// window from its blink clock before each paint (ADR-469) — and whether it was for the
    /// paint that left `canvas` as it is. Both painters skip a hidden blinking caret; the
    /// CPU one re-rasterises only the caret's rows when the two differ.
    pub(crate) caret_hidden: bool,
    pub(super) prev_caret_hidden: bool,
    pub(super) damage_ring: Vec<Vec<DamageRect>>,
    /// The pixel rows the last `raster_frame` copied from the old canvas instead of
    /// drawing (the scroll blit) — a per-paint stat for the trace and the tests.
    pub(super) blit_rows: usize,
    /// The textures a game uploaded (`gui/texture`), by handle, for the CPU painter's
    /// `[:sprite …]`: straight-alpha RGBA rows. The GPU target keeps its own copies on
    /// the device; a window has one or the other.
    pub(super) textures: HashMap<u32, CpuTexture>,
    /// The window's particle emitters (`[:emitter …]`), stepped once per paint by whichever
    /// target paints, and the CPU painter's own copy of their particles.
    pub(crate) emitters: crate::host::gui::effects::EmitterBook,
    pub(super) cpu_particles: crate::host::gui::effects::CpuParticles,
}

/// A texture as the CPU painter samples it: `w`×`h` straight-alpha RGBA bytes.
pub(super) struct CpuTexture {
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) rgba: Vec<u8>,
}

impl Renderer {
    pub(super) fn new(scale: f64, families: Families, base_px: f32) -> Self {
        let mut r = Renderer {
            families,
            default_family: value::intern(DEFAULT_FAMILY),
            base_px,
            scale,
            px: base_px,
            cell_w: 1,
            cell_h: 1,
            baseline: 0,
            base_inset: 0.0,
            seam: None,
            top_band: true,
            bg: None,
            line_height: LINE_HEIGHT,
            text_aa: TextAa::Auto,
            text_contrast: 1.0,
            cov_lut: Box::new(contrast_lut(1.0)),
            ligatures: false,
            cache: HashMap::new(),
            metrics_memo: RefCell::new(HashMap::new()),
            canvas: Vec::new(),
            canvas_size: (0, 0),
            prev_ops: Vec::new(),
            caret_hidden: false,
            prev_caret_hidden: false,
            damage_ring: Vec::new(),
            blit_rows: 0,
            textures: HashMap::new(),
            emitters: Default::default(),
            cpu_particles: Default::default(),
        };
        r.recompute();
        r
    }

    /// The content inset in PHYSICAL pixels (the logical `base_inset` × HiDPI
    /// scale) — the margin on the grid's left, right and bottom, and above row 0 when row 0
    /// is not a band ([`Renderer::rows`]); every pixel↔cell conversion goes through the row
    /// map built from it, so the grid the renderer draws and the one the mouse hit-tests stay
    /// the same. 0 leaves the grid flush to the window edge (the default).
    pub(crate) fn inset(&self) -> usize {
        (self.base_inset * self.scale as f32).round().max(0.0) as usize
    }

    /// Build (and cache) a cluster's rasterised RGBA glyph and hand back a reference —
    /// the GPU backend uploads it to a texture atlas. Same key/build/cache path as
    /// `draw_cluster`, minus the CPU blit.
    ///
    /// `subpixel` asks for per-channel coverage (LCD text) — only when the GPU can blend it per
    /// channel ([`Renderer::lcd_order`]); a glyph with no outline still comes back grey,
    /// which `CachedGlyph::subpixel` says.
    // The `gui-gpu` glyph-atlas upload path calls this (`gui/gpu.rs`); it's unused in a
    // CPU-only `gui` build, so suppress dead-code only when `gui-gpu` is off.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn cluster_glyph(
        &mut self,
        g: &str,
        family: Option<u32>,
        bold: bool,
        italic: bool,
        scale: u16,
        subpixel: bool,
    ) -> &CachedGlyph {
        let fid = family.unwrap_or(self.default_family);
        let key = (
            ClusterKey::of(g),
            fid,
            bold,
            italic,
            scale.max(1),
            subpixel,
            self.px.to_bits(),
        );
        if !self.cache.contains_key(&key) {
            let baked = self.build_cluster(g, fid, bold, italic, scale, subpixel);
            self.cache.insert(key.clone(), baked);
        }
        &self.cache[&key]
    }

    /// The family a face without `:family` is drawn in — part of the GPU atlas's glyph
    /// key, since `gui-font!` changes it at the same px.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn default_family(&self) -> u32 {
        self.default_family
    }

    /// The text contrast exponent γ (`gui-text-contrast!`) — the GPU shader's lift.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn text_contrast(&self) -> f32 {
        self.text_contrast
    }

    /// Set the content inset (logical px). The cell metrics don't change — only the
    /// grid's origin and how many cells fit — so the caller recomputes the grid
    /// (`update_cells`) and repaints; no glyph-cache drop (unlike `set_font`).
    pub(super) fn set_inset(&mut self, px: f32) {
        self.base_inset = px.max(0.0);
        self.invalidate();
    }

    /// The window background colour — the fill for `Op::Clear`, the pre-clear, and
    /// (since it's outside every cell) the inset margin + the snap remainder. `None`
    /// falls back to `DEFAULT_BG`. Behind `gui-bg!`, so an app's padding matches its
    /// theme instead of showing the hardcoded default.
    pub(crate) fn bg(&self) -> [u8; 3] {
        self.bg.unwrap_or(DEFAULT_BG)
    }

    /// Set the window background (clear/inset-margin fill). No metric change — only a
    /// repaint — so the caller just requests a redraw.
    pub(super) fn set_bg(&mut self, rgb: Option<[u8; 3]>) {
        self.bg = rgb;
        self.invalidate();
    }

    /// The grid's top-left origin in PHYSICAL px: `(inset, top)`, where `top` is 0 when
    /// row 0 is a band and the inset when it is not ([`Renderer::rows`] has the rule).
    ///
    /// Why not centre horizontally on the leftover: it is `width % cell_w`, so it depends on
    /// the cell size, and a font-size zoom would slide the grid sideways on every step. The
    /// columns stay left-anchored; the leftover width sits beyond the last column.
    pub(crate) fn grid_origin(&self, _w_px: usize, _h_px: usize) -> (usize, usize) {
        (self.inset(), self.grid_top())
    }

    /// The pixel row grid row 0 starts at: the window's top edge when row 0 is a band (a
    /// header line, a tab bar), so the band is the top of the window, its text centred in
    /// its own row; the inset otherwise, so the first line of text has the margin the
    /// sides have (ADR-610, the second rule).
    pub(crate) fn grid_top(&self) -> usize {
        if self.top_band { 0 } else { self.inset() }
    }

    /// The rows of a `w_px`×`h_px` window as pixels — THE LAYOUT RULE, in one place
    /// ([`RowMap`], ADR-610, refining ADR-508):
    ///
    ///   - **The inset is a margin on the LEFT, RIGHT and BOTTOM**, and on the top only
    ///     when row 0 is not a band ([`Renderer::grid_top`]).
    ///   - **Rows above the frame's seam are anchored to the top, rows from it down to the
    ///     bottom**: the last row ends exactly at `h_px - inset`.
    ///   - **The leftover height** (up to a cell less a pixel; `rows` is a floor division)
    ///     **sits at the seam**, painted by the row above it — or under the last row when
    ///     the frame declares no seam.
    ///
    /// Why a seam, and not the old rules: a top margin of the inset plus half the leftover
    /// changed with the window and put a strip of window above a header line; the leftover
    /// all under the last row (2026-11-05) moved an editor's echo area off the bottom edge
    /// by 0..cell_h-1 pixels as the window resized. Every terminal policy leaves the
    /// leftover beside a chrome band; the seam puts it inside the text, as Emacs does.
    ///
    /// WM-independent (no window resize, so it works where `request_inner_size` is
    /// ignored), and the mouse hit-test (`px_to_cell`) inverts the same map
    /// ([`RowMap::row_at`]) so clicks stay aligned with what's painted.
    pub(crate) fn rows(&self, w_px: usize, h_px: usize) -> RowMap {
        let (_, rows) = super::bleed::grid_cells(self, w_px, h_px);
        RowMap::window(
            self.grid_top(),
            self.cell_h,
            rows,
            self.inset(),
            h_px,
            self.seam,
        )
    }

    /// The seam this renderer paints with: the last frame's (`set_layout`).
    pub(crate) fn seam(&self) -> Option<u16> {
        self.seam
    }

    /// Take the layout a frame declares (`Op::Seam`, [`super::rows::frame_layout`]). Returns
    /// true when the GRID changed — row 0 became or stopped being a band, which moves the top
    /// margin and so the row count — so the caller re-publishes the window's size; a new seam
    /// alone moves pixels, not cells. Either change invalidates the retained frame.
    pub(crate) fn set_layout(&mut self, (seam, top_band): (Option<u16>, bool)) -> bool {
        let grid_changed = top_band != self.top_band;
        if seam != self.seam || grid_changed {
            self.seam = seam;
            self.top_band = top_band;
            self.invalidate();
        }
        grid_changed
    }

    /// Recompute the px size + cell metrics by shaping a reference glyph ('M') in
    /// the default family at the current size × HiDPI scale, dropping the cluster
    /// cache (baked at the old px). The grid stays uniform — a per-face
    /// `:family`/`:italic` only changes glyphs within the fixed cell.
    fn recompute(&mut self) {
        // Whole pixels per em: the rasteriser hints outlines to the pixel grid, and
        // hinting at a fractional ppem (15 px × a 1.25 HiDPI scale = 18.75) leaves stems
        // straddling pixels — soft, uneven text. Rounding keeps every glyph on the grid
        // the cell metrics already round to; the ≤0.5 px size error is invisible.
        self.px = (self.base_px * self.scale as f32).round().max(1.0);
        // The measured metrics depend on the family, the line height and the HiDPI scale —
        // all of which land here when they change, so this is where the memo is dropped.
        self.metrics_memo.borrow_mut().clear();
        // The cluster cache is NOT cleared: its key carries the px, so entries for the old
        // size stay valid and a zoom that returns to a size it has already drawn finds its
        // glyphs waiting. Clearing here is what made a continuous zoom re-rasterise every
        // glyph on every step. The retained FRAME is still stale — every cell moved — so
        // that is invalidated.
        self.invalidate();
        let (cw, ch, base) = self.metrics_for_px(self.px);
        self.cell_w = cw;
        self.cell_h = ch;
        self.baseline = base;
    }

    /// The cell metrics a given font size produces — `(cell_w, cell_h, baseline)` in physical
    /// px — by shaping the reference glyph at that size.
    ///
    /// Pure: it stores nothing. That is what lets ONE FRAME carry two sizes, which is what
    /// per-buffer zoom is: `Op::CellRegion` paints its ops with the metrics of its own px
    /// while the window keeps its own. `recompute` is this plus storing the result.
    pub(super) fn metrics_for_px(&self, px: f32) -> (usize, usize, i32) {
        let line_h = (px * self.line_height).round().max(1.0);
        // `name_of` returns owned data, so the immutable borrow ends on this
        // line — letting the `borrow_mut` below succeed (don't make it borrow).
        let fam = self.families.borrow().name_of(self.default_family);
        let mut shared = self.families.borrow_mut();
        let shared = &mut *shared;
        let metrics = Metrics::new(px, line_h);
        let mut tb = CtBuffer::new(&mut shared.fs, metrics);
        tb.set_size(Some(line_h * 4.0), Some(line_h * 2.0));
        let attrs = Attrs::new().family(Family::Name(fam.as_str()));
        tb.set_text("M", &attrs, Shaping::Advanced, None);
        tb.shape_until_scroll(&mut shared.fs, false);
        let (mut cw, mut base) = (px, px);
        if let Some(run) = tb.layout_runs().next() {
            base = run.line_y;
            if let Some(gl) = run.glyphs.first() {
                cw = gl.w;
            }
        }
        (
            cw.round().max(1.0) as usize,
            line_h as usize,
            base.round() as i32,
        )
    }

    /// The metrics currently in effect — what a scoped change restores.
    pub(crate) fn metrics(&self) -> CellMetrics {
        CellMetrics {
            px: self.px,
            cell_w: self.cell_w,
            cell_h: self.cell_h,
            baseline: self.baseline,
        }
    }

    /// The metrics a LOGICAL font size would produce (HiDPI applied here, and the px
    /// rounded to whole pixels for the same reason `recompute` rounds). Memoised: shaping
    /// the reference glyph is not free and a frame with per-buffer zoom asks for the same
    /// handful of sizes on every paint.
    pub(crate) fn metrics_at(&self, logical_px: f32) -> CellMetrics {
        // Clamped to a sane range before anything is shaped: the size comes from an app's
        // render op, and a 1e9-px cell would ask the shaper for a line box no machine has
        // the memory to lay out. NaN falls through `clamp` to the low end.
        let px = (logical_px.clamp(1.0, MAX_FONT_PX) * self.scale as f32)
            .round()
            .clamp(1.0, MAX_FONT_PX);
        let key = px.to_bits();
        if let Some(&(cell_w, cell_h, baseline)) = self.metrics_memo.borrow().get(&key) {
            return CellMetrics {
                px,
                cell_w,
                cell_h,
                baseline,
            };
        }
        let (cell_w, cell_h, baseline) = self.metrics_for_px(px);
        self.metrics_memo
            .borrow_mut()
            .insert(key, (cell_w, cell_h, baseline));
        CellMetrics {
            px,
            cell_w,
            cell_h,
            baseline,
        }
    }

    /// Install `metrics` for the ops that follow — the caller restores what `metrics()`
    /// gave it. Nothing else is touched: the cluster cache is keyed by px, so both sizes'
    /// glyphs coexist and neither region's work throws the other's away.
    pub(crate) fn set_metrics(&mut self, metrics: CellMetrics) {
        self.px = metrics.px;
        self.cell_w = metrics.cell_w;
        self.cell_h = metrics.cell_h;
        self.baseline = metrics.baseline;
    }

    /// Adjust for a new HiDPI scale factor (then recompute metrics).
    pub(super) fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        self.recompute();
    }

    /// The HiDPI scale factor (physical px per logical px).
    pub(crate) fn scale(&self) -> f64 {
        self.scale
    }

    /// Set the cell height as a multiple of the font px (behind `gui-line-height!`),
    /// then recompute the grid: the row count changes, so the caller re-derives
    /// `(cols, rows)` and re-renders — the same path as `set_font`.
    pub(super) fn set_line_height(&mut self, mult: f32) {
        let next = if mult.is_finite() {
            mult.clamp(0.8, 3.0)
        } else {
            LINE_HEIGHT
        };
        // Unchanged is free — see `set_font`. This is the one that bit: a caller writing
        // font and line height together re-rasterised the window twice per change.
        if next == self.line_height {
            return;
        }
        self.line_height = next;
        // Unlike a size change, this one moves glyphs without changing the px the cluster
        // cache is keyed by: a cluster is baked in its cell box at its baseline, so the
        // old line height's would sit wrong in the new cell.
        self.cache.clear();
        self.recompute();
    }

    /// Set how monochrome text is anti-aliased (behind `gui-text-aa!`). Drops the
    /// cluster cache (baked in the old mode) and the retained frame, so the next paint
    /// re-rasterises everything.
    pub(super) fn set_text_aa(&mut self, mode: TextAa) {
        self.text_aa = mode;
        self.cache.clear();
        self.invalidate();
    }

    /// Set the text contrast exponent (behind `gui-text-contrast!`): γ in 0.5..3.0, 1.0
    /// the plain sRGB lerp (`blend_text`) — i.e. no lift at all. Rebuilds the coverage
    /// curve and forgets the retained frame; the glyph cache holds raw coverage and is
    /// untouched.
    pub(super) fn set_text_contrast(&mut self, gamma: f32) {
        self.text_contrast = if gamma.is_finite() {
            gamma.clamp(0.5, 3.0)
        } else {
            1.0
        };
        *self.cov_lut = contrast_lut(self.text_contrast);
        self.invalidate();
    }

    /// A family was (re-)registered: the faces a keyword names may have changed, so drop
    /// every baked glyph, and re-measure the cell — when the window's own family is the
    /// one that moved, 'M' may be a different width now. (Before, re-registering the
    /// family a window already used kept the old font's cell: `set_font` with an
    /// unchanged keyword returns early, by design.)
    pub(super) fn fonts_changed(&mut self) {
        self.cache.clear();
        self.recompute();
    }

    /// The shared registration count — part of the GPU atlas key, which names a glyph
    /// by family KEYWORD and would otherwise keep a re-registered family's old glyphs.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn font_generation(&self) -> u64 {
        self.families.borrow().generation
    }

    /// Shape runs of punctuation as one unit (`on`), so a font's ligatures form, or
    /// every grapheme cluster alone (the default). Behind `gui-ligatures!`. The glyph
    /// cache needs no flush — a ligature unit is a different key from its clusters — but
    /// the retained frame was cut into the other units, so it is forgotten.
    pub(super) fn set_ligatures(&mut self, on: bool) {
        if self.ligatures != on {
            self.ligatures = on;
            self.invalidate();
        }
    }

    /// Whether a `:text` op is cut into ligature units (`text_units`).
    pub(crate) fn ligatures(&self) -> bool {
        self.ligatures
    }

    /// The cell height as a multiple of the font px (`gui-line-height!`) — part of the GPU
    /// fragment key, since a cell region inside a fragment derives its cell from it.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn line_height(&self) -> f32 {
        self.line_height
    }

    /// The subpixel order text is rasterised in right now: 0 none (grey), 1 R-G-B, 2 B-G-R —
    /// part of the GPU atlas key, since `gui-text-aa!` changes how the same cluster is baked.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn lcd_order(&self) -> u8 {
        match (self.subpixel_text(), self.text_aa) {
            (false, _) => 0,
            (true, TextAa::Bgr) => 2,
            (true, _) => 1,
        }
    }

    /// Whether text is rasterised per colour channel right now: the explicit mode, or
    /// under `Auto` only at a 1× scale (see `TextAa`).
    pub(super) fn subpixel_text(&self) -> bool {
        match self.text_aa {
            TextAa::Gray => false,
            TextAa::Subpixel | TextAa::Bgr => true,
            TextAa::Auto => (self.scale - 1.0).abs() < 1e-6,
        }
    }

    /// Forget the retained frame, so the next paint rasterises every row. For a
    /// metric change (font, inset, scale, line height) — `recompute` does it — and
    /// for anything else that changes how the same ops look (`set_bg`).
    pub(super) fn invalidate(&mut self) {
        self.prev_ops.clear();
    }

    /// Set the global default cell font — family and/or pixel size — then
    /// recompute the grid. The whole-window knob behind `gui-font!`.
    /// Set the default family and/or base px size (behind `gui-font`). A call that changes
    /// NEITHER returns without touching anything: `recompute` drops the whole glyph cache
    /// and the retained frame, so a redundant set costs a full re-rasterisation of the
    /// window — and a caller that writes its font settings together (an editor applying a
    /// size step, which also writes the line height) paid that twice per step for a value
    /// that had not moved.
    pub(super) fn set_font(&mut self, family: Option<u32>, px: Option<f32>) {
        let new_family = family.unwrap_or(self.default_family);
        let new_px = px.map(|p| p.max(1.0)).unwrap_or(self.base_px);
        if new_family == self.default_family && new_px == self.base_px {
            return;
        }
        self.default_family = new_family;
        self.base_px = new_px;
        self.recompute();
    }

    /// Shape + rasterise grapheme cluster `g` (cosmic-text + swash) into a small
    /// RGBA canvas sized to its cell span, cached by (cluster, family, style,
    /// scale). A color cluster (emoji) keeps its own colors; a monochrome cluster
    /// stores coverage in the alpha with white RGB, so the caller recolors it.
    fn build_cluster(
        &self,
        g: &str,
        fid: u32,
        bold: bool,
        italic: bool,
        scale: u16,
        subpixel: bool,
    ) -> CachedGlyph {
        let scale = scale.max(1) as usize;
        let px = (self.px * scale as f32).max(1.0);
        let cells = unit_cells(g).max(1);
        let cw = (self.cell_w * scale * cells).max(1);
        let ch = (self.cell_h * scale).max(1);
        let line_h = ch as f32;
        // The grid baseline (from the mono 'M', not this cluster's own line) so a
        // fallback glyph aligns with the surrounding text rather than floating to
        // wherever its own font's line metrics put it.
        let baseline = (self.baseline * scale as i32).max(0);
        let mut shared = self.families.borrow_mut();
        let shared = &mut *shared;
        let fam = shared.name_of(fid);
        // A glyph neither the family nor a bundled face has is drawn from an installed
        // font that has it, on the same baseline and in the same cell span (ADR-442).
        let fam = shared.fallback_family(g, &fam, bold, italic).unwrap_or(fam);
        // A ligature unit (several clusters, `text_units`) puts each cluster's glyph at
        // its own cell, not at the font's fractional pen position.
        let grid = is_ligature_unit(g).then_some((self.cell_w * scale) as i32);
        let attrs = |()| {
            let mut a = Attrs::new().family(Family::Name(fam.as_str()));
            if bold {
                a = a.weight(Weight::BOLD);
            }
            if italic {
                a = a.style(Style::Italic);
            }
            a
        };
        // Shape once at the text size to see whether the cluster fell back to a
        // *color* font (an emoji). Text/symbol glyphs are mono.
        let tb = shape_cluster(shared, g, attrs(()), px, cw as f32, line_h);
        let mut rgba = vec![0u8; cw * ch * 4];
        let (color, subpixel) = if first_glyph_is_color(shared, &tb) {
            // Emoji: render big enough to fill the cell block and center it — color
            // glyphs have no useful text baseline, so baseline-aligning them looks
            // low and cramped. Size to the smaller block dimension so the (square)
            // glyph fits its `cells`-wide span.
            let epx = ch.min(cw) as f32;
            let tb2 = shape_cluster(shared, g, attrs(()), epx, cw as f32, epx);
            composite_cluster(shared, &tb2, &mut rgba, cw, ch, Placement::Center);
            (true, false)
        } else if subpixel
            && composite_cluster_subpixel(
                shared,
                &tb,
                &mut rgba,
                cw,
                ch,
                baseline,
                grid,
                self.text_aa == TextAa::Bgr,
            )
        {
            (false, true)
        } else {
            // Gray AA — also the fallback when a glyph has no outline to render per
            // channel (a bitmap-only font), so the cluster still appears.
            rgba.fill(0);
            composite_cluster(
                shared,
                &tb,
                &mut rgba,
                cw,
                ch,
                Placement::Baseline { y: baseline, grid },
            );
            (false, false)
        };
        // A geometric mark (a status dot) sits in the middle of its cell, not where its
        // font's design puts it on the baseline (ADR-516).
        if !color && is_centred_symbol(g) {
            centre_ink_vertically(&mut rgba, cw, ch);
        }
        CachedGlyph {
            color,
            subpixel,
            width: cw,
            height: ch,
            rgba,
        }
    }

    /// Blit grapheme cluster `g` into the framebuffer at cell-pixel `(left, top)`,
    /// alpha-compositing over the cell background. A color cluster (emoji) draws in
    /// its own colors; a monochrome one is recolored with the face `fg`. The cluster
    /// occupies `string/display-width` cells (the caller advances the cursor to match).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_cluster(
        &mut self,
        canvas: &mut Canvas,
        left: usize,
        top: usize,
        g: &str,
        family: Option<u32>,
        bold: bool,
        italic: bool,
        scale: u16,
        fg: [u8; 3],
        clip_skip: usize,
    ) {
        if g == " " {
            return;
        }
        let fid = family.unwrap_or(self.default_family);
        let subpixel = self.subpixel_text();
        // The common single-char cluster keys via `ClusterKey::Char` with no
        // allocation; only a rare multi-char cluster allocates (a `Box<str>`).
        let key = (
            ClusterKey::of(g),
            fid,
            bold,
            italic,
            scale.max(1),
            subpixel,
            self.px.to_bits(),
        );
        PAINT_CLUSTERS.fetch_add(1, Ordering::Relaxed);
        if !self.cache.contains_key(&key) {
            let t0 = Instant::now();
            let baked = self.build_cluster(g, fid, bold, italic, scale, subpixel);
            PAINT_BUILD_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            PAINT_MISSES.fetch_add(1, Ordering::Relaxed);
            self.cache.insert(key.clone(), baked);
        }
        let cg = &self.cache[&key];
        // `clip_skip` glyph rows from the top are above the clip boundary (the grid
        // origin); they're skipped and the remaining rows land at `top` onward. The
        // canvas rect clips the rest: pixels outside `[x0, x1) × [y0, y1)` are not touched.
        let (fb_w, y0, y1) = (canvas.w, canvas.y0, canvas.y1);
        let columns = canvas.cols(left, cg.width);
        // Contrast lift (`text_contrast`): applied to a monochrome glyph where its
        // colour is lighter than the pixel it lands on, and never to a colour glyph
        // (emoji). A taste knob now, not a correction: the text lerp is in sRGB space
        // (`blend_text`), so γ = 1.0 already puts an edge pixel at the coverage it
        // represents.
        let lift = self.text_contrast != 1.0 && !cg.color;
        let lut = &self.cov_lut;
        let fg_lum = luma(fg);
        for ry in clip_skip..cg.height {
            let py = top + (ry - clip_skip);
            if py >= y1 {
                break;
            }
            if py < y0 {
                continue;
            }
            let row = py * fb_w;
            for pxx in columns.clone() {
                let rx = pxx - left;
                let i = (ry * cg.width + rx) * 4;
                let a = cg.rgba[i + 3];
                if a == 0 {
                    continue;
                }
                let dst = &mut canvas.buf[row + pxx];
                let lifted = lift && fg_lum > luma_packed(*dst);
                *dst = if cg.color {
                    blend(*dst, [cg.rgba[i], cg.rgba[i + 1], cg.rgba[i + 2]], a)
                } else if cg.subpixel {
                    let cov = [cg.rgba[i], cg.rgba[i + 1], cg.rgba[i + 2]];
                    let cov = if lifted {
                        [
                            lut[cov[0] as usize],
                            lut[cov[1] as usize],
                            lut[cov[2] as usize],
                        ]
                    } else {
                        cov
                    };
                    blend_text_rgb(*dst, fg, cov)
                } else {
                    blend_text(*dst, fg, if lifted { lut[a as usize] } else { a })
                };
            }
        }
    }
}

/// Source-over composite a straight-alpha pixel into the RGBA cluster canvas at
/// `(x, y)` (a no-op off-canvas / at zero alpha). Used to bake a shaped cluster's
/// glyphs into one canvas before it's cached.
#[allow(clippy::too_many_arguments)]
pub(super) fn canvas_over(
    rgba: &mut [u8],
    cw: usize,
    ch: usize,
    x: i32,
    y: i32,
    sr: u8,
    sg: u8,
    sb: u8,
    sa: u8,
) {
    if sa == 0 || x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 {
        return;
    }
    let idx = (y as usize * cw + x as usize) * 4;
    let (sa, da) = (sa as u32, rgba[idx + 3] as u32);
    let out_a = sa + da * (255 - sa) / 255;
    if out_a == 0 {
        return;
    }
    let mix = |s: u8, d: u8| (((s as u32 * sa) + (d as u32 * da * (255 - sa) / 255)) / out_a) as u8;
    rgba[idx] = mix(sr, rgba[idx]);
    rgba[idx + 1] = mix(sg, rgba[idx + 1]);
    rgba[idx + 2] = mix(sb, rgba[idx + 2]);
    rgba[idx + 3] = out_a as u8;
}

/// Whether cluster `g` is a geometric MARK rather than text: one character from Unicode's
/// Geometric Shapes block (`●` `■` `◆` `▲` `○` …) or the large circles and squares of
/// Miscellaneous Symbols and Arrows (`⬤` `⬛` `⭕`). These are drawn as status dots, bullets
/// and markers, which read as centred in their cell; their fonts place them on the baseline
/// at heights that differ from font to font (ADR-516). Box drawing and block elements are
/// not here: they fill their cell edge to edge already.
pub(super) fn is_centred_symbol(g: &str) -> bool {
    let mut chars = g.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    matches!(c, '\u{25A0}'..='\u{25FF}' | '\u{2B1B}'..='\u{2B2F}' | '\u{2B55}')
}

/// The first and last rows of a baked `w`×`h` RGBA canvas that carry ink, or `None` for a
/// blank one. Ink is any pixel with coverage, so a gray glyph (coverage in the alpha) and a
/// subpixel one (coverage per channel) are measured alike.
pub(super) fn ink_rows(rgba: &[u8], w: usize, h: usize) -> Option<(usize, usize)> {
    let stride = w * 4;
    let inked = |row: &usize| {
        rgba[row * stride..(row + 1) * stride]
            .chunks(4)
            .any(|p| p[3] > 0 && p[..3].iter().any(|c| *c > 0))
    };
    Some(((0..h).find(inked)?, (0..h).rev().find(inked)?))
}

/// Move the rows of a baked `w`×`h` RGBA canvas so its ink's top and bottom margins are
/// equal, the odd pixel below; a blank canvas is left as it is. The move is a rotation of
/// whole rows: the rows that wrap round are ones the ink leaves blank, since it moves by at
/// most the margin on the side it moves into.
fn centre_ink_vertically(rgba: &mut [u8], w: usize, h: usize) {
    let Some((first, last)) = ink_rows(rgba, w, h) else {
        return;
    };
    let target = (first + (h - 1 - last)) / 2;
    let stride = w * 4;
    if target < first {
        rgba.rotate_left((first - target) * stride);
    } else {
        rgba.rotate_right((target - first) * stride);
    }
}

/// Where a cluster's glyphs sit in its baked canvas. `Baseline { y, .. }` puts the text
/// baseline at row `y` (the shared grid baseline, so a fallback symbol aligns with
/// the surrounding text); with `grid: Some(cell_w)` each glyph is moved onto the cell of
/// the char it starts at (a ligature unit, `grid_shift`). `Center` ignores the baseline
/// and centers the glyph's bounding box in the canvas — for color emoji, which have no
/// useful text baseline.
pub(super) enum Placement {
    Baseline { y: i32, grid: Option<i32> },
    Center,
}

/// How far to move glyph `gl` of a ligature unit so it starts on its char's cell rather
/// than at the font's pen position: a unit is ASCII, one cell per byte, so the char at
/// byte `start` owns cell `start`. The font's advance is fractional and the cell is
/// rounded, so without this the pen drifts off the grid by up to half a pixel per char.
/// Zero when `grid` is `None` (a single cluster, placed exactly as before).
fn grid_shift(gl: &cosmic_text::LayoutGlyph, grid: Option<i32>) -> i32 {
    match grid {
        Some(cell_w) => gl.start as i32 * cell_w - gl.x.round() as i32,
        None => 0,
    }
}

/// The ASCII punctuation programming ligatures are made of (`->`, `!=`, `<=>`, `|>`,
/// `::`, `/*`, `&&`, `===`). Letters and digits are left out on purpose: a run of them is
/// a word, and caching every word as a unit would grow the glyph cache per identifier.
fn ligature_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'/'
            | b':'
            | b';'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'\\'
            | b'^'
            | b'_'
            | b'|'
            | b'~'
    )
}

/// The longest run shaped as one unit; a longer run (a `=====` rule) is cut into units
/// this long, so the glyph cache stays bounded by what the font can ligate.
const MAX_LIGATURE_RUN: usize = 8;

/// Whether `unit` is a ligature unit — two or more ligature bytes — rather than one
/// grapheme cluster. Unambiguous: every ASCII punctuation char is a cluster of its own.
pub(crate) fn is_ligature_unit(unit: &str) -> bool {
    unit.len() > 1 && unit.bytes().all(ligature_byte)
}

/// The cells a unit of `text_units` spans: one per byte for a ligature unit, the
/// cluster's own width otherwise.
pub(crate) fn unit_cells(unit: &str) -> usize {
    if is_ligature_unit(unit) {
        unit.len()
    } else {
        cluster_cells(unit)
    }
}

/// The cells a block caret over `text` covers (ADR-515): the text's width, at least one, so
/// the caret over a wide character covers both its cells.
pub(crate) fn caret_cells(text: &str) -> usize {
    unit_cells(text).max(1)
}

/// What a block caret at `(row, col)` draws over its solid fill (ADR-515): the text under it
/// as an ordinary text op, in its own face coloured `ground`, the window background
/// ([`Face::under_caret`]). Both painters draw the fill, `caret_cells` wide, then this.
pub(crate) fn caret_glyph(
    row: u16,
    col: u16,
    (text, face): &(String, crate::host::gui::Face),
    ground: [u8; 3],
) -> Op {
    Op::Text {
        row,
        col,
        s: text.clone(),
        face: face.under_caret(ground),
    }
}

/// The units a `:text` op is drawn in: its grapheme clusters, or with `ligatures` on,
/// its clusters with every run of two or more punctuation chars (`ligature_byte`) taken
/// as ONE unit, so the run is shaped together and the font's ligatures form. Each unit
/// is rasterised and cached as one canvas `unit_cells` wide, so a ligature spanning N
/// cells occupies exactly those N cells and the grid never moves.
pub(crate) fn text_units(text: &str, ligatures: bool) -> TextUnits<'_> {
    if ligatures {
        TextUnits::Ligated { text, at: 0 }
    } else {
        TextUnits::Clusters(text.graphemes(true))
    }
}

/// The iterator `text_units` returns.
pub(crate) enum TextUnits<'a> {
    Clusters(unicode_segmentation::Graphemes<'a>),
    Ligated { text: &'a str, at: usize },
}

impl<'a> Iterator for TextUnits<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        match self {
            TextUnits::Clusters(clusters) => clusters.next(),
            TextUnits::Ligated { text, at } => {
                let rest = &text[*at..];
                if rest.is_empty() {
                    return None;
                }
                let mut run = rest
                    .bytes()
                    .take_while(|byte| ligature_byte(*byte))
                    .count()
                    .min(MAX_LIGATURE_RUN);
                // A combining mark after the run belongs to its last char's cluster.
                if run >= 2 && rest[run - 1..].graphemes(true).next().map(str::len) != Some(1) {
                    run -= 1;
                }
                let unit = if run >= 2 {
                    &rest[..run]
                } else {
                    rest.graphemes(true).next().unwrap_or(rest)
                };
                *at += unit.len();
                Some(unit)
            }
        }
    }
}

/// Shape grapheme cluster `g` into a fresh cosmic-text buffer at `px` / line height
/// `line_h`, in family/style `attrs`. The layout box is generous so a wide glyph
/// isn't wrapped or clipped during shaping.
pub(super) fn shape_cluster(
    shared: &mut FontShared,
    g: &str,
    attrs: Attrs,
    px: f32,
    w: f32,
    line_h: f32,
) -> CtBuffer {
    let mut tb = CtBuffer::new(&mut shared.fs, Metrics::new(px.max(1.0), line_h.max(1.0)));
    tb.set_size(Some(w + px), Some(line_h + px));
    tb.set_text(g, &attrs, Shaping::Advanced, None);
    tb.shape_until_scroll(&mut shared.fs, false);
    tb
}

/// True if the cluster's first rasterised glyph is a *color* (emoji) bitmap — the
/// signal to size + center it rather than baseline-align it as text.
pub(super) fn first_glyph_is_color(shared: &mut FontShared, tb: &CtBuffer) -> bool {
    for run in tb.layout_runs() {
        for gl in run.glyphs.iter() {
            let phys = gl.physical((0.0, 0.0), 1.0);
            if let Some(img) = shared.swash.get_image(&mut shared.fs, phys.cache_key) {
                return matches!(img.content, SwashContent::Color);
            }
        }
    }
    false
}

/// Composite a shaped cluster's glyphs into the RGBA canvas `rgba` (`cw`×`ch`).
/// `Baseline` lays each glyph at its pen position on the shared baseline (text);
/// `Center` puts the glyph's bounding box in the middle of the canvas (emoji).
/// Color glyphs keep their own RGBA; mask glyphs store coverage as white + alpha
/// (the caller recolors them with the face fg).
pub(super) fn composite_cluster(
    shared: &mut FontShared,
    tb: &CtBuffer,
    rgba: &mut [u8],
    cw: usize,
    ch: usize,
    place: Placement,
) {
    for run in tb.layout_runs() {
        for gl in run.glyphs.iter() {
            let phys = gl.physical((0.0, 0.0), 1.0);
            let img = match shared.swash.get_image(&mut shared.fs, phys.cache_key) {
                Some(img) => img,
                None => continue,
            };
            let (iw, ih) = (img.placement.width as i32, img.placement.height as i32);
            let (ox, oy) = match place {
                Placement::Baseline { y, grid } => (
                    phys.x + img.placement.left + grid_shift(gl, grid),
                    y + phys.y - img.placement.top,
                ),
                Placement::Center => ((cw as i32 - iw) / 2, (ch as i32 - ih) / 2),
            };
            match img.content {
                SwashContent::Mask => {
                    for ry in 0..ih {
                        for rx in 0..iw {
                            let a = img.data[(ry * iw + rx) as usize];
                            canvas_over(rgba, cw, ch, ox + rx, oy + ry, 255, 255, 255, a);
                        }
                    }
                }
                SwashContent::Color => {
                    for ry in 0..ih {
                        for rx in 0..iw {
                            let i = ((ry * iw + rx) * 4) as usize;
                            canvas_over(
                                rgba,
                                cw,
                                ch,
                                ox + rx,
                                oy + ry,
                                img.data[i],
                                img.data[i + 1],
                                img.data[i + 2],
                                img.data[i + 3],
                            );
                        }
                    }
                }
                SwashContent::SubpixelMask => {}
            }
        }
    }
}

/// Composite a shaped cluster's glyphs into `rgba` as a **subpixel** mask: R/G/B carry
/// the coverage of each channel's third of the pixel (rasterised at 3× horizontal
/// resolution and LCD-filtered, `lcd_filter`, hinted like the gray path), A is the max
/// of the three so the blit's
/// "anything here?" test still works. `bgr` flips the channel order for a panel whose
/// subpixels run blue-first. Baseline-placed like text. Returns false — leaving `rgba`
/// untouched — if no glyph had an outline to render this way (a bitmap-only face),
/// so the caller can fall back to the gray mask.
pub(super) fn composite_cluster_subpixel(
    shared: &mut FontShared,
    tb: &CtBuffer,
    rgba: &mut [u8],
    cw: usize,
    ch: usize,
    baseline: i32,
    grid: Option<i32>,
    bgr: bool,
) -> bool {
    use cosmic_text::CacheKeyFlags;
    use swash::scale::{Render, Source};
    use swash::zeno::{Angle, Format, Transform, Vector};
    let mut drew = false;
    for run in tb.layout_runs() {
        for gl in run.glyphs.iter() {
            let phys = gl.physical((0.0, 0.0), 1.0);
            let key = phys.cache_key;
            let Some(font) = shared.fs.get_font(key.font_id, key.font_weight) else {
                continue;
            };
            // The same scaler settings cosmic-text's own (alpha) rasteriser uses for
            // this cache key — hinting, a synthesised italic for a family without an
            // italic face, whole-pixel offsets for a pixel font — so the subpixel glyph
            // is the gray glyph with three coverages, not a differently shaped one.
            let flags = key.flags;
            let mut scaler = shared
                .scaler
                .builder(font.as_swash())
                .size(f32::from_bits(key.font_size_bits))
                .hint(!flags.contains(CacheKeyFlags::DISABLE_HINTING))
                .build();
            let offset = if flags.contains(CacheKeyFlags::PIXEL_FONT) {
                Vector::new(key.x_bin.as_float().round(), key.y_bin.as_float().round())
            } else {
                Vector::new(key.x_bin.as_float(), key.y_bin.as_float())
            };
            // The outline is rasterised at THREE TIMES the horizontal resolution, one
            // coverage per subpixel, as FreeType does for LCD text — not as three
            // whole-pixel renders shifted a third of a pixel apart, which is what swash's
            // own `Format::Subpixel` is, and which leaves every stem edge a hard
            // orange or blue fringe. `lcd_filter` then spreads each subpixel over its
            // neighbours.
            let triple = Transform::scale(LCD_OVERSAMPLE as f32, 1.0);
            let transform = if flags.contains(CacheKeyFlags::FAKE_ITALIC) {
                Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)).then(&triple)
            } else {
                triple
            };
            let Some(img) = Render::new(&[Source::Outline])
                .format(Format::Alpha)
                .offset(Vector::new(offset.x * LCD_OVERSAMPLE as f32, offset.y))
                .transform(Some(transform))
                .render(&mut scaler, key.glyph_id)
            else {
                continue;
            };
            if img.data.len() < (img.placement.width * img.placement.height) as usize {
                continue;
            }
            let mask = lcd_filter(
                &img.data,
                img.placement.left,
                img.placement.width as usize,
                img.placement.height as usize,
                bgr,
            );
            let (iw, ih) = (mask.width as i32, img.placement.height as i32);
            let (ox, oy) = (
                phys.x + mask.left + grid_shift(gl, grid),
                baseline + phys.y - img.placement.top,
            );
            for ry in 0..ih {
                for rx in 0..iw {
                    let i = ((ry * iw + rx) * 3) as usize;
                    let (r, g, b) = (mask.rgb[i], mask.rgb[i + 1], mask.rgb[i + 2]);
                    let a = r.max(g).max(b);
                    if a == 0 {
                        continue;
                    }
                    let (x, y) = (ox + rx, oy + ry);
                    if x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 {
                        continue;
                    }
                    let idx = (y as usize * cw + x as usize) * 4;
                    // Per-channel union with what's there (two glyphs of one cluster
                    // may overlap — a base + combining mark): the max coverage wins.
                    rgba[idx] = rgba[idx].max(r);
                    rgba[idx + 1] = rgba[idx + 1].max(g);
                    rgba[idx + 2] = rgba[idx + 2].max(b);
                    rgba[idx + 3] = rgba[idx + 3].max(a);
                    drew = true;
                }
            }
        }
    }
    drew
}

/// Subpixels per pixel across a row: an LCD pixel is three stripes, R G B.
const LCD_OVERSAMPLE: i32 = 3;

/// FreeType's default LCD filter (`FT_LCD_FILTER_DEFAULT`), in 256ths: each subpixel's
/// coverage spread over itself and two neighbours each side. Unfiltered, a stem edge
/// that covers one stripe of a pixel lights that stripe alone, a saturated orange or
/// blue fringe; filtered, the energy is shared with the stripes beside it and the edge
/// reads as a lighter, near-neutral one. The weights sum to 256, so a solid run stays
/// solid.
const LCD_FILTER: [u32; 5] = [0x08, 0x4D, 0x56, 0x4D, 0x08];

/// A glyph's per-channel coverage at whole pixels: `width` × the image's rows, three
/// bytes a pixel (R G B in panel order), its left edge `left` pixels from the pen.
pub(super) struct LcdMask {
    pub(super) left: i32,
    pub(super) width: usize,
    pub(super) rgb: Vec<u8>,
}

/// Fold a coverage image rendered at [`LCD_OVERSAMPLE`]× horizontal resolution (`w3`
/// subpixels wide, its first `left3` subpixels from the pen) into an [`LcdMask`]:
/// filter each row with [`LCD_FILTER`], then give each channel its own stripe of the
/// pixel — the left stripe to red on an R-G-B panel, to blue on a B-G-R one (`bgr`).
/// The filter reaches two subpixels past the image on each side, so the mask can be a
/// pixel wider than the image.
pub(super) fn lcd_filter(alpha: &[u8], left3: i32, w3: usize, h: usize, bgr: bool) -> LcdMask {
    let reach = (LCD_FILTER.len() / 2) as i32;
    let first = (left3 - reach).div_euclid(LCD_OVERSAMPLE);
    let last = (left3 + w3 as i32 - 1 + reach).div_euclid(LCD_OVERSAMPLE);
    let width = (last - first + 1).max(0) as usize;
    let mut rgb = vec![0u8; width * h * 3];
    for row in 0..h {
        let line = &alpha[row * w3..(row + 1) * w3];
        let at = |sub: i32| -> u32 {
            let i = sub - left3;
            if i < 0 || i >= w3 as i32 {
                0
            } else {
                line[i as usize] as u32
            }
        };
        for p in 0..width {
            for stripe in 0..LCD_OVERSAMPLE {
                let sub = (first + p as i32) * LCD_OVERSAMPLE + stripe;
                let sum: u32 = LCD_FILTER
                    .iter()
                    .enumerate()
                    .map(|(k, w)| w * at(sub + k as i32 - reach))
                    .sum();
                let channel = if bgr { 2 - stripe } else { stripe } as usize;
                rgb[(row * width + p) * 3 + channel] = ((sum + 128) >> 8).min(255) as u8;
            }
        }
    }
    LcdMask {
        left: first,
        width,
        rgb,
    }
}

pub(super) fn pack(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32
}

/// linear-light (0..=1) → sRGB byte — the exact inverse of [`SRGB_TO_LINEAR`].
#[inline]
pub(super) fn linear_to_srgb(c: f32) -> u32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u32
}

/// Alpha-composite `fg` over destination pixel `dst` with coverage `cov` (0..=255),
/// **in linear light** so anti-aliased edges are weighted correctly. The rasteriser's
/// coverage is linear, but a naive sRGB-space lerp under-weights partial-coverage
/// pixels — on a dark theme (light text on `#1e1e2e`) that makes stems look thin and
/// the text fuzzy. Decoding to linear, blending, then re-encoding keeps strokes full
/// and edges crisp. The glyph interior (`cov == 255`) and exterior (`cov == 0`) — the
/// bulk of the pixels — take an exact fast path with no float work, so only the thin
/// anti-aliased rim pays the gamma cost.
pub(super) fn blend(dst: u32, fg: [u8; 3], cov: u8) -> u32 {
    if cov == 0 {
        return dst;
    }
    if cov == 255 {
        return pack(fg);
    }
    let a = cov as f32 / 255.0;
    let lut = &*SRGB_TO_LINEAR;
    let dr = lut[((dst >> 16) & 0xff) as usize];
    let dg = lut[((dst >> 8) & 0xff) as usize];
    let db = lut[(dst & 0xff) as usize];
    let r = linear_to_srgb(lut[fg[0] as usize] * a + dr * (1.0 - a));
    let g = linear_to_srgb(lut[fg[1] as usize] * a + dg * (1.0 - a));
    let b = linear_to_srgb(lut[fg[2] as usize] * a + db * (1.0 - a));
    (r << 16) | (g << 8) | b
}

/// Composite `fg` over `dst` at coverage `cov` **in sRGB space** — a plain per-channel
/// lerp of the encoded values, no linear round trip. This is the TEXT path, and it is
/// deliberately not what `blend` above does.
///
/// A rasteriser's coverage is not a light quantity, it is the fraction of the pixel the
/// outline covers, and every font stack that text is tuned against (FreeType/cairo/Xft,
/// and so Emacs) composites it in the encoded space. Blending it in linear light instead
/// makes a partly covered pixel far brighter than the fraction it represents: white on
/// black at half coverage lands at 188/255 rather than 128, and at quarter coverage 137
/// rather than 64. Every antialiased rim then glows, which reads as soft, puffy text —
/// and it is why this renderer grew a contrast knob whose useful setting was BELOW 1.0
/// (γ=0.5 squares the coverage, which is roughly the inverse of the sRGB encode: it was
/// cancelling this blend). With the lerp in sRGB, γ = 1.0 is the honest default again.
///
/// Geometry keeps `blend`: a rounded rect's edge coverage and the cursor's translucent
/// overlay ARE light quantities, and linear light is right for them.
pub(super) fn blend_text(dst: u32, fg: [u8; 3], cov: u8) -> u32 {
    if cov == 0 {
        return dst;
    }
    if cov == 255 {
        return pack(fg);
    }
    let a = cov as u32;
    let inv = 255 - a;
    // +127 rounds to nearest rather than truncating, so a symmetric pair of coverages
    // composites symmetrically.
    let ch =
        |shift: u32, f: u8| -> u32 { (((dst >> shift) & 0xff) * inv + f as u32 * a + 127) / 255 };
    (ch(16, fg[0]) << 16) | (ch(8, fg[1]) << 8) | ch(0, fg[2])
}

/// The subpixel sibling of `blend_text`: composite `fg` over `dst` with a separate
/// coverage per colour channel (`cov[0]` red, `[1]` green, `[2]` blue), in sRGB space.
/// Each channel is the same single-channel lerp `blend_text` does, so the two stay
/// consistent where a gray and a subpixel glyph sit side by side.
pub(super) fn blend_text_rgb(dst: u32, fg: [u8; 3], cov: [u8; 3]) -> u32 {
    if cov == [0, 0, 0] {
        return dst;
    }
    if cov == [255, 255, 255] {
        return pack(fg);
    }
    let ch = |shift: u32, c: u8, f: u8| -> u32 {
        let a = c as u32;
        (((dst >> shift) & 0xff) * (255 - a) + f as u32 * a + 127) / 255
    };
    (ch(16, cov[0], fg[0]) << 16) | (ch(8, cov[1], fg[1]) << 8) | ch(0, cov[2], fg[2])
}

pub(super) fn fill_cell(
    canvas: &mut Canvas,
    left: usize,
    top: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    let fb_w = canvas.w;
    let columns = canvas.cols(left, w);
    if columns.is_empty() {
        return;
    }
    for y in canvas.rows(top, h) {
        let row = y * fb_w;
        canvas.buf[row + columns.start..row + columns.end].fill(color);
    }
}

/// Fill a sub-pixel-positioned rounded rectangle with `color` at `opacity` (0..1),
/// anti-aliased. The shape is a signed-distance field: the distance from each pixel
/// centre to the rect's rounded core (the rect inset by `radius` on every side)
/// gives a 1px coverage ramp at the edge, so corners and fractional edges read
/// smooth instead of stair-stepped. `radius == 0` is a sharp rect (full coverage
/// inside, just the opacity blend). Blends over whatever's already in `buf`
/// (`blend`), so a faded overlay shows the content beneath it. A dimension thinner than a
/// logical pixel is a hairline and snaps to whole device pixels first (`snap_hairline`).
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_rrect(
    canvas: &mut Canvas,
    fx: f32,
    fy: f32,
    fw: f32,
    fh: f32,
    radius: f32,
    color: [u8; 3],
    opacity: f32,
    scale: f32,
) {
    if fw <= 0.0 || fh <= 0.0 || opacity <= 0.0 {
        return;
    }
    // A hairline — a dimension thinner than one LOGICAL pixel — snaps to a whole
    // number of device pixels (one per logical pixel, so 2 px at 2×) centred where it
    // was asked for. Otherwise a 0.45 px rule lands on 1 or 2 solid pixels depending on
    // which pixel boundary its fractional position straddles: uneven, and different
    // for every divider. Snapped, every hairline in a window is the same crisp line.
    let (fx, fw, fy, fh, radius) = snap_hairline(fx, fw, fy, fh, radius, scale);
    let (fb_w, fb_h) = (canvas.w, canvas.h);
    let base = opacity.clamp(0.0, 1.0) * 255.0;
    let r = radius.max(0.0).min(fw / 2.0).min(fh / 2.0);
    // The inner core the corners round around: the rect inset by `r` on each side.
    let (rx0, ry0) = (fx + r, fy + r);
    let (rx1, ry1) = (fx + fw - r, fy + fh - r);
    let x0 = (fx.floor().max(0.0) as usize).max(canvas.x0);
    let y0 = (fy.floor().max(0.0) as usize).max(canvas.y0);
    let x1 = ((fx + fw).ceil().max(0.0) as usize)
        .min(fb_w)
        .min(canvas.x1);
    let y1 = ((fy + fh).ceil().max(0.0) as usize)
        .min(fb_h)
        .min(canvas.y1);
    for py in y0..y1 {
        let cy = py as f32 + 0.5;
        let row = py * fb_w;
        for px in x0..x1 {
            let cx = px as f32 + 0.5;
            let dx = (rx0 - cx).max(cx - rx1).max(0.0);
            let dy = (ry0 - cy).max(cy - ry1).max(0.0);
            // Inside the straight body dx==dy==0 → full coverage; in a corner the
            // euclidean distance past the core gives the rounded falloff.
            let cov_f = if r <= 0.0 {
                1.0
            } else {
                (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
            };
            if cov_f <= 0.0 {
                continue;
            }
            let cov = (base * cov_f) as u32;
            if cov == 0 {
                continue;
            }
            let idx = row + px;
            canvas.buf[idx] = blend(canvas.buf[idx], color, cov.min(255) as u8);
        }
    }
}

/// The hairline rule behind `fill_rrect`: a rect dimension under one logical pixel
/// (`scale` device px) becomes exactly `max(1, round(scale))` device pixels, placed so
/// the requested centre is inside them; the snapped line is square-cornered (a
/// radius on a 1 px line only fades it). Both axes are checked independently, so a
/// thin vertical and a thin horizontal rule both snap. Returns the (possibly)
/// adjusted `(x, w, y, h, radius)`; a rect that isn't a hairline passes through.
pub(crate) fn snap_hairline(
    fx: f32,
    fw: f32,
    fy: f32,
    fh: f32,
    radius: f32,
    scale: f32,
) -> (f32, f32, f32, f32, f32) {
    let logical = scale.max(1.0);
    let hair = logical.round().max(1.0);
    let snap = |pos: f32, len: f32| -> (f32, f32) {
        let centre = pos + len / 2.0;
        ((centre - hair / 2.0).round(), hair)
    };
    let thin_w = fw < logical;
    let thin_h = fh < logical;
    if !thin_w && !thin_h {
        return (fx, fw, fy, fh, radius);
    }
    let (fx, fw) = if thin_w { snap(fx, fw) } else { (fx, fw) };
    let (fy, fh) = if thin_h { snap(fy, fh) } else { (fy, fh) };
    (fx, fw, fy, fh, 0.0)
}

/// Draw the text cursor at a cell per its `style`:
///   * `Block` — overlay 50% white on the whole cell, so the glyph under it
///     stays faintly visible (the terminal-style caret), plus a crisp solid
///     rim around the cell so the cursor reads unambiguously against any
///     background (a translucent fill alone sinks into a busy highlight —
///     e.g. the bracket-match block — and the eye loses which cell owns it);
///   * `Hollow` — the rim alone, thinner: an outline box, the cursor of a window
///     without focus (Emacs's and kitty's unfocused caret);
///   * `Bar` — a thin, solid vertical line on the cell's left edge (a modern
///     GUI insertion caret) that doesn't obscure the glyph;
///   * `Underline` — a thin solid rule along the cell bottom.
/// `color` is the op's caret colour: the block's wash and every solid stroke take
/// it; `None` keeps the defaults (a white wash, `CURSOR_FG` strokes).
/// The bar/underline thickness scales with the cell so it stays proportional on
/// HiDPI (≥2 physical px). They paint solid rather than a blend, so a 2px caret
/// reads crisply.
/// `draw_top`/`draw_bottom` are already clamped to the visible region `[oy, fb_h)`.
/// `cell_top` is the logical (unclipped) top of the cursor cell in framebuffer pixels;
/// it may be negative or above `oy` when the cursor is partially scrolled off the top.
/// Used for the rim's horizontal edges and the underline, which sit on the logical cell.
#[allow(clippy::too_many_arguments)]
pub(super) fn cursor_cell(
    canvas: &mut Canvas,
    left: usize,
    draw_top: usize,
    draw_bottom: usize,
    w: usize,
    h: usize,
    cell_top: isize,
    style: crate::host::gui::CursorStyle,
    color: Option<[u8; 3]>,
) {
    use crate::host::gui::CursorStyle;
    // The band clips the visible slice further; everything below derives from it.
    let draw_top = draw_top.max(canvas.y0);
    let draw_bottom = draw_bottom.min(canvas.y1);
    if draw_top >= draw_bottom {
        return;
    }
    let stroke = pack(color.unwrap_or(CURSOR_FG));
    let fb_w = canvas.w;
    // Every span below is computed on the unclipped cell, then cut to the canvas rect, so a
    // rect that holds half the caret paints exactly that half.
    let (clip_x0, clip_x1) = (canvas.x0, canvas.x1);
    let clip = |from: usize, to: usize| from.max(clip_x0)..to.min(clip_x1);
    match style {
        CursorStyle::Block | CursorStyle::Hollow => {
            let buf = &mut *canvas.buf;
            if style == CursorStyle::Block {
                let wash = color.unwrap_or([0xff, 0xff, 0xff]);
                for y in draw_top..draw_bottom {
                    let row = y * fb_w;
                    for x in clip(left, (left + w).min(fb_w)) {
                        buf[row + x] = blend(buf[row + x], wash, 128);
                    }
                }
            }
            // The rim, scaled like the bar caret so it stays proportional on HiDPI.
            // Vertical edges span the visible slice; horizontal edges sit at the
            // LOGICAL cell top/bottom (cell_top may be off-screen when the cursor is
            // partially scrolled off).
            let t = cursor_rim(w, style);
            let x0 = left;
            let x1 = (left + w).min(fb_w);
            for y in draw_top..draw_bottom {
                let row = y * fb_w;
                for x in clip(x0, (x0 + t).min(fb_w)) {
                    buf[row + x] = stroke;
                }
                for x in clip(x1.saturating_sub(t), x1) {
                    buf[row + x] = stroke;
                }
            }
            let edges = [
                (cell_top, cell_top + t as isize),
                (cell_top + h as isize - t as isize, cell_top + h as isize),
            ];
            for (ey0, ey1) in edges {
                let y0 = ey0.max(draw_top as isize).max(0) as usize;
                let y1 = ey1.min(draw_bottom as isize).max(0) as usize;
                for y in y0..y1 {
                    let row = y * fb_w;
                    for x in clip(x0, x1) {
                        buf[row + x] = stroke;
                    }
                }
            }
        }
        CursorStyle::Bar => {
            let thickness = (w / 8).max(2);
            fill_cell(
                canvas,
                left,
                draw_top,
                thickness,
                draw_bottom - draw_top,
                stroke,
            );
        }
        CursorStyle::Underline => {
            let thickness = (h / 10).max(2);
            // Underline sits at the bottom of the logical cell regardless of clipping.
            let uy = (cell_top + h as isize).saturating_sub(thickness as isize);
            if uy >= 0 {
                let uy = uy as usize;
                if uy >= draw_top && uy < draw_bottom {
                    let visible = thickness.min(draw_bottom - uy);
                    fill_cell(canvas, left, uy, w, visible, stroke);
                }
            }
        }
    }
}

/// The rim thickness of a block or hollow cursor `w` px wide: a tenth of the cell
/// (never under 2 px) for a block, a twelfth (never under 1 px) for the hollow box,
/// which is an outline and reads as one only when thinner than the focused caret.
/// Shared with the GPU target (`gpu::cursor_quads`), so both paint the same box.
pub(crate) fn cursor_rim(w: usize, style: crate::host::gui::CursorStyle) -> usize {
    match style {
        crate::host::gui::CursorStyle::Hollow => (w / 12).max(1),
        _ => (w / 10).max(2),
    }
}

#[cfg(test)]
mod text_aa_tests {
    use super::*;

    /// A subpixel-rendered cluster carries three coverages per pixel that differ from
    /// one another somewhere along a stem's edge — the whole point of LCD text — and
    /// the gray path renders the same glyph as one coverage in the alpha.
    #[test]
    fn subpixel_text_renders_per_channel_coverage() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        r.set_text_aa(TextAa::Subpixel);
        let sub = r.build_cluster("l", r.default_family, false, false, 1, true);
        assert!(sub.subpixel, "the subpixel path fell back to gray");
        let fringe = sub
            .rgba
            .chunks(4)
            .any(|p| p[3] > 0 && (p[0] != p[1] || p[1] != p[2]));
        assert!(fringe, "no per-channel coverage differences at all");
        let gray = r.build_cluster("l", r.default_family, false, false, 1, false);
        assert!(!gray.subpixel);
        assert!(gray.rgba.chunks(4).any(|p| p[3] > 0));
    }

    /// A subpixel glyph carries the same ink as its gray twin: the 3× raster and the
    /// filter move coverage between stripes, never add or lose it. A transform applied
    /// the wrong way round (offset scaled twice, the outline left at 1×) lands the glyph
    /// a third as wide or in the wrong place, and the ink no longer matches.
    #[test]
    fn subpixel_text_keeps_the_gray_glyphs_ink() {
        let mut r = Renderer::new(1.0, default_families(), 16.0);
        r.set_text_aa(TextAa::Subpixel);
        let ink = |rgba: &[u8], channels: &[usize]| -> f64 {
            rgba.chunks(4)
                .map(|p| channels.iter().map(|&c| p[c] as f64).sum::<f64>())
                .sum::<f64>()
                / channels.len() as f64
        };
        for glyph in ["l", "o", "W"] {
            let sub = r.build_cluster(glyph, r.default_family, false, false, 1, true);
            let gray = r.build_cluster(glyph, r.default_family, false, false, 1, false);
            assert!(sub.subpixel, "{glyph}: the subpixel path fell back to gray");
            let (lcd, alpha) = (ink(&sub.rgba, &[0, 1, 2]), ink(&gray.rgba, &[3]));
            let ratio = lcd / alpha;
            assert!(
                (0.85..1.15).contains(&ratio),
                "{glyph}: subpixel ink {lcd} vs gray {alpha}"
            );
        }
    }

    /// A solid run of subpixels stays solid through the filter: the weights sum to one.
    #[test]
    fn the_lcd_filter_keeps_a_solid_run_solid() {
        let mask = lcd_filter(&[255; 12], 0, 12, 1, false);
        // pixels 1 and 2 are wholly inside the run, two subpixels from either end
        assert_eq!(mask.left, -1);
        assert_eq!(&mask.rgb[2 * 3..3 * 3], &[255, 255, 255]);
    }

    /// One lit stripe — a stem edge covering the green third of a pixel — is shared with
    /// its neighbours, so the pixel reads near neutral instead of a saturated stripe, and
    /// the stripes two away get the filter's tail.
    #[test]
    fn the_lcd_filter_spreads_one_stripe_over_its_neighbours() {
        // subpixel 4 is pixel 1's green stripe
        let mask = lcd_filter(&[255], 4, 1, 1, false);
        assert_eq!(mask.left, 0);
        assert_eq!(&mask.rgb[3..6], &[77, 86, 77], "pixel 1: R G B");
        assert_eq!(mask.rgb[2], 8, "pixel 0's blue, two stripes left");
        assert_eq!(mask.rgb[6], 8, "pixel 2's red, two stripes right");
    }

    /// On a B-G-R panel the pixel's LEFT stripe is blue.
    #[test]
    fn the_lcd_filter_puts_the_left_stripe_in_blue_on_a_bgr_panel() {
        // subpixel 3 is pixel 1's left stripe
        let rgb = lcd_filter(&[255], 3, 1, 1, false);
        let bgr = lcd_filter(&[255], 3, 1, 1, true);
        assert_eq!(&rgb.rgb[3..6], &[86, 77, 8]);
        assert_eq!(&bgr.rgb[3..6], &[8, 77, 86]);
    }
}

/// The coverage curve for a text contrast exponent `gamma`: `cov -> 255·(cov/255)^(1/γ)`.
/// Identity at 1.0; above it, partial coverage is lifted (a stem's anti-aliased rim reads
/// fuller), which is the correction light-on-dark text wants under a linear-light blend.
/// Full and zero coverage are fixed points, so a glyph's interior and exterior never move.
pub(super) fn contrast_lut(gamma: f32) -> [u8; 256] {
    let mut lut = [0u8; 256];
    let inv = 1.0 / gamma.max(0.01);
    for (i, slot) in lut.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *slot = (c.powf(inv) * 255.0 + 0.5) as u8;
    }
    lut[0] = 0;
    lut[255] = 255;
    lut
}

/// Rec. 601 luma of an sRGB triple, in 0..=255·1000 units — a cheap "is this lighter
/// than that" ordering, not a colorimetric quantity.
#[inline]
pub(crate) fn luma(rgb: [u8; 3]) -> u32 {
    299 * rgb[0] as u32 + 587 * rgb[1] as u32 + 114 * rgb[2] as u32
}

/// `luma` of a packed `0xRRGGBB` pixel.
#[inline]
pub(super) fn luma_packed(px: u32) -> u32 {
    luma([
        ((px >> 16) & 0xff) as u8,
        ((px >> 8) & 0xff) as u8,
        (px & 0xff) as u8,
    ])
}

#[cfg(test)]
mod text_contrast_tests {
    use super::*;

    /// The coverage curve: identity at γ = 1, a lift everywhere in between at γ > 1,
    /// monotone, and with 0 and 255 as fixed points so a glyph's interior and exterior
    /// never move.
    #[test]
    fn the_contrast_curve_lifts_partial_coverage_and_keeps_the_ends() {
        let flat = contrast_lut(1.0);
        assert!(
            (0..=255).all(|i| flat[i] == i as u8),
            "γ=1 must be the identity"
        );
        let lut = contrast_lut(1.6);
        assert_eq!((lut[0], lut[255]), (0, 255));
        // strictly above the identity through the body of the range; the last entries
        // round back onto it (254 → 254.4)
        assert!(
            (1..250).all(|i| lut[i] > i as u8) && (250..255).all(|i| lut[i] >= i as u8),
            "γ>1 must lift every partial coverage"
        );
        assert!(
            lut.windows(2).all(|w| w[0] <= w[1]),
            "the curve must be monotone"
        );
        assert!(
            lut[128] >= 160,
            "half coverage should lift markedly at γ=1.6: {}",
            lut[128]
        );
    }

    /// Text composites in the ENCODED space, so a partly covered pixel comes out at the
    /// fraction of the pixel the outline covers — 50% coverage of white on black is
    /// mid-grey, not the 188/255 a linear-light lerp produces. This is the whole
    /// difference between text that reads crisp and text that reads puffy, so it is
    /// pinned as numbers rather than left to the eye.
    #[test]
    fn text_composites_in_srgb_space_not_linear_light() {
        assert_eq!(blend_text(0x000000, [0xff; 3], 128) & 0xff, 128);
        assert_eq!(blend_text(0x000000, [0xff; 3], 64) & 0xff, 64);
        assert_eq!(blend_text(0xffffff, [0; 3], 128) & 0xff, 127);
        // the ends are exact, and the linear path (geometry, cursor) is left alone
        assert_eq!(blend_text(0x000000, [0xff; 3], 0), 0x000000);
        assert_eq!(blend_text(0x000000, [0xff; 3], 255), 0xffffff);
        assert!(
            blend(0x000000, [0xff; 3], 128) & 0xff > 180,
            "the linear-light blend must stay as it is for geometry"
        );
        // per-channel subpixel coverage lerps the same way, so a gray and an LCD glyph
        // side by side carry the same weight
        assert_eq!(
            blend_text_rgb(0x000000, [0xff; 3], [128, 128, 128]),
            0x808080
        );
        assert_eq!(
            blend_text_rgb(0x000000, [0xff; 3], [255, 128, 0]) & 0xffffff,
            0xff8000
        );
    }

    /// Ink (blue-channel sum) of the glyph `l` drawn in `fg` over a canvas of `ground`
    /// at contrast `gamma`.
    fn ink(r: &mut Renderer, gamma: f32, fg: [u8; 3], ground: u32) -> (u64, Vec<u32>) {
        r.set_text_contrast(gamma);
        let mut buf = vec![ground; 32 * 32];
        let mut canvas = Canvas::full(&mut buf, 32, 32);
        r.draw_cluster(&mut canvas, 2, 2, "l", None, false, false, 1, fg, 0);
        (buf.iter().map(|&p| (p & 0xff) as u64).sum(), buf)
    }

    /// The lift applies only where the text is lighter than its ground: white on black
    /// gains ink at γ > 1, and black on white renders identically at γ = 1 and γ = 1.8.
    #[test]
    fn the_lift_is_one_sided_light_on_dark_only() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        r.set_text_aa(TextAa::Gray);
        let (plain, _) = ink(&mut r, 1.0, [0xff; 3], 0);
        let (lifted, _) = ink(&mut r, 1.8, [0xff; 3], 0);
        assert!(
            lifted > plain,
            "white on black should gain ink: {plain} -> {lifted}"
        );
        let (_, a) = ink(&mut r, 1.0, [0; 3], 0xffffff);
        let (_, b) = ink(&mut r, 1.8, [0; 3], 0xffffff);
        assert_eq!(a, b, "dark on light must be untouched by the lift");
    }
}

#[cfg(test)]
mod grid_stability_tests {
    use super::*;

    /// A size change must not move the grid sideways: centring the horizontal remainder
    /// (`width % cell_w`, which depends on the cell size) would slide the grid by up to
    /// half a cell on every font-size step.
    #[test]
    fn the_grid_does_not_slide_sideways_as_the_font_size_changes() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        let (w, h) = (1913, 1077); // deliberately not a multiple of any cell size
        let xs: Vec<usize> = (10..=30)
            .map(|px| {
                r.set_font(None, Some(px as f32));
                r.grid_origin(w, h).0
            })
            .collect();
        assert!(
            xs.windows(2).all(|p| p[0] == p[1]),
            "the grid's x origin moved across sizes 10..30: {xs:?}"
        );
    }

    /// Row 0 starts at the window's top edge at every font size and inset, and the rows fill
    /// down to the bottom inset with less than a row left over: a header line on row 0 is the
    /// top of the window, with no strip of margin above it to show as a gap (the bug this rule
    /// replaced: the top margin was the inset plus half the leftover).
    #[test]
    fn row_zero_starts_at_the_top_edge_at_every_size() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        let (w, h) = (1913, 1077);
        for inset in [0.0, 8.0, 12.0] {
            r.set_inset(inset);
            for px in 10..=30 {
                r.set_font(None, Some(px as f32));
                let (_, rows) = super::super::bleed::grid_cells(&r, w, h);
                assert_eq!(r.grid_origin(w, h).1, 0, "at {px} px, inset {inset}");
                let below = h - rows as usize * r.cell_h;
                assert!(
                    below >= r.inset() && below < r.inset() + r.cell_h,
                    "at {px} px, inset {inset}: {below} px below the last row"
                );
            }
        }
    }

    /// Setting a font or line height to what it already is must not recompute at all, and a
    /// size change must not throw away what is already rasterised. The cluster cache is keyed
    /// by px, so sizes coexist: a zoom no longer re-rasterises every glyph per step, and a
    /// zoom back to a size already drawn finds its glyphs waiting.
    #[test]
    fn the_cluster_cache_survives_a_size_change() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        r.set_font(None, Some(20.0));
        let mut buf = vec![0u32; 32 * 32];
        {
            let mut canvas = Canvas::full(&mut buf, 32, 32);
            r.draw_cluster(&mut canvas, 0, 0, "M", None, false, false, 1, [0xff; 3], 0);
        }
        let warm = r.cache.len();
        assert!(warm > 0, "the cluster cache should have an entry to lose");

        r.set_font(None, Some(20.0)); // same size
        r.set_font(Some(r.default_family), None); // same family
        r.set_line_height(r.line_height); // same line height
        assert_eq!(r.cache.len(), warm, "an unchanged set disturbed the cache");

        // a real change keeps what is there and adds the new size beside it
        r.set_font(None, Some(21.0));
        {
            let mut canvas = Canvas::full(&mut buf, 32, 32);
            r.draw_cluster(&mut canvas, 0, 0, "M", None, false, false, 1, [0xff; 3], 0);
        }
        assert_eq!(r.cache.len(), warm + 1, "the old size was thrown away");

        // …and coming back finds the first entry still there, with nothing new baked
        r.set_font(None, Some(20.0));
        {
            let mut canvas = Canvas::full(&mut buf, 32, 32);
            r.draw_cluster(&mut canvas, 0, 0, "M", None, false, false, 1, [0xff; 3], 0);
        }
        assert_eq!(
            r.cache.len(),
            warm + 1,
            "returning to a drawn size re-rasterised it"
        );
    }
}

#[cfg(test)]
mod font_tests {
    //! ADR-442: system fonts by name, fallback by coverage, ligature units. Written for CI
    //! (they read no font a stock runner might lack, except where noted), never run on
    //! the desktop they were written on.
    use super::*;

    fn units(text: &str, ligatures: bool) -> Vec<&str> {
        text_units(text, ligatures).collect()
    }

    #[test]
    fn without_ligatures_a_text_op_is_its_grapheme_clusters() {
        assert_eq!(units("a->b", false), ["a", "-", ">", "b"]);
        assert_eq!(units("e\u{301}!", false), ["e\u{301}", "!"]);
    }

    #[test]
    fn with_ligatures_a_punctuation_run_is_one_unit() {
        assert_eq!(units("a->b", true), ["a", "->", "b"]);
        assert_eq!(units("x === y", true), ["x", " ", "===", " ", "y"]);
        assert_eq!(units("|> !=", true), ["|>", " ", "!="]);
        // a lone punctuation char stays a cluster; letters never join a run
        assert_eq!(units("a-b", true), ["a", "-", "b"]);
        assert_eq!(units("www", true), ["w", "w", "w"]);
    }

    #[test]
    fn a_long_run_is_cut_into_bounded_units() {
        assert_eq!(units("==========", true), ["========", "=="]);
    }

    #[test]
    fn a_combining_mark_stays_with_the_char_it_marks() {
        // the mark makes `>` + U+0301 one cluster, so the run stops before it
        assert_eq!(units("->\u{301}", true), ["-", ">\u{301}"]);
        assert_eq!(units("-\u{301}>", true), ["-\u{301}", ">"]);
        assert_eq!(units("<=>\u{301}", true), ["<=", ">\u{301}"]);
    }

    #[test]
    fn a_unit_spans_one_cell_per_char_of_a_run() {
        assert_eq!(unit_cells("==="), 3);
        assert_eq!(unit_cells("->"), 2);
        assert_eq!(unit_cells("a"), 1);
        assert_eq!(unit_cells("中"), 2);
        assert_eq!(unit_cells("\u{301}"), 0);
        // the units of a line add up to its display width, ligatures or not
        for text in ["fn a() -> b { x != y }", "a <=> b |> c", "中-->文"] {
            let lig: usize = text_units(text, true).map(unit_cells).sum();
            let plain: usize = text_units(text, false).map(unit_cells).sum();
            assert_eq!(lig, plain, "{text:?}");
            assert_eq!(
                lig,
                crate::host::text_width::display_width(text),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_ligature_unit_bakes_one_canvas_as_wide_as_its_cells() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        r.set_ligatures(true);
        let cell_w = r.cell_w;
        let glyph = r.cluster_glyph("->", None, false, false, 1, false);
        assert_eq!(glyph.width, 2 * cell_w);
        assert!(
            glyph.rgba.chunks(4).any(|px| px[3] > 0),
            "the run drew nothing"
        );
        let glyph = r.cluster_glyph("===", None, false, false, 1, false);
        assert_eq!(glyph.width, 3 * cell_w);
    }

    #[test]
    fn toggling_ligatures_forgets_the_retained_frame_only_on_a_change() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        r.prev_ops.push(Op::Clear);
        r.set_ligatures(false); // unchanged
        assert_eq!(r.prev_ops.len(), 1);
        r.set_ligatures(true);
        assert!(r.prev_ops.is_empty());
        assert!(r.ligatures());
    }

    #[test]
    fn the_bundled_font_never_scans_the_system() {
        let mut shared = FontShared::new();
        for text in ["a", "M", "->", "é", "─", "😀"] {
            assert_eq!(
                shared.fallback_family(text, MONO_FAMILY, false, false),
                None
            );
        }
        assert!(
            shared.system.is_none(),
            "drawing what the bundled fonts cover scanned the installed fonts"
        );
        let info = shared.info(value::intern(DEFAULT_FAMILY), false);
        assert_eq!(info.origin, "bundled");
        assert_eq!(info.name, MONO_FAMILY);
        assert!(!info.system_scanned);
    }

    /// Reads the runner's installed fonts — but only to fail to find one.
    #[test]
    fn a_family_name_nothing_installed_answers_to_falls_back_to_the_bundled_mono() {
        let mut shared = FontShared::new();
        let id = value::intern("no-such-font-family-adr-432");
        shared.register_installed(id, "No Such Font Family ADR 432");
        assert_eq!(shared.name_of(id), MONO_FAMILY);
        let info = shared.info(id, false);
        assert_eq!(info.origin, "not-found");
        assert_eq!(
            info.requested.as_deref(),
            Some("No Such Font Family ADR 432")
        );
        assert!(info.system_scanned);
    }

    /// Reads the runner's installed fonts: whatever it finds (if anything) for a CJK
    /// ideograph must be loaded where shaping can reach it, and asked for once.
    #[test]
    fn a_fallback_found_on_the_system_is_loaded_for_shaping_and_remembered() {
        let mut shared = FontShared::new();
        let found = shared.fallback_family("中", MONO_FAMILY, false, false);
        if let Some(family) = &found {
            assert!(
                query_face(shared.fs.db(), family, Weight::NORMAL, Style::Normal).is_some(),
                "{family} was chosen but not loaded into the shaping database"
            );
            assert_eq!(
                shared
                    .info(value::intern(DEFAULT_FAMILY), false)
                    .fallbacks
                    .len(),
                1
            );
        }
        let scanned = shared.loaded_files.len();
        assert_eq!(
            shared.fallback_family("中", MONO_FAMILY, false, false),
            found
        );
        assert_eq!(
            shared.loaded_files.len(),
            scanned,
            "the second ask loaded again"
        );
    }
}

#[cfg(test)]
mod centred_symbol_tests {
    use super::*;

    fn glyph_ink(glyph: &CachedGlyph) -> Option<(usize, usize)> {
        ink_rows(&glyph.rgba, glyph.width, glyph.height)
    }

    /// A status dot (bedit's VCS state, a modified marker) and the geometric shapes like it
    /// mean "here is a mark", not text: they sit in the MIDDLE of their cell, as a terminal
    /// draws them, wherever their font's design puts them on the baseline. DejaVu's sit
    /// low, and a taller line (`set_line_height`) made it plainer: the dot at the end of a
    /// mode line read as dropped below the text. Every size, both line heights, gray and
    /// subpixel: the ink's top and bottom margins differ by a pixel at most.
    #[test]
    fn a_geometric_symbol_is_centred_in_its_cell() {
        for line_height in [1.0, 1.4] {
            for px in 10..=30 {
                for subpixel in [false, true] {
                    let mut r = Renderer::new(1.0, default_families(), px as f32);
                    r.set_line_height(line_height);
                    for symbol in ["●", "■", "◆", "▲", "○", "◉"] {
                        let glyph =
                            r.build_cluster(symbol, r.default_family, false, false, 1, subpixel);
                        let (first, last) = glyph_ink(&glyph)
                            .unwrap_or_else(|| panic!("{symbol} at {px} px drew no ink"));
                        let above = first;
                        let below = glyph.height - 1 - last;
                        assert!(
                            above.abs_diff(below) <= 1,
                            "{symbol} at {px} px, line height {line_height}, subpixel \
                             {subpixel}: {above} rows above its ink, {below} below"
                        );
                    }
                }
            }
        }
    }

    /// Text is not centred: a letter keeps its place on the baseline, so `x` and `g` still
    /// line up with each other after the symbols move.
    #[test]
    fn a_letter_stays_on_the_baseline() {
        let r = Renderer::new(1.0, default_families(), 15.0);
        let x = r.build_cluster("x", r.default_family, false, false, 1, false);
        let g = r.build_cluster("g", r.default_family, false, false, 1, false);
        let (x_top, x_bottom) = glyph_ink(&x).expect("x drew no ink");
        let (_, g_bottom) = glyph_ink(&g).expect("g drew no ink");
        assert!(g_bottom > x_bottom, "g's descender no longer drops below x");
        assert!(x_top > 0);
    }
}
