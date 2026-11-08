//! Which font draws a glyph, and how a font's vertical metrics place a line: the shared
//! `cosmic-text` engine, system fonts found by name or by coverage (ADR-442), and the box
//! a font's numbers give a cell row. The rasteriser is `render`.

use super::*;

use swash::scale::ScaleContext;

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
    pub(super) fs: FontSystem,
    pub(super) swash: SwashCache,
    /// swash's own scaler, for the subpixel (LCD) raster path: cosmic-text's
    /// `SwashCache` renders alpha masks only, so a per-channel mask is rendered here
    /// from the same font + glyph id + hinting.
    pub(super) scaler: ScaleContext,
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
    pub(super) generation: u64,
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
    pub(super) fn name_of(&self, id: u32) -> String {
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
        } else {
            super::report_degraded(
                "font",
                &format!(
                    "{} is not a font this can read; the family is not registered and its text uses the default font",
                    paths[0]
                ),
                None,
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
            super::report_degraded(
                "font",
                &format!(
                    "no installed font is called {wanted:?}; the bundled DejaVu Sans Mono stands in"
                ),
                None,
            );
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

/// A font's vertical metrics, in font units: what CSS's inline layout places a line of it by
/// (ADR-611). `ascent` and `descent` are the box the half-leading is split around — the OS/2
/// typographic ascender and descender, the hhea ones when the font has no plausible typo
/// metrics — and `x_height` is where lowercase ink tops out, which is where the optical
/// middle of a line of code sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FontVertical {
    pub(super) units_per_em: f32,
    pub(super) ascent: f32,
    /// Below the baseline, positive.
    pub(super) descent: f32,
    /// The hhea ascender: the line a font's tallest glyphs (an accented capital) are drawn
    /// under, which a baseline is never placed so high as to cut off (`line_box`).
    pub(super) clip_ascent: f32,
    pub(super) x_height: f32,
}

impl FontVertical {
    /// For a font whose tables cannot be read: an 0.8/0.2 em box, a half-em x-height.
    pub(super) const FALLBACK: FontVertical = FontVertical {
        units_per_em: 1.0,
        ascent: 0.8,
        descent: 0.2,
        clip_ascent: 0.8,
        x_height: 0.5,
    };

    /// Read `font`'s `head`, `hhea` and `OS/2` tables; `None` when it has no usable box.
    pub(super) fn read(font: &swash::FontRef) -> Option<FontVertical> {
        use swash::{TableProvider, tag_from_bytes};
        let be_u16 = |table: &[u8], at: usize| -> Option<u16> {
            Some(u16::from_be_bytes([*table.get(at)?, *table.get(at + 1)?]))
        };
        let be_i16 = |table: &[u8], at: usize| be_u16(table, at).map(|v| v as i16);
        let head = font.table_by_tag(tag_from_bytes(b"head"))?;
        let units_per_em = be_u16(head, 18).filter(|u| *u > 0)? as f32;
        let hhea = font
            .table_by_tag(tag_from_bytes(b"hhea"))
            .and_then(|t| Some((be_i16(t, 4)?, be_i16(t, 6)?)));
        let os2 = font.table_by_tag(tag_from_bytes(b"OS/2"));
        let typo = os2.and_then(|t| Some((be_i16(t, 68)?, be_i16(t, 70)?)));
        // sxHeight exists from OS/2 version 2 on
        let x_height = os2
            .filter(|t| be_u16(t, 0).is_some_and(|version| version >= 2))
            .and_then(|t| be_i16(t, 86))
            .filter(|x| *x > 0)
            .map(f32::from);
        let (ascent, descent) = vertical_box(units_per_em, typo, hhea)?;
        let clip_ascent = hhea
            .map(|(hhea_ascent, _)| f32::from(hhea_ascent))
            .filter(|clip| *clip > ascent)
            .unwrap_or(ascent);
        Some(FontVertical {
            units_per_em,
            ascent,
            descent,
            clip_ascent,
            x_height: x_height.unwrap_or(units_per_em * 0.5),
        })
    }
}

/// The ascent/descent box a line of a font is laid out by (CSS Inline 3): the OS/2
/// typographic ascender and descender when they are plausible — a positive ascender, a
/// descender at or below the baseline, a box between 0.6 and 2.5 em — else the hhea ones.
/// The typo metrics are the font's statement of its text's extent; hhea is often stretched
/// to clear every accent in the font (DejaVu's spans 1.16 em), which is the box that sat
/// bedit's text low in its rows. Descent comes back positive.
pub(super) fn vertical_box(
    units_per_em: f32,
    typo: Option<(i16, i16)>,
    hhea: Option<(i16, i16)>,
) -> Option<(f32, f32)> {
    let plausible = |(ascent, descent): (i16, i16)| {
        let span = (ascent as f32 - descent as f32) / units_per_em;
        ascent > 0 && descent <= 0 && (0.6..=2.5).contains(&span)
    };
    typo.filter(|box_| plausible(*box_))
        .or(hhea.filter(|(ascent, descent)| *ascent > 0 && *descent <= 0))
        .map(|(ascent, descent)| (ascent as f32, -(descent as f32)))
}

/// Where text sits in a cell `cell_h` px tall at font size `px`: `(baseline, x_middle)`
/// from the cell's top. CSS's half-leading: the leading `L = cell − (A + D)` is split half
/// above the ascent and half below the descent, and the baseline lands at `round(L/2 + A)`
/// — a whole pixel, rounded once, here, so every row of every region puts its baseline on
/// the same pixel of its cell.
///
/// One guard on it: where the font's clip ascent (hhea, where its accented capitals reach)
/// would leave the cell, the baseline moves down a pixel, while the cell has room for that
/// and the descent. DejaVu Sans Mono's typo box is an em and its accents reach 0.93 em, so
/// at a line height of 1.25 the pure half-leading baseline cuts an É's tip off. One pixel,
/// not as far as the accents want: further puts the lowercase visibly low again, which is
/// the fault the typo box fixes, for a glyph code rarely holds.
///
/// `x_middle` is half an x-height above the baseline, in px (the x-height rounded to whole
/// pixels as the hinter rounds it): the optical middle of lowercase text, where a geometric
/// mark is centred.
pub(super) fn line_box(vertical: &FontVertical, px: f32, cell_h: f32) -> (i32, f32) {
    let scale = px / vertical.units_per_em;
    let (ascent, descent) = (vertical.ascent * scale, vertical.descent * scale);
    let half_leading = (cell_h - (ascent + descent)) / 2.0;
    let centred = (half_leading + ascent).round();
    let clear_of_accents = (vertical.clip_ascent * scale)
        .ceil()
        .min(cell_h - descent.ceil())
        .min(centred + 1.0);
    let baseline = centred.max(clear_of_accents).clamp(0.0, cell_h);
    let x_middle = baseline - (vertical.x_height * scale).round() / 2.0;
    (baseline as i32, x_middle.clamp(0.0, cell_h))
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
        r.retained.prev_ops.push(Op::Clear);
        r.set_ligatures(false); // unchanged
        assert_eq!(r.retained.prev_ops.len(), 1);
        r.set_ligatures(true);
        assert!(r.retained.prev_ops.is_empty());
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
mod line_box_tests {
    use super::super::offscreen::picture;
    use super::*;
    use crate::host::gui::{Face, FrameImage, Op};

    /// DejaVu Sans Mono's own numbers: a typo box of exactly an em, an hhea box of 1.16 em.
    const DEJAVU: FontVertical = FontVertical {
        units_per_em: 2048.0,
        ascent: 1556.0,
        descent: 492.0,
        clip_ascent: 1901.0,
        x_height: 1120.0,
    };

    /// The box a line is laid out by is the OS/2 typo one when it is plausible (CSS Inline
    /// 3), the hhea one when the typo metrics are missing or absurd.
    #[test]
    fn the_typo_box_is_used_when_plausible_else_hhea() {
        let hhea = Some((1901, -483));
        assert_eq!(
            vertical_box(2048.0, Some((1556, -492)), hhea),
            Some((1556.0, 492.0))
        );
        assert_eq!(
            vertical_box(2048.0, Some((0, 0)), hhea),
            Some((1901.0, 483.0))
        );
        assert_eq!(vertical_box(2048.0, None, hhea), Some((1901.0, 483.0)));
        assert_eq!(
            vertical_box(1000.0, Some((3000, -200)), Some((800, -200))),
            Some((800.0, 200.0))
        );
        assert_eq!(vertical_box(1000.0, None, None), None);
    }

    /// The baseline is the half-leading one, rounded once — and never so high that the
    /// font's accents leave the cell while the cell has room for them.
    #[test]
    fn the_baseline_is_the_half_leading_one_clear_of_the_accents() {
        let no_accents = FontVertical {
            clip_ascent: DEJAVU.ascent,
            ..DEJAVU
        };
        // 16 px in 22: A 12.16, D 3.84, L 6 — round(3 + 12.16) = 15, the accents clear
        assert_eq!(line_box(&DEJAVU, 16.0, 22.0), (15, 10.5));
        // 16 px in 20: round(2 + 12.16) = 14, but É reaches 14.85 px: one lower
        assert_eq!(line_box(&no_accents, 16.0, 20.0).0, 14);
        assert_eq!(line_box(&DEJAVU, 16.0, 20.0).0, 15);
        // 16 px in 16: no room for both, and the descent keeps its pixels
        assert_eq!(line_box(&DEJAVU, 16.0, 16.0).0, 12);
    }

    /// Rows `[top, bottom]` of ink in the `w`×`h` box at `(x0, y0)` of `image`, where ink
    /// is any pixel that is not black.
    fn ink_in(image: &FrameImage, x0: u32, y0: u32, w: u32, h: u32) -> Option<(u32, u32)> {
        let inked = |y: u32| {
            (x0..x0 + w).any(|x| {
                let i = ((y * image.width + x) * 4) as usize;
                image.rgba[i..i + 3] != [0, 0, 0]
            })
        };
        let first = (y0..y0 + h).find(|y| inked(*y))?;
        let last = (y0..y0 + h).rev().find(|y| inked(*y))?;
        Some((first - y0, last - y0))
    }

    /// What `gui/render-frame` paints (`offscreen::picture`): "Hxg" on every row, at three
    /// sizes and two line heights. Every row puts its `H`'s cap top and its baseline on the
    /// same pixel of its cell, and the baseline is the one the metrics say: the `H` stands on
    /// it (its last inked row is the one above).
    #[test]
    fn every_row_puts_its_cap_top_and_baseline_on_the_same_pixel() {
        for px in [11.0, 15.0, 22.0] {
            for line_height in [1.25, 1.4] {
                let mut r = Renderer::new(1.0, default_families(), px);
                r.set_line_height(line_height);
                r.set_bg(Some([0, 0, 0]));
                let metrics = r.metrics();
                let rows = 5u16;
                let mut ops = vec![Op::Clear];
                ops.extend((0..rows).map(|row| Op::Text {
                    row,
                    col: 0,
                    s: "Hxg".into(),
                    face: Face {
                        fg: Some([255, 255, 255]),
                        ..Face::default()
                    },
                }));
                let (w, h) = (metrics.cell_w * 4, metrics.cell_h * rows as usize);
                let image = picture(&mut r, &ops, w, h);
                let (cell_w, cell_h) = (metrics.cell_w as u32, metrics.cell_h as u32);
                let per_row: Vec<_> = (0..rows as u32)
                    .map(|row| ink_in(&image, 0, row * cell_h, cell_w, cell_h))
                    .collect();
                let (top, bottom) = per_row[0].expect("the H drew no ink");
                assert!(
                    per_row.iter().all(|ink| *ink == Some((top, bottom))),
                    "{px} px, line height {line_height}: the H sits differently by row: \
                     {per_row:?}"
                );
                assert_eq!(
                    bottom as i32 + 1,
                    metrics.baseline,
                    "{px} px, line height {line_height}: the H does not stand on the baseline"
                );
            }
        }
    }

    /// A lowercase letter's ink is centred in its row — within a pixel of the row's middle
    /// and of `x_middle`, the half-leading prediction — at every size from 10 to 30 px and
    /// the line heights an editor uses. Before the typo box, DejaVu's lowercase sat up to two
    /// pixels low (its hhea box carries the accents' room above the caps).
    #[test]
    fn lowercase_ink_is_centred_in_its_row() {
        for line_height in [1.25, 1.4, 1.6] {
            for px in 10..=30 {
                let mut r = Renderer::new(1.0, default_families(), px as f32);
                r.set_line_height(line_height);
                let metrics = r.metrics();
                let x = r.build_cluster("x", r.default_family, false, false, 1, false);
                let (first, last) = ink_rows(&x.rgba, x.width, x.height).expect("x drew no ink");
                let ink_middle = (first + last + 1) as f32 / 2.0;
                let row_middle = metrics.cell_h as f32 / 2.0;
                assert!(
                    (ink_middle - row_middle).abs() <= 1.0,
                    "{px} px, line height {line_height}: x's ink {first}..={last} is centred at \
                     {ink_middle}, the row's middle is {row_middle}"
                );
                assert!(
                    (ink_middle - metrics.x_middle as f32).abs() <= 1.0,
                    "{px} px, line height {line_height}: x's ink centred at {ink_middle}, \
                     x_middle says {}",
                    metrics.x_middle
                );
            }
        }
    }
}
