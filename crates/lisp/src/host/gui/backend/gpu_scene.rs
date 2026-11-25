//! A morphed `[:scene …]` on the GPU against the CPU painter, on a real device with no window.
//!
//! Morph targets are kept sparse and blended on the CPU into one offset a vertex, which the GPU
//! target uploads each frame and its vertex shader reads (`mesh::add_morphs`); nothing in the
//! pure tests creates the pipelines or binds that buffer, so this draws a triangle that ten
//! targets (more than the old cap of eight) move across the picture, on both painters, and
//! checks it landed where all ten put it. A pipeline whose layouts do not match the shader, or
//! a bind group that does not match its layout, fails here as wgpu's validation error.
//!
//! A machine without a GPU (CI) runs the CPU half, and says it skipped the GPU one.

use std::sync::Arc;

use super::fonts::default_families;
use super::render::Renderer;
use crate::host::gui::Op;
use crate::host::gui::gpu::{GpuWindow, hardware_adapters};
use crate::host::gui::mesh::{
    AmbientOcclusion, Draw, Light, Material, Mesh, MeshParts, Scene, Shadow, Surface, rasterize,
};

const SIDE: u32 = 64;
const TARGETS: u32 = 10;

fn bytes_of(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// A triangle in the left half of clip space, facing the camera, and `TARGETS` morphs that
/// each move it 0.08 to the right: all of them together put it in the right half.
fn mesh() -> Mesh {
    let shift = [0.08, 0.0, 0.0];
    Mesh::from_parts(MeshParts {
        positions: bytes_of(&[-0.9, -0.9, 0.5, -0.1, -0.9, 0.5, -0.5, 0.9, 0.5]),
        normals: bytes_of(&[0.0, 0.0, -1.0, 0.0, 0.0, -1.0, 0.0, 0.0, -1.0]),
        morphs: (0..TARGETS)
            .map(|_| (bytes_of(&[shift, shift, shift].concat()), Vec::new()))
            .collect(),
        ..MeshParts::default()
    })
    .expect("a whole triangle")
}

fn scene() -> Scene {
    let mut identity = [0.0f32; 16];
    for axis in 0..4 {
        identity[axis * 5] = 1.0;
    }
    Scene {
        x: 0.0,
        y: 0.0,
        w: SIDE as f32,
        h: SIDE as f32,
        view_proj: identity,
        eye: [0.0, 0.0, -10.0],
        lights: vec![Light::directional([0.0, 0.0, 1.0], [1.0, 1.0, 1.0])],
        sky: [0.1, 0.1, 0.1],
        ground: [0.1, 0.1, 0.1],
        ambient_sh: None,
        exposure: 1.0,
        ambient_occlusion: None,
        emitters: Vec::new(),
        background: [0.0, 0.0, 0.0, 0.0],
        palettes: vec![],
        contacts: Vec::new(),
        contact_drags: Vec::new(),
        contact_skips: Vec::new(),
        draws: vec![Draw {
            mesh: 1,
            model: identity,
            palette: None,
            morphs: (0..TARGETS).map(|target| (target, 1.0)).collect(),
            material: Material {
                surface: Surface::Standard,
                color: [1.0, 0.0, 0.0, 1.0],
                double_sided: true,
                ..Material::default()
            },
        }],
    }
}

/// Where the triangle was (x −0.5) and where its morphs put it (x +0.3), a quarter of the way
/// up from the bottom: as `(column, row)` in the picture.
const WAS: (u32, u32) = (16, 40);
const IS: (u32, u32) = (41, 40);

fn red(pixels: &[u8], stride: usize, (column, row): (u32, u32)) -> u8 {
    pixels[(row * SIDE + column) as usize * stride]
}

#[test]
fn a_morphed_scene_draws_where_every_target_puts_it_on_both_painters() {
    let mesh = Arc::new(mesh());
    let scene = scene();
    let (_, _, cpu) = rasterize(&scene, &|_| Some(mesh.clone()), &|_| None);
    assert!(
        red(&cpu, 4, IS) > 80,
        "the CPU painter draws it where its morphs put it"
    );
    assert!(
        red(&cpu, 4, WAS) < 30,
        "the CPU painter leaves where it was"
    );

    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let mut gpu = match GpuWindow::offscreen(adapter, SIDE, SIDE) {
            Ok(gpu) => gpu,
            Err(error) => {
                eprintln!("skipped {name}: {error}");
                continue;
            }
        };
        gpu.upload_mesh(1, &mesh);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let pixels = gpu.read_pixels();
        let (is, was) = (red(&pixels, 3, IS), red(&pixels, 3, WAS));
        eprintln!("{name}: red where its morphs put it {is}, where it was {was}");
        if is <= 80 || was >= 30 {
            failures.push(format!(
                "{name}: red {is} where its {TARGETS} morphs put it, {was} where it was"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What a morphed figure costs a GPU frame at a real figure's size (womanblend's body: 70,737
/// vertices, 37 face targets each moving about 2,500 of them, scattered), lit by a shadowed
/// spot, at 900×1000: each frame painted and read back (so it is finished), for 0, 8, 16 and 37
/// targets moving at once. The difference from 0 is what the morphs cost: the CPU blending them
/// into the frame's offsets, the upload, and one read a vertex a pass.
/// `cargo test --release -p brood --features gui-gpu --lib -- --ignored gpu_morph_cost --nocapture`
#[test]
#[ignore = "a timing, run by hand"]
fn gpu_morph_cost_at_her_size() {
    // a grid of 266 × 266 vertices over the picture: 70,756, near her body's count
    let across = 266usize;
    let vertex_count = across * across;
    let mut positions = Vec::with_capacity(vertex_count * 3);
    let mut normals = Vec::with_capacity(vertex_count * 3);
    for row in 0..across {
        for column in 0..across {
            let at = |step: usize| step as f32 / (across - 1) as f32 * 1.8 - 0.9;
            positions.extend_from_slice(&[at(column), at(row), 0.5]);
            normals.extend_from_slice(&[0.0, 0.0, -1.0]);
        }
    }
    let mut indices: Vec<u8> = Vec::new();
    for row in 0..across - 1 {
        for column in 0..across - 1 {
            let corner = (row * across + column) as u32;
            let right = corner + 1;
            let below = corner + across as u32;
            for index in [corner, right, below, right, below + 1, below] {
                indices.extend(index.to_le_bytes());
            }
        }
    }
    let moved_each = 2_500usize;
    let morphs = (0..37usize)
        .map(|target| {
            let mut offsets = vec![0.0f32; vertex_count * 3];
            let stride = vertex_count / moved_each;
            for step in 0..moved_each {
                let vertex = (target * 7 + step * stride) % vertex_count;
                offsets[vertex * 3 + 2] = -0.001;
            }
            (bytes_of(&offsets), Vec::new())
        })
        .collect();
    let mesh = Mesh::from_parts(MeshParts {
        positions: bytes_of(&positions),
        normals: bytes_of(&normals),
        indices,
        indices_wide: true,
        morphs,
        ..MeshParts::default()
    })
    .expect("a whole grid");
    let (width, height) = (900u32, 1000u32);
    let mut light = Light::directional([0.0, 0.0, 1.0], [1.0, 1.0, 1.0]);
    light.shadow = Some(crate::host::gui::mesh::Shadow::default());
    let scene_with = |count: u32| {
        let mut scene = scene();
        scene.w = width as f32;
        scene.h = height as f32;
        scene.lights = vec![light.clone()];
        scene.draws[0].morphs = (0..count).map(|target| (target, 0.5)).collect();
        Arc::new(scene)
    };
    for adapter in &hardware_adapters() {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, width, height) else {
            continue;
        };
        gpu.upload_mesh(1, &mesh);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        let mut report = Vec::new();
        for count in [0u32, 8, 16, 37] {
            let frame = [Op::Scene(scene_with(count))];
            // warmed up, then timed
            for _ in 0..5 {
                gpu.paint(&frame, &mut renderer).expect("painted");
                gpu.read_pixels();
            }
            let rounds = 40;
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                gpu.paint(&frame, &mut renderer).expect("painted");
                std::hint::black_box(gpu.read_pixels());
            }
            report.push(format!(
                "{count}: {:.2} ms",
                start.elapsed().as_secs_f64() * 1000.0 / rounds as f64
            ));
        }
        eprintln!(
            "GPU MORPH COST {name}, a frame with N targets: {}",
            report.join(", ")
        );
    }
}

/// A flat square at `height`, `half` metres each way about the origin, facing up.
fn square(height: f32, half: f32) -> Mesh {
    let corners = [[-half, half], [half, half], [half, -half], [-half, -half]];
    let positions: Vec<f32> = corners.iter().flat_map(|[x, z]| [*x, height, *z]).collect();
    let normals: Vec<f32> = (0..4).flat_map(|_| [0.0, 1.0, 0.0]).collect();
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
        .iter()
        .flat_map(|index| index.to_le_bytes())
        .collect();
    Mesh::from_parts(MeshParts {
        positions: bytes_of(&positions),
        normals: bytes_of(&normals),
        indices,
        ..MeshParts::default()
    })
    .expect("a whole square")
}

#[test]
fn a_soft_shadow_draws_on_the_gpu_as_on_the_cpu() {
    soft_shadow_on_both_painters(Vec::new());
}

#[test]
fn a_soft_shadow_from_the_smaller_map_array_draws_on_the_gpu_as_on_the_cpu() {
    // ADR-800: a dark light with a bigger map takes array 0, so the card's light's 1024 map is
    // kept and read in array 1
    let mut bigger = Light::directional([1.0, -1.0, 0.0], [0.0, 0.0, 0.0]);
    bigger.shadow = Some(Shadow {
        resolution: 2048,
        ..Shadow::default()
    });
    soft_shadow_on_both_painters(vec![bigger]);
}

/// The soft shadow parity scene, with `more` lights after the one that casts it.
fn soft_shadow_on_both_painters(more: Vec<Light>) {
    // seen from above: a 1 m card a metre over an 8 m floor, lit at 45° toward −x, so its
    // shadow lies on x −1.5 … −0.5, its edges filtered over 0.3 m
    let side = 128u32;
    let floor = Arc::new(square(0.0, 4.0));
    let card = Arc::new(square(1.0, 0.5));
    let mut light = Light::directional([-1.0, -1.0, 0.0], [1.0, 1.0, 1.0]);
    light.shadow = Some(Shadow {
        softness: 0.3,
        ..Shadow::default()
    });
    let material = Material {
        double_sided: true,
        specular: 0.0,
        ..Material::default()
    };
    // x → clip x / 4, z → clip −y / 4, height → depth
    let view_proj = [
        0.25, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.view_proj = view_proj;
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![light];
    scene.lights.extend(more);
    let draw = |mesh: u32| Draw {
        mesh,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: material.clone(),
    };
    scene.draws = vec![draw(1), draw(2)];
    let meshes = |handle: u32| match handle {
        1 => Some(floor.clone()),
        2 => Some(card.clone()),
        _ => None,
    };
    let (_, _, cpu) = rasterize(&scene, &meshes, &|_| None);
    let at = |x: f32| (((64.0 + 16.0 * x) as u32), 64u32);
    let red_at = |pixels: &[u8], stride: usize, x: f32| {
        let (column, row) = at(x);
        i32::from(pixels[(row * side + column) as usize * stride])
    };
    let points = [-1.0f32, -1.35, -1.65, 2.5];
    let cpu_reds: Vec<i32> = points.iter().map(|x| red_at(&cpu, 4, *x)).collect();
    assert!(
        cpu_reds[0] + 40 < cpu_reds[3],
        "the CPU painter's shadow is dark in its middle: {cpu_reds:?}"
    );
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &floor);
        gpu.upload_mesh(2, &card);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let pixels = gpu.read_pixels();
        let gpu_reds: Vec<i32> = points.iter().map(|x| red_at(&pixels, 3, *x)).collect();
        eprintln!("{name}: soft shadow reds cpu {cpu_reds:?} gpu {gpu_reds:?}");
        // the same filter, turned by the same hash: within the grain of the two painters
        if cpu_reds
            .iter()
            .zip(&gpu_reds)
            .any(|(cpu, gpu)| (cpu - gpu).abs() > 20)
        {
            failures.push(format!("{name}: GPU {gpu_reds:?} against CPU {cpu_reds:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ambient_occlusion_draws_on_the_gpu_as_on_the_cpu() {
    // seen from above, lit by its ambient alone: a 1 m card lying 10 cm over an 8 m floor, the
    // floor beside its edges darkened over 0.3 m, open floor untouched
    let side = 128u32;
    let floor = Arc::new(square(0.0, 4.0));
    let card = Arc::new(square(0.1, 0.5));
    let material = Material {
        double_sided: true,
        specular: 0.0,
        ..Material::default()
    };
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    // x → clip x / 4, z → clip −y / 4, height → depth
    scene.view_proj = [
        0.25, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![];
    scene.sky = [0.5, 0.5, 0.5];
    scene.ground = [0.5, 0.5, 0.5];
    scene.ambient_occlusion = Some(AmbientOcclusion {
        radius: 0.3,
        strength: 1.0,
    });
    let draw = |mesh: u32| Draw {
        mesh,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: material.clone(),
    };
    scene.draws = vec![draw(1), draw(2)];
    let meshes = |handle: u32| match handle {
        1 => Some(floor.clone()),
        2 => Some(card.clone()),
        _ => None,
    };
    let (_, _, cpu) = rasterize(&scene, &meshes, &|_| None);
    let red_at = |pixels: &[u8], stride: usize, x: f32| {
        let (column, row) = ((64.0 + 16.0 * x) as u32, 64u32);
        i32::from(pixels[(row * side + column) as usize * stride])
    };
    let points = [-0.55f32, -0.75, 0.0, -2.5];
    let cpu_reds: Vec<i32> = points.iter().map(|x| red_at(&cpu, 4, *x)).collect();
    assert!(
        cpu_reds[0] + 5 < cpu_reds[3],
        "the CPU painter darkens the floor beside the card: {cpu_reds:?}"
    );
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &floor);
        gpu.upload_mesh(2, &card);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let pixels = gpu.read_pixels();
        let gpu_reds: Vec<i32> = points.iter().map(|x| red_at(&pixels, 3, *x)).collect();
        eprintln!("{name}: ambient occlusion reds cpu {cpu_reds:?} gpu {gpu_reds:?}");
        // the same taps against the same depth view: within the two painters' rounding
        if cpu_reds
            .iter()
            .zip(&gpu_reds)
            .any(|(cpu, gpu)| (cpu - gpu).abs() > 12)
        {
            failures.push(format!("{name}: GPU {gpu_reds:?} against CPU {cpu_reds:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn hair_covers_and_shines_on_the_gpu_as_on_the_cpu() {
    // seen from above: a 1 m hair card over nothing, half covered (alpha at its cutoff) and
    // fully covered, lit and seen straight down, its strands running along z (UV v): what
    // reaches the picture over black, both painters
    let side = 128u32;
    let card = Arc::new(square(0.0, 0.5));
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.view_proj = [
        0.25, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0])];
    let hair = |opacity: f32| Draw {
        mesh: 1,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: Material {
            surface: Surface::Hair,
            double_sided: true,
            cutoff: 0.5,
            roughness: 0.4,
            specular: 0.5,
            color: [0.8, 0.6, 0.3, opacity],
            ..Material::default()
        },
    };
    let meshes = |handle: u32| (handle == 1).then(|| card.clone());
    let centre = ((64 * side + 64) * 4) as usize;
    let mut cases = Vec::new();
    for opacity in [0.5f32, 1.0] {
        scene.draws = vec![hair(opacity)];
        let (_, _, cpu) = rasterize(&scene, &meshes, &|_| None);
        // straight alpha over black, as the GPU frame shows it
        let red = i32::from(cpu[centre]) * i32::from(cpu[centre + 3]) / 255;
        cases.push((opacity, scene.clone(), red));
    }
    assert!(
        cases[0].2 * 2 < cases[1].2 + 8 && cases[0].2 * 2 > cases[1].2 - 8,
        "half covered is half as bright over black: {} against {}",
        cases[0].2,
        cases[1].2
    );
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        for (opacity, scene, cpu_red) in &cases {
            let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
                continue;
            };
            gpu.upload_mesh(1, &card);
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            let pixels = gpu.read_pixels();
            let gpu_red = i32::from(pixels[((64 * side + 64) * 3) as usize]);
            eprintln!("{name}: hair at opacity {opacity}: cpu {cpu_red} gpu {gpu_red}");
            if (cpu_red - gpu_red).abs() > 12 {
                failures.push(format!(
                    "{name}: opacity {opacity}: GPU {gpu_red} against CPU {cpu_red}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The scene passes' timestamps (ADR-801): a few frames drawn, the card's time on the scene is
/// read back into `scene_gpu_times`, never waited on, and is a time a frame could take.
#[test]
fn a_scene_is_timed_on_the_gpu_a_few_frames_late() {
    use crate::host::gui::scene_gpu_times::scene_gpu_times;
    let adapters: Vec<_> = hardware_adapters()
        .into_iter()
        .filter(|adapter| adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY))
        .collect();
    if adapters.is_empty() {
        eprintln!("skipped: no hardware GPU adapter with timestamp queries here");
        return;
    }
    let mesh = mesh();
    let scene = scene();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let mut gpu = match GpuWindow::offscreen(adapter, SIDE, SIDE) {
            Ok(gpu) => gpu,
            Err(error) => {
                eprintln!("skipped {name}: {error}");
                continue;
            }
        };
        gpu.upload_mesh(1, &mesh);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        let before = scene_gpu_times().len();
        // each read-back waits for the card, so the frame after it finds the stamps mapped
        for _ in 0..4 {
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            gpu.read_pixels();
        }
        let times = scene_gpu_times();
        let newest = times.last().copied();
        eprintln!(
            "{name}: {} frames timed, newest {newest:?}",
            times.len() - before.min(times.len())
        );
        let newest = newest.unwrap_or_else(|| panic!("{name}: no frame was timed"));
        assert_eq!(newest.scenes, 1, "{name}: one scene a frame");
        assert!(
            newest.scene_us > 0.0 && newest.scene_us < 1_000_000.0,
            "{name}: the scene pass took {} us",
            newest.scene_us
        );
    }
}

/// ADR-805: a soft slab facing the camera with a ball pressed 5 cm into it. Its skin is laid onto
/// the ball (a bowl, facing into it) and rises in a ring round it, on both painters alike; the
/// same slab unpressed (no contacts, or not `:deform`) is drawn as before.
#[test]
fn a_ball_pressed_into_soft_skin_dents_it_alike_on_both_painters() {
    use crate::host::gui::mesh::Contact;
    let side = 96u32;
    // a 1 × 1 slab at z = 0.5 facing the camera (−z), 48 × 48 squares, all soft
    let cells = 48usize;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    for row in 0..=cells {
        for column in 0..=cells {
            let x = column as f32 / cells as f32 - 0.5;
            let y = row as f32 / cells as f32 - 0.5;
            positions.extend_from_slice(&[x, y, 0.5]);
            normals.extend_from_slice(&[0.0, 0.0, -1.0]);
        }
    }
    let mut indices = Vec::new();
    for row in 0..cells {
        for column in 0..cells {
            let corner = (row * (cells + 1) + column) as u32;
            let next = corner + (cells + 1) as u32;
            // (wound to face the camera, so neither painter turns its normals round)
            indices.extend_from_slice(&[corner, corner + 1, next, corner + 1, next + 1, next]);
        }
    }
    let count = (cells + 1) * (cells + 1);
    let slab = Arc::new(
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&positions),
            normals: bytes_of(&normals),
            indices: indices
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            indices_wide: true,
            softness: bytes_of(&vec![1.0; count]),
            ..MeshParts::default()
        })
        .expect("a slab"),
    );
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    // seen from in front and a little to the side, lit from the upper left
    scene.lights = vec![Light::directional([0.6, -0.5, 0.6], [1.0, 1.0, 1.0])];
    let draw = |deform: bool| Draw {
        mesh: 1,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: Material {
            surface: Surface::Standard,
            deform,
            double_sided: true,
            roughness: 0.6,
            specular: 0.0,
            color: [0.8, 0.8, 0.8, 1.0],
            ..Material::default()
        },
    };
    // a ball of radius 0.2 whose surface reaches 5 cm past the slab
    let ball = Contact::Sphere {
        centre: [0.0, 0.0, 0.35],
        radius: 0.2,
    };
    let meshes = |handle: u32| (handle == 1).then(|| slab.clone());
    let shoot = |deform: bool, contacts: Vec<Contact>| {
        let mut scene = scene.clone();
        scene.draws = vec![draw(deform)];
        scene.contacts = contacts;
        scene
    };
    // (pressed and dragged 2 cm sideways by its friction, ADR-805's `:drag`)
    let mut pressed = shoot(true, vec![ball]);
    pressed.contact_drags = vec![[0.02, 0.0, 0.0]];
    let plain = shoot(true, vec![]);
    let not_deformed = shoot(false, vec![ball]);
    let (_, _, cpu_pressed) = rasterize(&pressed, &meshes, &|_| None);
    let (_, _, cpu_plain) = rasterize(&plain, &meshes, &|_| None);
    let (_, _, cpu_not) = rasterize(&not_deformed, &meshes, &|_| None);
    let difference = |a: &[u8], b: &[u8], stride: usize| -> u64 {
        a.chunks(stride)
            .zip(b.chunks(stride))
            .map(|(x, y)| u64::from(x[0].abs_diff(y[0])))
            .sum()
    };
    assert_eq!(
        cpu_plain, cpu_not,
        "the CPU painter: no contacts and no :deform draw the same"
    );
    assert!(
        difference(&cpu_pressed, &cpu_plain, 4) > 500,
        "the CPU painter dents the slab"
    );
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let mut frames = Vec::new();
        for scene in [&pressed, &plain, &not_deformed] {
            let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
                continue;
            };
            gpu.upload_mesh(1, &slab);
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            frames.push(gpu.read_pixels());
        }
        if frames.len() < 3 {
            continue;
        }
        let dent = difference(&frames[0], &frames[1], 3);
        let unchanged = difference(&frames[1], &frames[2], 3);
        // the GPU's dent against the CPU's, pixel by pixel in red, over the whole picture
        let cpu_red: Vec<u8> = cpu_pressed.chunks(4).map(|pixel| pixel[0]).collect();
        let gpu_red: Vec<u8> = frames[0].chunks(3).map(|pixel| pixel[0]).collect();
        let apart = cpu_red
            .iter()
            .zip(&gpu_red)
            .filter(|(a, b)| a.abs_diff(**b) > 24)
            .count();
        eprintln!(
            "{name}: dent {dent}, unpressed apart {unchanged}, pixels apart from the CPU {apart} of {}",
            gpu_red.len()
        );
        if dent < 500 || unchanged > 0 || apart > gpu_red.len() / 50 {
            failures.push(format!("{name}: dent {dent}, unpressed apart {unchanged}, {apart} pixels apart from the CPU"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
