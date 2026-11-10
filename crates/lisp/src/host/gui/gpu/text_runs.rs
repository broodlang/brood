//! Text runs on the GPU target: a run drawn by replaying its cached quads when it repeats,
//! or shaped into cell backgrounds, atlas glyphs and underlines when it does not — the
//! CPU painter's text walk.

use super::*;

impl GpuWindow {
    /// Draw text run `s` in `face` with its anchor at `(anchor_x, anchor_y)` — a cell
    /// run's grid origin and row top, a pixel run's `x` and top — replaying the quads
    /// the last frame shaped for it when it repeats, shaping (and remembering) it when
    /// it does not (`RunCache`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_run(
        &mut self,
        fb: &mut FrameBuilder,
        renderer: &mut Renderer,
        s: &str,
        face: &Face,
        place: RunPlace,
        g: Grid,
        anchor_x: f32,
        anchor_y: f32,
    ) {
        let metrics = renderer.metrics();
        let context = RunContext {
            place,
            metrics: [
                g.cw.to_bits(),
                g.ch.to_bits(),
                metrics.px.to_bits(),
                renderer.default_family(),
            ],
            ground: renderer.bg(),
            fonts: (
                renderer.font_generation(),
                renderer.ligatures(),
                self.lcd_order(renderer),
            ),
        };
        let reuse = self.reuse;
        let hash = run_hash(s, face, &context);
        if reuse {
            // an enclosing fragment's entry carries the runs it drew (`FragmentCache::record`)
            self.fragments.runs_used.push(hash);
        }
        if reuse && let Some(quads) = self.runs.take(hash, s, face, &context) {
            replay(fb, quads, anchor_x, anchor_y);
            return;
        }
        let quads = self.shape_run(renderer, s, face, place, g.cw, g.ch, context.ground);
        replay(fb, &quads, anchor_x, anchor_y);
        if reuse {
            self.runs.put(
                hash,
                TextRun {
                    text: s.to_string(),
                    face: *face,
                    context,
                    quads,
                },
            );
        }
    }

    /// Shape a text run into quads relative to its anchor: every cell background first
    /// (one solid batch), then a glyph per non-space cluster out of the atlas (one
    /// textured batch), then any underline — the CPU painter's text walk. A cell run
    /// starts at its column (a raw tab advances to the next SCREEN stop, background
    /// only); a pixel run at its alignment's shift by the run's shaped width.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shape_run(
        &mut self,
        renderer: &mut Renderer,
        s: &str,
        face: &Face,
        place: RunPlace,
        cw: f32,
        ch: f32,
        ground: [u8; 3],
    ) -> Vec<RunQuad> {
        let atlas_size = ATLAS_SIZE as f32;
        let (mut fg, mut bg) = (
            face.fg.unwrap_or(renderer.fg()),
            face.bg.unwrap_or(renderer.bg()),
        );
        // A face with no `:bg` is transparent: the glyph composites over whatever is
        // under it (an hl-line band), exactly like Emacs.
        let mut paint_bg = face.bg.is_some();
        if face.reverse {
            std::mem::swap(&mut fg, &mut bg);
            paint_bg = true;
        }
        // The ground a glyph's blend is corrected by, and the contrast lift applies to text
        // lighter than (ADR-611). The CPU reads it per pixel; here it is the run's own
        // background, or — for a transparent face — the window's, since what else lies under
        // it is not known until the blend (ADR-440).
        let ground_luminance = luminance(if paint_bg { bg } else { ground });
        let lift = luminance(fg) > ground_luminance;
        let scale = face.scale.max(1) as usize;
        let ch_s = scale as f32 * ch;
        let mut quads = Vec::new();
        let mut clusters: Vec<(f32, f32, &str)> = Vec::new();
        match place {
            RunPlace::Cell { col } => {
                let mut cx = col as usize;
                // Ligature units when `gui-ligatures!` is on: a punctuation run is one
                // glyph canvas spanning its cells (`text_units`, ADR-442).
                for cluster in text_units(s, renderer.ligatures()) {
                    if cluster == "\t" {
                        let cells = cluster_cells_at(cluster, cx, TAB_WIDTH);
                        if paint_bg {
                            quads.push(RunQuad::Fill {
                                x: cx as f32 * cw,
                                y: 0.0,
                                w: cells as f32 * cw,
                                h: ch_s,
                                rgb: bg,
                            });
                        }
                        cx += cells;
                        continue;
                    }
                    let cells = unit_cells(cluster);
                    if cells == 0 {
                        continue;
                    }
                    let block_w = (cells * scale) as f32 * cw;
                    let left = cx as f32 * cw;
                    if paint_bg {
                        quads.push(RunQuad::Fill {
                            x: left,
                            y: 0.0,
                            w: block_w,
                            h: ch_s,
                            rgb: bg,
                        });
                    }
                    if cluster != " " {
                        clusters.push((left, block_w, cluster));
                    }
                    cx += cells * scale;
                }
            }
            RunPlace::Pixel { align } => {
                let width = text_px_width(s, cw as usize, scale) as f32;
                let left0 = match align {
                    1 => -width / 2.0,
                    2 => -width,
                    _ => 0.0,
                };
                let mut cx = 0.0f32;
                for cluster in s.graphemes(true) {
                    let cells = cluster_cells(cluster);
                    if cells == 0 {
                        continue;
                    }
                    let block_w = (cells * scale) as f32 * cw;
                    let left = left0 + cx;
                    if paint_bg {
                        quads.push(RunQuad::Fill {
                            x: left,
                            y: 0.0,
                            w: block_w,
                            h: ch_s,
                            rgb: bg,
                        });
                    }
                    if cluster != " " {
                        clusters.push((left, block_w, cluster));
                    }
                    cx += block_w;
                }
            }
        }
        for (left, _, cluster) in &clusters {
            let Some(glyph) = self.glyph_slot(
                renderer,
                cluster,
                face.family,
                face.bold,
                face.italic,
                face.scale,
            ) else {
                continue;
            };
            quads.push(RunQuad::Glyph {
                page: glyph.page,
                x: *left,
                y: 0.0,
                w: glyph.w as f32,
                h: glyph.h as f32,
                uv: [
                    glyph.x as f32 / atlas_size,
                    glyph.y as f32 / atlas_size,
                    glyph.w as f32 / atlas_size,
                    glyph.h as f32 / atlas_size,
                ],
                tint: [fg[0], fg[1], fg[2], 255],
                mode: glyph_mode(glyph.mono, glyph.lcd, lift),
                ground: ground_luminance,
            });
        }
        if face.underline {
            // A rule (or a wave) near the block bottom, in its own colour or the text's,
            // scaled with the glyph so it stays proportional.
            let underline_y = ch_s - 2.0 * scale as f32;
            let rgb = face.underline_rgb.unwrap_or(fg);
            for (left, block_w, _) in &clusters {
                for (x, y, w, h) in crate::host::gui::underline_rects(
                    left.round() as isize,
                    underline_y.round() as isize,
                    block_w.round().max(0.0) as usize,
                    scale,
                    face.underline_wave,
                ) {
                    quads.push(RunQuad::Fill {
                        x: x as f32,
                        y: y as f32,
                        w: w as f32,
                        h: h as f32,
                        rgb,
                    });
                }
            }
        }
        quads
    }
}
