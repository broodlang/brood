//! Unit tests of the GPU target's pure parts: quad culling, batching and the clip band,
//! the cursor and scroll geometry, the shaders' validation, the particle uniform, and the
//! swapchain's format and present-mode choice — all without a device.

use super::*;

const FW: f32 = 800.0;
const FH: f32 = 600.0;

#[test]
fn an_on_screen_quad_is_kept() {
    assert!(quad_visible(10.0, 10.0, 5.0, 5.0, FW, FH));
    // Partly off each edge still touches the viewport.
    assert!(quad_visible(-2.0, -2.0, 5.0, 5.0, FW, FH));
    assert!(quad_visible(798.0, 598.0, 5.0, 5.0, FW, FH));
}

#[test]
fn a_quad_entirely_outside_the_viewport_is_culled() {
    assert!(!quad_visible(-10.0, 10.0, 5.0, 5.0, FW, FH)); // left
    assert!(!quad_visible(10.0, -10.0, 5.0, 5.0, FW, FH)); // above
    assert!(!quad_visible(FW, 10.0, 5.0, 5.0, FW, FH)); // right
    assert!(!quad_visible(10.0, FH, 5.0, 5.0, FW, FH)); // below
    // A bitboard cell far past the window: the `Op::Cells` case the cull exists for.
    assert!(!quad_visible(1_000_000.0, 1_000_000.0, 8.0, 16.0, FW, FH));
}

#[test]
fn a_degenerate_quad_is_culled() {
    assert!(!quad_visible(10.0, 10.0, 0.0, 5.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, 5.0, 0.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, -5.0, 5.0, FW, FH));
}

#[test]
fn a_nan_coordinate_is_culled_not_buffered() {
    // Every comparison against NaN is false, so a positively-phrased test culls; a
    // negatively-phrased one (`!(x + w <= 0.0 || …)`) would have buffered the quad.
    assert!(!quad_visible(f32::NAN, 10.0, 5.0, 5.0, FW, FH));
    assert!(!quad_visible(10.0, f32::NAN, 5.0, 5.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, f32::NAN, 5.0, FW, FH));
    assert!(!quad_visible(10.0, 10.0, 5.0, f32::NAN, FW, FH));
}

#[test]
fn an_infinity_is_judged_by_where_it_puts_the_quad() {
    // A quad starting at -inf with finite width never reaches the viewport: culled.
    assert!(!quad_visible(f32::NEG_INFINITY, 10.0, 5.0, 5.0, FW, FH));
    // A quad starting at +inf is past the right edge: culled.
    assert!(!quad_visible(f32::INFINITY, 10.0, 5.0, 5.0, FW, FH));
    // An infinitely wide quad from an on-screen x covers the viewport: kept.
    assert!(quad_visible(10.0, 10.0, f32::INFINITY, 5.0, FW, FH));
}

#[test]
fn a_saturating_extent_does_not_wrap_into_view() {
    // `x + w` overflowing to +inf must not read as "on screen" from the left.
    assert!(!quad_visible(f32::MAX, 10.0, f32::MAX, 5.0, FW, FH));
}

#[test]
fn a_rotated_quad_is_judged_by_the_disc_it_sweeps() {
    let fb = FrameBuilder::new(FW, FH);
    // A 400-wide, 4-tall bar whose unrotated rect sits wholly below the viewport,
    // turned a quarter turn about its centre: it stands up through the bottom edge, so
    // it is kept …
    assert!(!fb.visible(100.0, FH + 10.0, 400.0, 4.0, 0.0));
    assert!(fb.visible(100.0, FH + 10.0, 400.0, 4.0, std::f32::consts::FRAC_PI_2));
    // … while a small quad far away stays culled whatever its angle.
    assert!(!fb.visible(FW + 500.0, 100.0, 4.0, 4.0, 1.0));
}

/// Parse and validate WGSL as wgpu would on a device with `capabilities` — the only
/// check a shader gets without a GPU, which CI has none of.
fn validate_wgsl(source: &str, capabilities: wgpu::naga::valid::Capabilities) {
    let module = wgpu::naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
    wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), capabilities)
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
}

#[test]
fn the_text_shaders_validate_the_lcd_one_only_with_dual_source_blending() {
    use wgpu::naga::valid::Capabilities;
    validate_wgsl(SHADER_SRC, Capabilities::default());
    validate_wgsl(&lcd_shader_source(), Capabilities::DUAL_SOURCE_BLENDING);
    // the LCD source asks for the feature: a device without it must not be handed it
    let without = wgpu::naga::front::wgsl::parse_str(&lcd_shader_source()).map(|module| {
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            Capabilities::default(),
        )
        .validate(&module)
        .is_ok()
    });
    assert!(!matches!(without, Ok(true)));
}

#[test]
fn the_mesh_shader_validates() {
    // ADR-786: the WGSL twin of `host::gui::mesh`'s shading model, checked without a device
    validate_wgsl(MESH_SRC, wgpu::naga::valid::Capabilities::default());
    // ADR-788: the scene uniform holds as many lights and shadow views as the CPU plans, and
    // `scene_uniform` packs exactly the struct the shader declares
    use crate::host::gui::mesh::{MAX_LIGHTS, MAX_SHADOW_VIEWS};
    assert!(MESH_SRC.contains(&format!("lights: array<Light, {MAX_LIGHTS}>")));
    assert!(MESH_SRC.contains(&format!("shadows: array<ShadowView, {MAX_SHADOW_VIEWS}>")));
    // ADR-805/810/816: the contact list and its depths, sized by the constant the CPU packs to (six
    // vec4 a contact), and both loops bounded by it — the WGSL spells them as literals
    use crate::host::gui::mesh::MAX_CONTACTS;
    assert!(MESH_SRC.contains(&format!("shapes: array<vec4<f32>, {}>", MAX_CONTACTS * 6)));
    assert!(MESH_SRC.contains(&format!(
        "contact_depths: array<atomic<u32>, {MAX_CONTACTS}>"
    )));
    assert_eq!(
        MESH_SRC
            .matches(&format!("min(contact_list.count.x, {MAX_CONTACTS}u)"))
            .count(),
        2
    );
    let module = wgpu::naga::front::wgsl::parse_str(MESH_SRC).expect("parses");
    let scene_type = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("SceneU"))
        .map(|(handle, _)| handle)
        .expect("SceneU");
    let mut layouter = wgpu::naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("lays out");
    let declared = layouter[scene_type].size as usize;
    let packed = scenes::scene_uniform_len();
    assert_eq!(
        declared, packed,
        "SceneU is {declared} bytes, scene_uniform packs {packed}"
    );
    // ADR-794: a draw's uniform fits the slot each draw is given, and the shader reads the
    // skin table at the size the CPU painter builds it, a texel a table entry
    let draw_type = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("DrawU"))
        .map(|(handle, _)| handle)
        .expect("DrawU");
    let draw_declared = u64::from(layouter[draw_type].size);
    assert!(
        draw_declared <= scenes::DRAW_BYTES,
        "DrawU is {draw_declared} bytes, a draw's slot {}",
        scenes::DRAW_BYTES
    );
    use crate::host::gui::mesh::{SKIN_MAX_CURVATURE, SKIN_TABLE_HEIGHT, SKIN_TABLE_WIDTH};
    assert!(MESH_SRC.contains(&format!(
        "const SKIN_TABLE_LAST = vec2<f32>({}.0, {}.0);",
        SKIN_TABLE_WIDTH - 1,
        SKIN_TABLE_HEIGHT - 1
    )));
    assert_eq!(
        SKIN_MAX_CURVATURE, 1.0,
        "the shader clamps the curvature to 1"
    );
    // ADR-834: the fibre table below the skin table's rows, its shape as the shader reads it
    {
        use crate::host::gui::fibre_table::{
            FIBRE_TABLE_ALBEDOS, FIBRE_TABLE_ANGLES, FIBRE_TABLE_ROUGHNESSES, FIBRE_TABLE_ROWS,
            table_roughness,
        };
        for line in [
            format!("const FIBRE_FIRST_ROW: i32 = {SKIN_TABLE_HEIGHT};"),
            format!("const FIBRE_ANGLES: i32 = {FIBRE_TABLE_ANGLES};"),
            format!("const FIBRE_ROUGHNESSES: i32 = {FIBRE_TABLE_ROUGHNESSES};"),
            format!("const FIBRE_ALBEDOS: i32 = {FIBRE_TABLE_ALBEDOS};"),
        ] {
            assert!(MESH_SRC.contains(&line), "the shader lacks {line}");
        }
        assert!(
            (table_roughness(0) - 0.2).abs() < 1e-12 && (table_roughness(1) - 0.3).abs() < 1e-12
        );
        assert!(MESH_SRC.contains("i32(round((roughness - 0.2) / 0.1))"));
        assert_eq!(scenes::TABLE_ROWS, SKIN_TABLE_HEIGHT + FIBRE_TABLE_ROWS);
        assert_eq!(
            scenes::table_bytes().len(),
            SKIN_TABLE_WIDTH * scenes::TABLE_ROWS * 16
        );
    }
    assert_eq!(
        scenes::skin_table_bytes().len(),
        SKIN_TABLE_WIDTH * SKIN_TABLE_HEIGHT * 16
    );
    assert_eq!(
        scenes::VERTEX_FLOATS,
        28,
        "a vertex carries its curvature, its centre of rotation, its second UV set, its lag weight and its standoff"
    );
    assert!(
        MESH_SRC.contains("let at = index * 28u;"),
        "the shader reads a vertex at the stride it is packed"
    );
    assert!(MESH_SRC.contains("@location(6) curvature: f32"));
    // ADR-797: `draw_uniform` packs exactly the struct the shader declares, the detail tile's
    // settings four floats before an eye's optics; the shader's flat texel is the CPU painter's
    use crate::host::gui::mesh::{DETAIL_FLAT, Draw, Material};
    let draw = Draw {
        loose: None,
        mesh: 1,
        model: [0.0; 16],
        palette: None,
        morphs: Vec::new(),
        material: Material {
            detail: Some(3),
            detail_scale: [60.0, 30.0],
            detail_strength: 0.5,
            detail_cavity: 0.25,
            ..Material::default()
        },
    };
    let packed = scenes::draw_uniform(&draw, [0; 4], [0; 4], [0; 4], [0.0; 4]);
    assert_eq!(
        packed.len() as u64,
        draw_declared,
        "DrawU is {draw_declared} bytes, draw_uniform packs {}",
        packed.len()
    );
    let detail: Vec<f32> = packed[176..192]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_le_bytes(*word))
        .collect();
    assert_eq!(detail, [60.0, 30.0, 0.5, 0.25]);
    assert!(MESH_SRC.contains("detail: vec4<f32>,"));
    assert!(MESH_SRC.contains("const DETAIL_FLAT = 128.0 / 255.0;"));
    assert_eq!(DETAIL_FLAT, 128.0 / 255.0);
    assert!(MESH_SRC.contains("@group(3) @binding(4) var detail_texture: texture_2d<f32>;"));
    // ADR-795: the scene uniform fits the slot each scene is given (`scene_uniform` pads to it,
    // and would cut a longer one short without a word), it holds as many emitters as the CPU
    // plans, and the shader's numbers are the CPU painter's
    use crate::host::gui::mesh::{COAT_F0, MAX_EMITTERS, MIN_ROUGHNESS, RESOLVED_EXPONENT};
    assert!(
        declared as u64 <= scenes::SCENE_BYTES,
        "SceneU is {declared} bytes, a scene's slot {}",
        scenes::SCENE_BYTES
    );
    assert_eq!(scenes::SCENE_BYTES % 256, 0, "a dynamic offset's alignment");
    assert!(MESH_SRC.contains(&format!("emitters: array<Emitter, {MAX_EMITTERS}>")));
    assert_eq!(MAX_LIGHTS, 8, "the shader's `shaping` holds a light's each");
    assert!(MESH_SRC.contains("var shaping: array<f32, 8>;"));
    let emitter_type = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Emitter"))
        .map(|(handle, _)| handle)
        .expect("Emitter");
    assert_eq!(layouter[emitter_type].size, 64, "four vec4s an emitter");
    assert_eq!(
        (COAT_F0, MIN_ROUGHNESS, RESOLVED_EXPONENT),
        (0.02, 0.04, 4096.0)
    );
    assert!(MESH_SRC.contains("let reflectances = vec3<f32>(f0, f0, 0.02);"));
    assert!(MESH_SRC.contains("fresnel(0.02, dot(view, half_vector))"));
    assert!(MESH_SRC.contains("let clamped = clamp(roughness, 0.04, 1.0);"));
    assert!(MESH_SRC.contains("if (exponent <= 4096.0 || turn <= 0.0) {"));
    // a draw's coat rides beside its scatter, the rim's old slot spare
    let coated = Draw {
        material: Material {
            coat: Some([0.5, 0.15]),
            ..Material::default()
        },
        ..draw
    };
    let packed = scenes::draw_uniform(&coated, [0; 4], [0; 4], [0; 4], [0.0; 4]);
    let floats: Vec<f32> = packed[80..112]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_le_bytes(*word))
        .collect();
    assert_eq!(
        floats[..4],
        [Material::default().scatter_scale(), 0.5, 0.15, 0.0]
    );
    // (where the rim was: a `:fibres` surface's `:density` now, its default on any other, ADR-813)
    assert_eq!(floats[6], Material::default().fibre_density);
    // (the coat's strength scaled by a skin region's oil, ADR-826; its roughness as given)
    assert!(MESH_SRC.contains("coat = vec2<f32>(draw.skin.y * region.w, draw.skin.z);"));
    // ADR-796: an eye's optics are `DrawU`'s last eight vec4s, zeros for any other surface; the
    // shader's numbers are the CPU painter's; the vertices carry the eye's frame
    use crate::host::gui::mesh::{CORNEA_EDGE, Eye, EyeAt, PUPIL_EDGE, Surface};
    let optics = Eye {
        eyes: vec![
            EyeAt {
                centre: [1.0, 2.0, 3.0],
                axis: [0.0, 0.0, 1.0],
                iris_uv: [0.3, 0.7],
                pupil_scale: 1.0,
                pupil_offset: [0.0, 0.0],
                pupil_waves: Default::default(),
            },
            EyeAt {
                centre: [-1.0, 2.0, 3.0],
                axis: [0.0, 1.0, 0.0],
                iris_uv: [0.7, 0.2],
                pupil_scale: 1.0,
                pupil_offset: [0.0, 0.0],
                pupil_waves: Default::default(),
            },
        ],
        up: [0.0, 1.0, 0.0],
        radius: 0.012,
        iris_radius: 0.006,
        cornea_radius: 0.008,
        iris_depth: 0.003,
        ior: 1.4,
        iris_uv_radius: 0.1,
        pupil: 0.3,
        concavity: 0.5,
        limbus: 0.3,
        limbus_width: 0.12,
        pupil_ruff: crate::host::gui::mesh::DEFAULT_PUPIL_RUFF,
    };
    let eye_floats = |surface: Surface| -> Vec<f32> {
        let eyed = Draw {
            loose: None,
            mesh: 1,
            model: [0.0; 16],
            palette: None,
            morphs: Vec::new(),
            material: Material {
                surface,
                eye: Some(optics.clone()),
                ..Material::default()
            },
        };
        let packed = scenes::draw_uniform(&eyed, [0; 4], [0; 4], [0; 4], [0.0; 4]);
        assert_eq!(packed.len() as u64, draw_declared);
        packed[192..192 + 32 * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| f32::from_le_bytes(*word))
            .collect()
    };
    assert_eq!(
        eye_floats(Surface::Eye),
        [
            1.0, 2.0, 3.0, 2.0, -1.0, 2.0, 3.0, 0.0, // the centres, and how many
            0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0,
            0.12, // the axes (the second's fourth: the limbal ring's width)
            0.0, 1.0, 0.0, 1.4, // up, the index
            0.012, 0.006, 0.008, 0.003, // the shape
            0.3, 0.7, 0.7, 0.2, // the irises in the texture
            0.1, 0.3, 0.5,
            0.3, // the iris's radius there, the pupil, the bowl's slope, the limbal ring's strength
        ]
    );
    assert_eq!(eye_floats(Surface::Cutout), [0.0; 32], "no other surface");
    // ADR-835: an eye's pupils in the clamp planes' place (bytes 384 … 448, after the model matrix), each its own
    let mut own = optics.clone();
    own.eyes[1].pupil_scale = 0.975;
    own.eyes[1].pupil_offset = [0.04, 0.02];
    own.eyes[1].pupil_waves = [[2.0, 0.03, std::f32::consts::PI], [3.0, -0.02, 0.0]];
    let pupils = scenes::draw_uniform(
        &Draw {
            loose: None,
            mesh: 1,
            model: [0.0; 16],
            palette: None,
            morphs: Vec::new(),
            material: Material {
                surface: Surface::Eye,
                eye: Some(own),
                clamp_planes: vec![[0.0, 1.0, 0.0, 0.5]],
                ..Material::default()
            },
        },
        [0; 4],
        [0; 4],
        [0; 4],
        [0.0; 4],
    );
    let read = |at: usize| f32::from_le_bytes(pupils[at..at + 4].try_into().expect("a float"));
    let clamp_floats: Vec<f32> = (0..16).map(|index| read(384 + index * 4)).collect();
    let ruff = crate::host::gui::mesh::DEFAULT_PUPIL_RUFF;
    assert_eq!(
        clamp_floats,
        [
            1.0, 0.0, 0.0, ruff, 0.975, 0.04, 0.02, ruff, // each eye's scale, offset, ruff
            0.0, 0.0, 0.0, 0.0, 2.5, 0.03, 3.0,
            -0.02, // each eye's waves: turns + phase's share, depth
        ]
    );
    assert_eq!(read(368), 0.0, "an eye keeps to no clamp planes");
    assert!(MESH_SRC.contains("let own = draw.clamps[which];"));
    assert_eq!(
        draw_declared, 512,
        "the eye's fields, then a hair's `:sheen` (ADR-799), `deform` (ADR-805), `loose` (ADR-812), the clamp planes (ADR-813), the wrinkle weights (ADR-817), then a hair's look (ADR-825): the whole of DRAW_BYTES"
    );
    // a hair surface's `:sheen` last, its fourth float 1 when given; its `:shift` in the
    // strand vector's spare two (ADR-799)
    let hair = Draw {
        loose: None,
        mesh: 1,
        model: [0.0; 16],
        palette: None,
        morphs: Vec::new(),
        material: Material {
            surface: Surface::Strands,
            hair_shift: [-0.1, 0.2],
            sheen: Some([0.5, 0.25, 0.125]),
            ..Material::default()
        },
    };
    let floats: Vec<f32> = scenes::draw_uniform(&hair, [0; 4], [0; 4], [0; 4], [0.0; 4])
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_le_bytes(*word))
        .collect();
    // `sheen`, then the four words of `deform` (ADR-805), the four floats of `loose` (ADR-812), the
    // clamps, the wrinkle weights and a hair's look (ADR-825)
    assert_eq!(
        floats[floats.len() - 48..floats.len() - 44],
        [0.5, 0.25, 0.125, 1.0]
    );
    assert_eq!(
        floats[42..44],
        [-0.1, 0.2],
        "strand.zw: after 32 floats, 8 words and the widths"
    );
    assert_eq!(scenes::DRAW_BYTES % 256, 0, "a dynamic offset's alignment");
    assert_eq!((PUPIL_EDGE, CORNEA_EDGE), (0.08, 0.85));
    // (the GPU's pupil edge is half a pixel, its ruff beyond it: ADR-835; `PUPIL_EDGE` the CPU's)
    assert!(MESH_SRC.contains("seen.pupil = 1.0 - smoothstep(edge - soft, edge + soft, reach);"));
    assert!(MESH_SRC.contains("let cornea = 1.0 - smoothstep(0.85, 1.0, entry_rim);"));
    assert!(MESH_SRC.contains("if (draw.flags.x == 6u && draw.eye_centres[0].w > 0.0) {"));
    assert!(MESH_SRC.contains("@location(5) eye_centre: vec4<f32>,"));
    // the iris lit as a bowl, by light bent at the eye's front: the CPU painter's arithmetic
    assert!(MESH_SRC.contains(
        "seen.iris_normal = safe_normalize(axis - slope * (right * across + up * above));"
    ));
    assert!(MESH_SRC.contains(
        "falling = ratio * to_light + (sqrt(inside) - ratio * cosine) * under_cornea.xyz;"
    ));
    assert!(MESH_SRC.contains("let_in = 1.0 - fresnel(head_on * head_on, cosine);"));
    // its clear shell is cut out of the picture and of the shadow maps, as a cutout's is
    assert!(MESH_SRC.contains("if (draw.flags.x == 2u || draw.flags.x == 6u) {"));
    assert!(
        MESH_SRC.contains("draw.flags.x == 5u || draw.flags.x == 6u) && alpha < draw.material.w")
    );
}

#[test]
fn a_draws_texture_bits_say_which_textures_it_has_and_how_each_was_uploaded() {
    // ADR-797: the shader reads these bits by number (`fs_mesh`): 1 and 2 the base texture and
    // whether it is premultiplied, 4 and 8 the normal map, 16 and 32 the occlusion map, 64 and
    // 128 the detail tile, 256 that the tile's normal tilts (the mesh has tangents)
    use scenes::texture_bits;
    assert_eq!(texture_bits([None; 4], true), 0);
    assert_eq!(texture_bits([Some(false), None, None, None], false), 1);
    assert_eq!(texture_bits([Some(true), None, None, None], false), 3);
    assert_eq!(texture_bits([None, Some(true), None, None], true), 12);
    assert_eq!(texture_bits([None, None, Some(false), None], true), 16);
    assert_eq!(
        texture_bits([None, None, None, Some(true)], true),
        64 | 128 | 256
    );
    assert_eq!(
        texture_bits([None, None, None, Some(false)], true),
        64 | 256
    );
    // without tangents the tile's cavity is still read, its normal not
    assert_eq!(
        texture_bits([None, None, None, Some(true)], false),
        64 | 128
    );
    assert_eq!(texture_bits([Some(true); 4], true), 511);
    for bit in ["4u", "8u", "16u", "32u", "64u", "128u", "256u"] {
        assert!(
            MESH_SRC.contains(&format!("(draw.flags.y & {bit}) != 0u")),
            "the shader reads bit {bit}"
        );
    }
}

#[test]
fn a_glyph_mode_says_colour_grey_or_lcd_and_whether_it_is_lifted() {
    assert_eq!(glyph_mode(false, false, true), 0); // an emoji draws in its own colours
    assert_eq!(glyph_mode(false, true, true), 0);
    assert_eq!(glyph_mode(true, false, false), 1);
    assert_eq!(glyph_mode(true, false, true), 2);
    assert_eq!(glyph_mode(true, true, false), 3);
    assert_eq!(glyph_mode(true, true, true), 4);
}

#[test]
fn lcd_glyphs_batch_apart_from_grey_ones_on_the_same_atlas_page() {
    // they draw with different pipelines, so one batch cannot hold both
    let mut fb = FrameBuilder::new(FW, FH);
    let uv = [0.0, 0.0, 0.1, 0.1];
    fb.textured(
        TexKey::Atlas(0),
        0.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        3,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::Atlas(0),
        8.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        4,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::Atlas(0),
        16.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        1,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::Atlas(0),
        24.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        3,
        0.0,
        0.0,
    );
    let shape: Vec<(u32, u32, bool)> = fb
        .batches
        .iter()
        .map(|b| match b {
            Batch::Tex {
                first, count, lcd, ..
            } => (*first, *count, *lcd),
            _ => unreachable!("only glyphs in this frame"),
        })
        .collect();
    assert_eq!(shape, vec![(0, 2, true), (2, 1, false), (3, 1, true)]);
}

#[test]
fn a_smooth_texture_is_kept_premultiplied_and_drawn_in_its_own_mode() {
    // premultiplied at upload: colour scaled by alpha (rounded), alpha kept; an opaque texel
    // is untouched and a transparent one loses its colour, which is what keeps the linear
    // sampler from mixing a transparent pixel's black into an edge (ADR-783)
    assert_eq!(
        premultiplied(&[255, 255, 255, 255, 200, 100, 50, 128, 255, 0, 0, 0]),
        vec![255, 255, 255, 255, 100, 50, 25, 128, 0, 0, 0, 0]
    );
    // nearest sprites keep the straight colour mode; smooth ones the premultiplied one,
    // which the shader turns back to straight alpha before the blend
    assert_eq!(sprite_mode(TextureFilter::Nearest), 0);
    assert_eq!(sprite_mode(TextureFilter::Smooth), 5);
    // mode 5 is colour, not subpixel text: it batches with the texture's other sprites
    // and draws through the plain textured pipeline
    let mut fb = FrameBuilder::new(FW, FH);
    for x in [0.0, 10.0] {
        fb.textured(
            TexKey::User(1),
            x,
            0.0,
            8.0,
            8.0,
            [0.0, 0.0, 1.0, 1.0],
            [255; 4],
            sprite_mode(TextureFilter::Smooth),
            0.1,
            0.0,
        );
    }
    let shape: Vec<(u32, bool)> = fb
        .batches
        .iter()
        .map(|batch| match batch {
            Batch::Tex { count, lcd, .. } => (*count, *lcd),
            _ => unreachable!("only sprites in this frame"),
        })
        .collect();
    assert_eq!(shape, vec![(2, false)]);
}

#[test]
fn consecutive_quads_of_one_kind_form_one_batch_and_a_kind_change_starts_another() {
    let mut fb = FrameBuilder::new(FW, FH);
    fb.solid(0.0, 0.0, 10.0, 10.0, [255, 0, 0, 255], 0.0, Edge::Plain);
    fb.solid(20.0, 0.0, 10.0, 10.0, [255, 0, 0, 255], 0.0, Edge::Plain);
    let uv = [0.0, 0.0, 1.0, 1.0];
    fb.textured(
        TexKey::User(7),
        0.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        0,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::User(7),
        8.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        0,
        0.0,
        0.0,
    );
    fb.textured(
        TexKey::User(9),
        16.0,
        0.0,
        8.0,
        8.0,
        uv,
        [255; 4],
        0,
        0.0,
        0.0,
    );
    fb.solid(40.0, 0.0, 10.0, 10.0, [0, 255, 0, 255], 0.0, Edge::Plain);
    let shape: Vec<(bool, u32, u32)> = fb
        .batches
        .iter()
        .map(|b| match b {
            Batch::Solid { first, count, .. } => (true, *first, *count),
            Batch::Tex { first, count, .. } => (false, *first, *count),
            Batch::Particles { .. } => unreachable!("no emitter in this frame"),
        })
        .collect();
    // Two solids → one batch; two sprites of texture 7 → one; texture 9 → its own;
    // the trailing solid a fourth, continuing the solid buffer at index 2.
    assert_eq!(
        shape,
        vec![(true, 0, 2), (false, 0, 2), (false, 2, 1), (true, 2, 1)]
    );
    assert_eq!(fb.solids.len(), 3 * SOLID_FLOATS);
    assert_eq!(fb.texs.len(), 3 * TEX_WORDS);
}

#[test]
fn a_clip_band_culls_what_lies_outside_it_and_rides_on_every_quad() {
    let mut fb = FrameBuilder::new(FW, FH);
    fb.clip = [0.0, 100.0, FW, 200.0];
    // Wholly above the band: culled. Straddling it: kept, carrying the band.
    fb.solid(10.0, 10.0, 10.0, 10.0, [255; 4], 0.0, Edge::Plain);
    assert!(fb.batches.is_empty());
    fb.solid(10.0, 95.0, 10.0, 10.0, [255; 4], 0.0, Edge::Plain);
    assert_eq!(fb.solids.len(), SOLID_FLOATS);
    assert_eq!(&fb.solids[SOLID_FLOATS - 4..], &[0.0, 100.0, FW, 200.0]);
    // A rounded edge is flagged for the SDF path with its radius.
    fb.solid(10.0, 120.0, 10.0, 10.0, [255; 4], 0.0, Edge::Rounded(3.0));
    assert_eq!(&fb.solids[SOLID_FLOATS + 9..SOLID_FLOATS + 11], &[3.0, 1.0]);
}

#[test]
fn cursor_quads_follow_the_cpu_geometry() {
    // Block: the overlay plus four rim bars, 2 px thick for a 10 px cell.
    let block = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Block, None);
    assert_eq!(block.len(), 5);
    assert_eq!(block[0], (100.0, 50.0, 10.0, 20.0, [255, 255, 255, 128]));
    assert_eq!(block[2].0, 108.0); // the right rim at cw - t
    // Bar: one caret on the left edge, at least 2 px.
    let bar = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Bar, None);
    assert_eq!(bar.len(), 1);
    assert_eq!((bar[0].2, bar[0].3), (2.0, 20.0));
    // Underline: one rule along the cell bottom.
    let under = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Underline, None);
    assert_eq!(under[0].1, 68.0);
    // Hollow: the rim alone, 1 px for a 10 px cell, in the op's colour.
    let hollow = cursor_quads(
        100.0,
        50.0,
        10.0,
        20.0,
        CursorStyle::Hollow,
        Some([1, 2, 3]),
    );
    assert_eq!(hollow.len(), 4);
    assert_eq!(hollow[0], (100.0, 50.0, 1.0, 20.0, [1, 2, 3, 255]));
    // A coloured block washes the cell in that colour at half alpha.
    let tinted = cursor_quads(100.0, 50.0, 10.0, 20.0, CursorStyle::Block, Some([9, 8, 7]));
    assert_eq!(tinted[0].4, [9, 8, 7, 128]);
}

#[test]
fn a_scroll_shift_is_whole_pixels_and_capped() {
    assert_eq!(scroll_px(0.5, 16.0), 8.0);
    assert_eq!(scroll_px(-0.26, 16.0), -4.0);
    assert_eq!(scroll_px(1.0e9, 16.0), MAX_SCROLL_PX);
}

#[test]
fn no_batch_merges_across_the_overlay() {
    let mut fb = FrameBuilder::new(100.0, 100.0);
    fb.solid(0.0, 0.0, 10.0, 10.0, [255, 0, 0, 255], 0.0, Edge::Plain);
    fb.overlay_at = Some(fb.batches.len());
    fb.solid(20.0, 0.0, 10.0, 10.0, [0, 255, 0, 255], 0.0, Edge::Plain);
    fb.solid(40.0, 0.0, 10.0, 10.0, [0, 0, 255, 255], 0.0, Edge::Plain);
    let counts: Vec<u32> = fb
        .batches
        .iter()
        .map(|b| match b {
            Batch::Solid { count, .. } => *count,
            _ => 0,
        })
        .collect();
    assert_eq!(
        counts,
        vec![1, 2],
        "the scene's run ends at the overlay; the overlay's merge"
    );
}

#[test]
fn a_step_is_the_shaders_params_word_for_word() {
    let step = EmitterStep {
        id: 1,
        x: 3.0,
        y: 4.0,
        spec: crate::host::gui::EmitterSpec::default(),
        dt: 0.5,
        spawn_first: 6,
        spawn_count: 7,
        seed: 8,
        present: true,
    };
    let words = params_words(&step, 9);
    assert_eq!(words.len() as u64 * 4, PARTICLE_PARAMS_BYTES);
    assert_eq!(f32::from_bits(words[0]), 3.0);
    assert_eq!(f32::from_bits(words[20]), 0.5, "dt");
    assert_eq!(
        &words[22..26],
        &[6, 7, 9, 8],
        "spawn_first, spawn_count, capacity, seed"
    );
}

#[test]
fn a_culled_quad_leaves_no_instance_and_no_batch() {
    let mut fb = FrameBuilder::new(FW, FH);
    fb.solid(-100.0, -100.0, 10.0, 10.0, [255; 4], 0.0, Edge::Plain);
    assert!(fb.batches.is_empty());
    assert!(fb.solids.is_empty());
}

#[test]
fn a_non_srgb_format_is_preferred_over_an_srgb_one() {
    use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba16Float};
    assert_eq!(
        pick_format(&[Bgra8UnormSrgb, Rgba16Float, Bgra8Unorm]),
        Some(Bgra8Unorm)
    );
    // Only sRGB on offer: the first supported one, rather than no window at all.
    assert_eq!(pick_format(&[Bgra8UnormSrgb]), Some(Bgra8UnormSrgb));
    assert_eq!(pick_format(&[]), None);
}

#[test]
fn the_default_is_mailbox_no_vsync_may_tear_and_vsync_is_fifo() {
    use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
    let all = [Fifo, Mailbox, Immediate];
    // the default never tears: mailbox, else fifo — never immediate
    assert_eq!(pick_present_mode(&all, None), Mailbox);
    assert_eq!(pick_present_mode(&[Fifo, Immediate], None), Fifo);
    // `:vsync false` asks for the work-bound rate
    assert_eq!(pick_present_mode(&all, Some(false)), Immediate);
    assert_eq!(pick_present_mode(&[Fifo, Mailbox], Some(false)), Mailbox);
    assert_eq!(pick_present_mode(&[Fifo], Some(false)), Fifo);
    // `:vsync true` is fifo
    assert_eq!(pick_present_mode(&all, Some(true)), Fifo);
}

// --- a pixel-space quad's radius and outline (ADR-832) ---------------------------------------

/// The solid shader's `solid_coverage` (shaders.rs) for one pixel, mirrored: `local` the pixel
/// centre's offset from the quad's top left, `half` its half size, `radius` and `aa` as the
/// instance carries them (`FrameBuilder::solid`). What the GPU computes, so its arithmetic is
/// tested here where there is no GPU.
fn solid_coverage(local: (f32, f32), half: (f32, f32), radius: f32, aa: f32) -> f32 {
    if aa == 0.0 {
        return 1.0;
    }
    let (px, py) = (local.0 - half.0, local.1 - half.1);
    let r = radius.min(half.0.min(half.1));
    let (qx, qy) = (px.abs() - (half.0 - r), py.abs() - (half.1 - r));
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let d = outside + qx.max(qy).min(0.0) - r;
    if aa > 1.0 {
        let width = aa - 1.0;
        let band = (d + 0.5 * width).abs() - 0.5 * width;
        return (0.5 - band).clamp(0.0, 1.0);
    }
    (0.5 - d).clamp(0.0, 1.0)
}

/// The summed coverage of a `side`×`side` quad's pixels, as the shader would cover them.
fn covered_area(side: usize, radius: f32, aa: f32) -> f32 {
    let half = side as f32 * 0.5;
    let mut sum = 0.0;
    for y in 0..side {
        for x in 0..side {
            sum += solid_coverage((x as f32 + 0.5, y as f32 + 0.5), (half, half), radius, aa);
        }
    }
    sum
}

#[test]
fn a_quad_with_a_radius_is_a_smooth_disc_and_with_an_outline_a_ring() {
    let pi = std::f32::consts::PI;
    // a 40 px quad of radius 20: a disc, its area πr² to within half a percent
    let disc = covered_area(40, 20.0, 1.0);
    assert!(
        (disc - pi * 400.0).abs() < 0.005 * pi * 400.0,
        "disc area {disc}"
    );
    // its edge anti-aliased: the pixel the circle crosses through its middle is half covered
    let edge = solid_coverage((40.0, 20.0), (20.0, 20.0), 20.0, 1.0);
    assert!((edge - 0.5).abs() < 0.01, "edge coverage {edge}");
    // a 3 px outline of it: the band between radius 17 and 20
    let ring = covered_area(40, 20.0, 1.0 + 3.0);
    let band = pi * (400.0 - 289.0);
    assert!(
        (ring - band).abs() < 0.02 * band,
        "ring area {ring} against {band}"
    );
    // and a plain fill covers every pixel
    assert_eq!(covered_area(10, 0.0, 0.0), 100.0);
}

#[test]
fn a_quad_op_takes_its_edge_from_its_radius_and_outline() {
    let mut fb = FrameBuilder::new(FW, FH);
    // an outline: its radius, and 1 + its width in the `aa` slot
    fb.solid(
        10.0,
        10.0,
        20.0,
        20.0,
        [255; 4],
        0.0,
        Edge::Outline(10.0, 2.5),
    );
    assert_eq!(&fb.solids[9..11], &[10.0, 3.5]);
}

// --- the surroundings' bounce in the occluded ambient (ADR-831) ------------------------------

#[test]
fn surroundings_ride_in_the_sheen_slot_of_a_surface_that_is_not_hair() {
    use crate::host::gui::mesh::{Draw, Material, Surface};
    let floats = |material: Material| -> Vec<f32> {
        let draw = Draw {
            loose: None,
            mesh: 1,
            model: [0.0; 16],
            palette: None,
            morphs: Vec::new(),
            material,
        };
        scenes::draw_uniform(&draw, [0; 4], [0; 4], [0; 4], [0.0; 4])
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| f32::from_le_bytes(*word))
            .collect()
    };
    let slot = |all: Vec<f32>| -> Vec<f32> { all[all.len() - 48..all.len() - 44].to_vec() };
    // teeth: their surroundings, given
    let teeth = floats(Material {
        surroundings: Some([0.39, 0.14, 0.12]),
        sheen: Some([1.0, 1.0, 1.0]),
        ..Material::default()
    });
    assert_eq!(slot(teeth), [0.39, 0.14, 0.12, 1.0]);
    // hair: its sheen, whatever surroundings say
    let hair = floats(Material {
        surface: Surface::Strands,
        surroundings: Some([0.39, 0.14, 0.12]),
        sheen: Some([0.5, 0.25, 0.125]),
        ..Material::default()
    });
    assert_eq!(slot(hair), [0.5, 0.25, 0.125, 1.0]);
    // neither: nothing, and the shader leaves the occlusion as it was
    assert_eq!(slot(floats(Material::default())), [0.0; 4]);
    assert!(
        MESH_SRC
            .contains("fn bounced_occlusion(visibility: f32, albedo: vec3<f32>) -> vec3<f32> {")
    );
    assert!(
        MESH_SRC.contains("if (draw.flags.x != 4u && draw.flags.x != 5u && draw.sheen.w > 0.5) {")
    );
}

#[test]
fn the_bounce_is_never_darker_than_the_occlusion_and_tinted_by_the_surroundings() {
    // the shader's fit (GTAO), here as the shader has it
    let bounced = |visibility: f32, albedo: f32| -> f32 {
        let a = 2.0404 * albedo - 0.3324;
        let b = -4.7951 * albedo + 0.6417;
        let c = 2.7552 * albedo + 0.6903;
        visibility.max(((a * visibility + b) * visibility + c) * visibility)
    };
    assert!(MESH_SRC.contains("let a = 2.0404 * albedo - vec3<f32>(0.3324);"));
    for visibility in [0.0, 0.1, 0.3, 0.6, 1.0] {
        let [red, green, blue] = [0.39, 0.14, 0.12].map(|albedo| bounced(visibility, albedo));
        assert!(red >= visibility && green >= visibility && blue >= visibility);
        assert!(
            red >= green && green >= blue,
            "the gums' red given back most: {visibility}"
        );
        assert!(red <= 1.0 + 1e-5);
    }
    // nothing occluded: nothing to give back
    assert!((bounced(1.0, 0.39) - 1.0).abs() < 1e-4);
    // a dark corner of the mouth (visibility 0.2): warm, red over green by more than 1.15
    let (red, green) = (bounced(0.2, 0.39), bounced(0.2, 0.14));
    assert!(red / green > 1.15, "{red} / {green}");
}

#[test]
fn a_light_without_a_shadow_map_still_has_its_contact_shadow() {
    // ADR-818, amended 2026-11-27: every light but on skin and hair
    assert!(
        MESH_SRC.contains(
            "} else if (draw.flags.x != 0u && draw.flags.x != 4u && draw.flags.x != 5u) {"
        )
    );
    assert!(MESH_SRC.contains("            contact = contact_shadow(world, arrival.to_light, surface_normal, view);\n            seen = contact;"));
}
