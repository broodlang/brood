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
        contact_garments: Vec::new(),
        contact_onlys: Vec::new(),
        contact_withins: Vec::new(),
        draws: vec![Draw {
            loose: None,
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
        loose: None,
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
        loose: None,
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
        loose: None,
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
        let mut gpu_reds = Vec::new();
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
            gpu_reds.push(gpu_red);
        }
        // the GPU covers as the CPU does: half covered is half as bright over black, and fully
        // covered it is lit. (Not the CPU's value itself: the GPU's tone curve is on luminance
        // since ADR-830, the frozen CPU painter's on each channel, so a bright saturated colour
        // differs by design: 255 against 242 here)
        if let [half, whole] = gpu_reds[..]
            && (half * 2 >= whole + 8 || half * 2 <= whole - 8 || whole < 128)
        {
            failures.push(format!(
                "{name}: half covered {half} against whole {whole} (half as bright, lit)"
            ));
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
        loose: None,
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
    // ADR-816: the same ball acting only where x > 0 (`:within`): its left half drawn as the
    // unpressed slab, and both painters alike
    let mut half = pressed.clone();
    half.contact_withins = vec![[1.0, 0.0, 0.0, 0.0]];
    let (_, _, cpu_half) = rasterize(&half, &meshes, &|_| None);
    let left_apart = (0..side as usize)
        .flat_map(|row| (0..side as usize / 3).map(move |column| row * side as usize + column))
        .filter(|at| cpu_half[at * 4].abs_diff(cpu_plain[at * 4]) > 4)
        .count();
    assert!(
        left_apart < 20,
        "the far side unpressed: {left_apart} pixels apart"
    );
    assert!(
        difference(&cpu_half, &cpu_plain, 4) > 100,
        "the near side pressed"
    );
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &slab);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(half.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let pixels = gpu.read_pixels();
        let apart = (0..(side * side) as usize)
            .filter(|at| pixels[at * 3].abs_diff(cpu_half[at * 4]) > 24)
            .count();
        eprintln!("{name}: within, pixels apart from the CPU {apart}");
        if apart > (side * side / 50) as usize {
            failures.push(format!("{name}: within, {apart} pixels apart from the CPU"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ADR-805/810: contacts past the sixteenth, alike on both painters. A soft slab with 21
/// contacts: a ball pressing and dragging it at slot 4, and twenty far balls that press nothing,
/// among them an `:only` at 17 and at 20 and a `:within` at 19. The CPU painter's skip mask was
/// 16 bits until 1f94bcc89: slot 20's `:only` wrapped onto slot 4 and the slab was never
/// pressed there. Drawn, the 21 must be the ball alone, on both painters (press and drag; not
/// the contact shadows, the CPU painter being frozen at ADR-818).
#[test]
fn contacts_past_sixteen_press_and_drag_alike_on_both_painters() {
    use crate::host::gui::mesh::Contact;
    let side = 96u32;
    let cells = 48usize;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    for row in 0..=cells {
        for column in 0..=cells {
            positions.extend_from_slice(&[
                column as f32 / cells as f32 - 0.5,
                row as f32 / cells as f32 - 0.5,
                0.5,
            ]);
            normals.extend_from_slice(&[0.0, 0.0, -1.0]);
        }
    }
    let mut indices = Vec::new();
    for row in 0..cells {
        for column in 0..cells {
            let corner = (row * (cells + 1) + column) as u32;
            let next = corner + (cells + 1) as u32;
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
    scene.lights = vec![Light::directional([0.6, -0.5, 0.6], [1.0, 1.0, 1.0])];
    scene.draws = vec![Draw {
        loose: None,
        mesh: 1,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: Material {
            surface: Surface::Standard,
            deform: true,
            double_sided: true,
            roughness: 0.6,
            specular: 0.0,
            color: [0.8, 0.8, 0.8, 1.0],
            ..Material::default()
        },
    }];
    let ball = Contact::Sphere {
        centre: [0.0, 0.0, 0.35],
        radius: 0.2,
    };
    let far = Contact::Sphere {
        centre: [0.0, 0.0, -10.0],
        radius: 0.1,
    };
    let drag = [0.02, 0.0, 0.0];
    let none = [u16::MAX; 4];
    // the ball alone, pressing and dragging: what the 21 must draw
    let mut alone = scene.clone();
    alone.contacts = vec![ball];
    alone.contact_drags = vec![drag];
    // the 21: the ball at slot 4, an `:only` (joint 0) at 17 and 20, a `:within` at 19
    let mut many = scene.clone();
    many.contacts = (0..21)
        .map(|slot| if slot == 4 { ball } else { far })
        .collect();
    many.contact_drags = (0..21)
        .map(|slot| if slot == 4 { drag } else { [0.0; 3] })
        .collect();
    many.contact_onlys = (0..21)
        .map(|slot| {
            if slot == 17 || slot == 20 {
                [0, u16::MAX, u16::MAX, u16::MAX]
            } else {
                none
            }
        })
        .collect();
    many.contact_withins = (0..21)
        .map(|slot| {
            if slot == 19 {
                [1.0, 0.0, 0.0, 0.0]
            } else {
                [0.0; 4]
            }
        })
        .collect();
    let plain = scene.clone();
    let meshes = |handle: u32| (handle == 1).then(|| slab.clone());
    let (_, _, cpu_alone) = rasterize(&alone, &meshes, &|_| None);
    let (_, _, cpu_many) = rasterize(&many, &meshes, &|_| None);
    let (_, _, cpu_plain) = rasterize(&plain, &meshes, &|_| None);
    let apart = |a: &[u8], a_stride: usize, b: &[u8], b_stride: usize| -> usize {
        a.chunks(a_stride)
            .zip(b.chunks(b_stride))
            .filter(|(x, y)| x[0].abs_diff(y[0]) > 24)
            .count()
    };
    // (red summed over the picture, as the ball's test measures a dent: a dent shades many pixels
    // a little)
    let difference = |a: &[u8], a_stride: usize, b: &[u8], b_stride: usize| -> u64 {
        a.chunks(a_stride)
            .zip(b.chunks(b_stride))
            .map(|(x, y)| u64::from(x[0].abs_diff(y[0])))
            .sum()
    };
    let pixels = (side * side) as usize;
    let dent = difference(&cpu_alone, 4, &cpu_plain, 4);
    let many_from_alone = difference(&cpu_many, 4, &cpu_alone, 4);
    eprintln!("CPU: the ball's dent {dent}; the 21 from the ball alone {many_from_alone}");
    assert!(
        dent > 500,
        "the ball alone dents the slab on the CPU painter ({dent})"
    );
    assert_eq!(
        many_from_alone, 0,
        "the CPU painter: the 21 contacts draw as the ball alone: slot 4 pressed and dragged past slot 16's :only and :within"
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
        gpu.upload_mesh(1, &slab);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(many.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let gpu_many = gpu.read_pixels();
        let from_cpu = apart(&gpu_many, 3, &cpu_many, 4);
        let dented = difference(&gpu_many, 3, &cpu_plain, 4);
        eprintln!(
            "{name}: 21 contacts: {from_cpu} pixels apart from the CPU's of {pixels}, dented {dented} from the unpressed slab"
        );
        if from_cpu > pixels / 50 || dented <= 500 {
            failures.push(format!(
                "{name}: 21 contacts: {from_cpu} pixels apart from the CPU, dented {dented} from unpressed"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ADR-812: a slab skinned to one joint, its left half bone (softness 0) and its right half soft
/// (1), drawn `:loose` toward a lagging palette 10 cm above its own at amount 0.5: its right half
/// is drawn 5 cm up and its left where it was, alike on both painters; without `:loose` (or at
/// amount 0) it is drawn as its own palette puts it.
#[test]
fn loose_flesh_follows_its_lagging_bones_alike_on_both_painters() {
    use crate::host::gui::mesh::Loose;
    let side = 96u32;
    let cells = 48usize;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut softness = Vec::new();
    for row in 0..=cells {
        for column in 0..=cells {
            let x = column as f32 / cells as f32 - 0.5;
            let y = row as f32 / cells as f32 - 0.5;
            positions.extend_from_slice(&[x, y * 0.6, 0.5]);
            normals.extend_from_slice(&[0.0, 0.0, -1.0]);
            softness.push(if x > 0.0 { 1.0f32 } else { 0.0 });
        }
    }
    let mut indices = Vec::new();
    for row in 0..cells {
        for column in 0..cells {
            let corner = (row * (cells + 1) + column) as u32;
            let next = corner + (cells + 1) as u32;
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
            // every vertex wholly on joint 0
            joints: vec![0u8; count * 4],
            weights: bytes_of(&[1.0f32, 0.0, 0.0, 0.0].repeat(count)),
            softness: bytes_of(&softness),
            ..MeshParts::default()
        })
        .expect("a slab"),
    );
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let mut lifted = identity;
    lifted[13] = 0.1;
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.lights = vec![Light::directional([0.6, -0.5, 0.6], [1.0, 1.0, 1.0])];
    scene.palettes = vec![Arc::from(&identity[..]), Arc::from(&lifted[..])];
    let shoot = |loose: Option<Loose>| {
        let mut scene = scene.clone();
        scene.draws = vec![Draw {
            loose,
            mesh: 1,
            model: identity,
            palette: Some(0),
            morphs: vec![],
            material: Material {
                surface: Surface::Standard,
                deform: true,
                double_sided: true,
                roughness: 0.6,
                specular: 0.0,
                color: [0.8, 0.8, 0.8, 1.0],
                ..Material::default()
            },
        }];
        scene
    };
    let loose = shoot(Some(Loose {
        palette: 1,
        amount: 0.5,
    }));
    let plain = shoot(None);
    let still = shoot(Some(Loose {
        palette: 1,
        amount: 0.0,
    }));
    let meshes = |handle: u32| (handle == 1).then(|| slab.clone());
    let (_, _, cpu_loose) = rasterize(&loose, &meshes, &|_| None);
    let (_, _, cpu_plain) = rasterize(&plain, &meshes, &|_| None);
    let (_, _, cpu_still) = rasterize(&still, &meshes, &|_| None);
    assert_eq!(cpu_plain, cpu_still, "the CPU painter: amount 0 is no lag");
    // the left third of the picture (bone) unmoved, the right third moved
    let third = |pixels: &[u8], stride: usize, right: bool| -> Vec<u8> {
        (0..side as usize)
            .flat_map(|row| {
                let columns = if right {
                    (2 * side as usize / 3)..side as usize
                } else {
                    0..side as usize / 3
                };
                columns.map(move |column| (row * side as usize + column) * stride)
            })
            .map(|at| pixels[at])
            .collect()
    };
    assert_eq!(
        third(&cpu_loose, 4, false),
        third(&cpu_plain, 4, false),
        "the CPU painter: bone stays where its bones are"
    );
    assert_ne!(
        third(&cpu_loose, 4, true),
        third(&cpu_plain, 4, true),
        "the CPU painter: soft flesh goes toward the lagging bones"
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
        for scene in [&loose, &plain, &still] {
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
        let red = |pixels: &[u8], stride: usize| -> Vec<u8> {
            pixels.chunks(stride).map(|pixel| pixel[0]).collect()
        };
        let apart = |a: &[u8], b: &[u8]| {
            a.iter()
                .zip(b)
                .filter(|(x, y)| x.abs_diff(**y) > 24)
                .count()
        };
        let from_cpu = apart(&red(&frames[0], 3), &red(&cpu_loose, 4));
        let moved = apart(&red(&frames[0], 3), &red(&frames[1], 3));
        let still_apart = apart(&red(&frames[2], 3), &red(&frames[1], 3));
        eprintln!(
            "{name}: moved {moved}, amount 0 apart {still_apart}, pixels apart from the CPU {from_cpu} of {}",
            side * side
        );
        if moved < 100 || still_apart > 0 || from_cpu > (side * side) as usize / 50 {
            failures.push(format!(
                "{name}: moved {moved}, amount 0 apart {still_apart}, {from_cpu} apart from the CPU"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ADR-813: a `:fibres` strand seen from above, at its real width. Half a pixel wide it
/// covers about half its pixel, blended over the black behind (no hash, no mask); sixteen
/// pixels wide it covers its pixel wholly; and nothing is drawn beside it. Its passes'
/// pipelines (`fibres.rs`, `fs_fibre_*`) are made and validated here.
#[test]
fn a_fibre_strand_covers_its_pixel_by_its_real_width() {
    let side = 128u32;
    // one strand along z through the middle, from z = −2 (its root) to z = 2 (its tip): two
    // vertices a point, its normal its direction, uv.x its side, uv.y 1 at the root
    let strand = Arc::new(
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&[0.0, 0.0, -2.0, 0.0, 0.0, -2.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0]),
            normals: bytes_of(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
            uvs: bytes_of(&[-0.5, 1.0, 0.5, 1.0, -0.5, 0.0, 0.5, 0.0]),
            indices: [0u16, 1, 2, 1, 3, 2]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        })
        .expect("a strand"),
    );
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    // seen straight down: 0.25 of the picture's half a metre, so 16 pixels a metre
    scene.view_proj = [
        0.25, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0])];
    let fibre = |width: f32| Draw {
        loose: None,
        mesh: 1,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: Material {
            surface: Surface::Strands,
            fibres: true,
            double_sided: true,
            strand_width: [width, width],
            roughness: 0.6,
            specular: 0.0,
            color: [0.9, 0.6, 0.3, 1.0],
            ..Material::default()
        },
    };
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let mut reds = Vec::new();
        for width in [1.0f32 / 32.0, 1.0] {
            let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
                continue;
            };
            gpu.upload_mesh(1, &strand);
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            scene.draws = vec![fibre(width)];
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            let pixels = gpu.read_pixels();
            let red = |x: u32| i32::from(pixels[((64 * side + x) * 3) as usize]);
            // the strand's column (it runs between pixels 63 and 64) and well beside it
            let across = (red(63) + red(64), red(40));
            eprintln!(
                "{name}: a fibre {width} m wide: red in its column {}, beside it {}",
                across.0, across.1
            );
            reds.push(across);
        }
        if reds.len() < 2 {
            continue;
        }
        let (thin, wide) = (reds[0], reds[1]);
        assert!(
            wide.0 > 100,
            "{name}: a wide fibre covers its pixels: {wide:?}"
        );
        assert!(
            thin.0 > 10 && thin.0 * 10 < wide.0 * 7,
            "{name}: half a pixel wide covers part: {thin:?} against {wide:?}"
        );
        assert!(
            thin.1 < 3 && wide.1 < 3,
            "{name}: nothing beside it: {thin:?} {wide:?}"
        );
    }
}

/// ADR-813: a strand along z, seen from above, with glass at z = 1 facing back along it
/// (`:clamp-planes`). Its part past the glass is laid onto it, so nothing of it is drawn beyond
/// z = 1 and it is drawn as before short of it, alike on both painters (a plain `:strands`
/// surface, which both draw the same way); without the plane it reaches z = 2.
#[test]
fn a_strand_is_laid_on_its_clamp_plane_alike_on_both_painters() {
    let side = 128u32;
    let strand = Arc::new(
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&[0.0, 0.0, -2.0, 0.0, 0.0, -2.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0]),
            normals: bytes_of(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
            uvs: bytes_of(&[-0.5, 1.0, 0.5, 1.0, -0.5, 0.0, 0.5, 0.0]),
            indices: [0u16, 1, 2, 1, 3, 2]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        })
        .expect("a strand"),
    );
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    // seen straight down, 16 pixels a metre: z = 0.5 at row 72, z = 1.5 at row 88
    scene.view_proj = [
        0.25, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0])];
    let shoot = |planes: Vec<[f32; 4]>| {
        let mut scene = scene.clone();
        scene.draws = vec![Draw {
            loose: None,
            mesh: 1,
            model: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            palette: None,
            morphs: vec![],
            material: Material {
                surface: Surface::Strands,
                double_sided: true,
                strand_width: [0.25, 0.25],
                roughness: 0.6,
                specular: 0.0,
                color: [0.9, 0.6, 0.3, 1.0],
                clamp_planes: planes,
                ..Material::default()
            },
        }];
        scene
    };
    // the glass: normal −z (the strand keeps to z < 1), −z · (0, 0, 1) = −1
    let clamped = shoot(vec![[0.0, 0.0, -1.0, -1.0]]);
    let free = shoot(vec![]);
    let meshes = |handle: u32| (handle == 1).then(|| strand.clone());
    let red = |pixels: &[u8], stride: usize, row: u32| {
        i32::from(pixels[((row * side + 64) * stride as u32) as usize])
    };
    let (_, _, cpu_clamped) = rasterize(&clamped, &meshes, &|_| None);
    let (_, _, cpu_free) = rasterize(&free, &meshes, &|_| None);
    assert!(
        red(&cpu_free, 4, 88) > 40,
        "the CPU painter: the free strand reaches z = 1.5"
    );
    assert!(
        red(&cpu_clamped, 4, 88) < 3,
        "the CPU painter: nothing past the glass"
    );
    assert!(
        red(&cpu_clamped, 4, 72) > 40,
        "the CPU painter: the strand short of the glass"
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
        gpu.upload_mesh(1, &strand);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(clamped.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let pixels = gpu.read_pixels();
        let apart = (0..side * side)
            .filter(|at| pixels[(*at * 3) as usize].abs_diff(cpu_clamped[(*at * 4) as usize]) > 24)
            .count();
        eprintln!(
            "{name}: past the glass {}, short of it {}, pixels apart from the CPU {apart} of {}",
            red(&pixels, 3, 88),
            red(&pixels, 3, 72),
            side * side
        );
        if red(&pixels, 3, 88) >= 3
            || red(&pixels, 3, 72) <= 40
            || apart > (side * side / 100) as usize
        {
            failures.push(format!(
                "{name}: past {} short {} apart {apart}",
                red(&pixels, 3, 88),
                red(&pixels, 3, 72)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ADR-814: a tube bent 90° at its joint, seen side on, its vertices turning about their centres
/// of rotation: drawn alike on both painters, and fuller at the bend than the same tube with its
/// joints' matrices blended (which collapses there).
#[test]
fn a_bent_tube_turns_about_its_centres_alike_on_both_painters() {
    let side = 96u32;
    let (rings, sides) = (21usize, 16usize);
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut joints = Vec::new();
    let mut weights = Vec::new();
    for ring in 0..rings {
        let y = ring as f32 / (rings - 1) as f32 * 1.2 - 0.6;
        let upper = ((y + 0.2) / 0.4).clamp(0.0, 1.0);
        for around in 0..sides {
            let angle = around as f32 / sides as f32 * std::f32::consts::TAU;
            positions.extend_from_slice(&[0.2 * angle.cos(), y, 0.2 * angle.sin()]);
            normals.extend_from_slice(&[angle.cos(), 0.0, angle.sin()]);
            joints.extend_from_slice(&[0u8, 1, 0, 0]);
            weights.extend_from_slice(&[1.0 - upper, upper, 0.0, 0.0]);
        }
    }
    let mut indices: Vec<u32> = Vec::new();
    for ring in 0..rings - 1 {
        for around in 0..sides {
            let a = (ring * sides + around) as u32;
            let b = (ring * sides + (around + 1) % sides) as u32;
            let (c, d) = (a + sides as u32, b + sides as u32);
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let parts = || MeshParts {
        positions: bytes_of(&positions),
        normals: bytes_of(&normals),
        joints: joints.clone(),
        weights: bytes_of(&weights),
        indices: indices
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect(),
        indices_wide: true,
        ..MeshParts::default()
    };
    let blended = Arc::new(Mesh::from_parts(parts()).expect("a tube"));
    let found = crate::host::gui::centres::centres(
        &blended.positions,
        &blended.indices,
        &blended.joints,
        &blended.weights,
        2,
    );
    let turning = Arc::new(
        Mesh::from_parts(MeshParts {
            centres: bytes_of(&found.concat()),
            ..parts()
        })
        .expect("a tube with centres"),
    );
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    // the upper bone turned 90° about z through the joint
    let bent: [f32; 16] = [
        0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.lights = vec![Light::directional([0.3, -0.5, 0.8], [1.0, 1.0, 1.0])];
    scene.palettes = vec![Arc::from(&[identity, bent].concat()[..])];
    scene.draws = vec![Draw {
        loose: None,
        mesh: 1,
        model: identity,
        palette: Some(0),
        morphs: vec![],
        material: Material {
            surface: Surface::Standard,
            double_sided: true,
            roughness: 0.6,
            specular: 0.0,
            color: [0.8, 0.8, 0.8, 1.0],
            ..Material::default()
        },
    }];
    let covered =
        |pixels: &[u8], stride: usize| pixels.chunks(stride).filter(|pixel| pixel[0] > 8).count();
    let shoot_cpu = |mesh: &Arc<Mesh>| {
        let mesh = mesh.clone();
        let meshes = move |handle: u32| (handle == 1).then(|| mesh.clone());
        rasterize(&scene, &meshes, &|_| None).2
    };
    let cpu_blended = shoot_cpu(&blended);
    let cpu_turning = shoot_cpu(&turning);
    eprintln!(
        "CPU: covered blended {}, turning {}",
        covered(&cpu_blended, 4),
        covered(&cpu_turning, 4)
    );
    assert!(
        covered(&cpu_turning, 4) > covered(&cpu_blended, 4) + 40,
        "turning about its centres, the bend keeps its fullness"
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
        gpu.upload_mesh(1, &turning);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let pixels = gpu.read_pixels();
        let apart = (0..(side * side) as usize)
            .filter(|at| pixels[at * 3].abs_diff(cpu_turning[at * 4]) > 24)
            .count();
        eprintln!(
            "{name}: covered {}, pixels apart from the CPU {apart} of {}",
            covered(&pixels, 3),
            side * side
        );
        if apart > (side * side / 50) as usize {
            failures.push(format!("{name}: {apart} pixels apart from the CPU"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// ADR-817: a skin quad facing the camera, lit from the side, with a hand-made compress map
/// tilted along +u, a flat stretch map and a mask marking zone one: at zone one's weight 1 the
/// tilt turns its shading, alike on both painters; at weight 0 it is drawn as without wrinkles.
#[test]
fn wrinkles_turn_the_skins_normal_alike_on_both_painters() {
    use crate::host::gui::mesh::{TextureRef, Wrinkles};
    let side = 64u32;
    let positions = [
        -0.5f32, -0.5, 0.5, 0.5, -0.5, 0.5, 0.5, 0.5, 0.5, -0.5, 0.5, 0.5,
    ];
    let normals = [0.0f32, 0.0, -1.0].repeat(4);
    let uvs = [0.0f32, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
    // the second set mirrored in u: read by it (`:uv 1`), the wrinkles are where the head's own
    // layout says
    let uvs2 = [1.0f32, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0];
    let tangents = [1.0f32, 0.0, 0.0, 1.0].repeat(4);
    let quad = Arc::new(
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&positions),
            normals: bytes_of(&normals),
            uvs: bytes_of(&uvs),
            uvs2: bytes_of(&uvs2),
            tangents: bytes_of(&tangents),
            indices: [0u16, 1, 2, 0, 2, 3]
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect(),
            ..MeshParts::default()
        })
        .expect("a quad"),
    );
    // 4 × 4 textures: the compress map tilted along +u (red 192: a tilt of 0.5), the stretch map
    // flat, zone one's mask full, the second mask empty
    let texture = |texel: [u8; 4]| texel.repeat(16);
    let compress = texture([192, 128, 255, 255]);
    let stretch = texture([128, 128, 255, 255]);
    let first = texture([255, 0, 0, 0]);
    let second = texture([0, 0, 0, 0]);
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.lights = vec![Light::directional([0.8, 0.0, 0.6], [1.0, 1.0, 1.0])];
    let shoot = |weight: f32| {
        let mut scene = scene.clone();
        let mut weights = [0.0f32; 8];
        weights[0] = weight;
        scene.draws = vec![Draw {
            loose: None,
            mesh: 1,
            model: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            palette: None,
            morphs: vec![],
            material: Material {
                surface: Surface::Skin,
                double_sided: true,
                specular: 0.0,
                color: [0.8, 0.8, 0.8, 1.0],
                wrinkles: Some(Wrinkles {
                    compress: 11,
                    stretch: 12,
                    masks: [13, 14],
                    weights,
                    uv: 1,
                }),
                ..Material::default()
            },
        }];
        scene
    };
    let meshes = |handle: u32| (handle == 1).then(|| quad.clone());
    let textures = |handle: u32| -> Option<TextureRef<'_>> {
        let rgba: &[u8] = match handle {
            11 => &compress,
            12 => &stretch,
            13 => &first,
            14 => &second,
            _ => return None,
        };
        Some(TextureRef {
            w: 4,
            h: 4,
            rgba,
            levels: &[],
        })
    };
    let folded = shoot(1.0);
    let still = shoot(0.0);
    let (_, _, cpu_folded) = rasterize(&folded, &meshes, &textures);
    let (_, _, cpu_still) = rasterize(&still, &meshes, &textures);
    let centre = ((side / 2 * side + side / 2) * 4) as usize;
    assert!(
        cpu_folded[centre].abs_diff(cpu_still[centre]) > 8,
        "the CPU painter: the wrinkle turns the light: {} against {}",
        cpu_folded[centre],
        cpu_still[centre]
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
        for scene in [&folded, &still] {
            let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
                continue;
            };
            gpu.upload_mesh(1, &quad);
            for (handle, rgba) in [(11, &compress), (12, &stretch), (13, &first), (14, &second)] {
                gpu.upload_texture(handle, rgba, 4, 4, crate::host::gui::TextureFilter::Data);
            }
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            frames.push(gpu.read_pixels());
        }
        if frames.len() < 2 {
            continue;
        }
        let at = ((side / 2 * side + side / 2) * 3) as usize;
        eprintln!(
            "{name}: centre red folded {} still {}; CPU folded {} still {}",
            frames[0][at], frames[1][at], cpu_folded[centre], cpu_still[centre]
        );
        if frames[0][at].abs_diff(cpu_folded[centre]) > 6
            || frames[1][at].abs_diff(cpu_still[centre]) > 6
        {
            failures.push(format!("{name}: GPU apart from the CPU at the centre"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_contact_shadow_darkens_what_lies_just_under_a_near_edge_on_the_gpu() {
    // seen from above, lit by one shadowed light grazing in from +x (no ambient): a card 4 mm
    // over the floor. Just past its −x edge the floor is under it on the way to the light, too
    // near for the shadow map to tell (ADR-818): darker with the camera's depth view (the
    // ambient occlusion's, which the contact shadow marches through) than without; open floor
    // is the same either way. The GPU painter only: the CPU painter is brood's frozen fallback.
    let side = 128u32;
    let floor = Arc::new(square(0.0, 2.0));
    let card = Arc::new(square(0.004, 0.1));
    let mut light = Light::directional([-1.0, -0.25, 0.0], [1.0, 1.0, 1.0]);
    light.shadow = Some(Shadow::default());
    let material = Material {
        double_sided: true,
        specular: 0.0,
        ..Material::default()
    };
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    // x → clip x × 4 (half a metre across, 4 mm a pixel), z → clip −y × 4, height → depth
    scene.view_proj = [
        4.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -4.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![light];
    scene.sky = [0.0, 0.0, 0.0];
    scene.ground = [0.0, 0.0, 0.0];
    let draw = |mesh: u32| Draw {
        loose: None,
        mesh,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: material.clone(),
    };
    scene.draws = vec![draw(1), draw(2)];
    let mut with_depth = scene.clone();
    with_depth.ambient_occlusion = Some(AmbientOcclusion {
        radius: 0.01,
        strength: 1.0,
    });
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    // (pixel columns: the card's −x edge at x −0.1 m is column 64 − 0.1 × 4 × 64 = 38.4)
    let red_at = |pixels: &[u8], column: u32| i32::from(pixels[(64 * side + column) as usize * 3]);
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &floor);
        gpu.upload_mesh(2, &card);
        let mut paint = |scene: &Scene| {
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            gpu.read_pixels()
        };
        let plain = paint(&scene);
        let shaded = paint(&with_depth);
        let under = [37u32, 36].map(|column| (red_at(&plain, column), red_at(&shaded, column)));
        let open = (red_at(&plain, 10), red_at(&shaded, 10));
        eprintln!("{name}: under the edge (plain, contact) {under:?}, open floor {open:?}");
        if !under.iter().any(|(plain, shaded)| shaded + 20 < *plain) {
            failures.push(format!(
                "{name}: no contact shadow under the edge: {under:?}"
            ));
        }
        if (open.0 - open.1).abs() > 3 {
            failures.push(format!("{name}: open floor changed: {open:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn of_two_fibres_over_a_pixel_the_nearer_counts_more_on_the_gpu() {
    // seen from above: a red strand 0.2 m over a blue one, both along z through the middle and
    // each ~0.6 of a pixel wide. Composited front to back (ADR-813, amended), the red one's colour
    // weighs its coverage c and the blue one's c(1 − c): the pixel is redder than blue.
    // Averaged alike (as they once were), the two came out even and hair read over-blended.
    let side = 128u32;
    let strand_at = |height: f32| {
        Arc::new(
            Mesh::from_parts(MeshParts {
                positions: bytes_of(&[
                    0.0, height, -2.0, 0.0, height, -2.0, 0.0, height, 2.0, 0.0, height, 2.0,
                ]),
                normals: bytes_of(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
                uvs: bytes_of(&[-0.5, 1.0, 0.5, 1.0, -0.5, 0.0, 0.5, 0.0]),
                indices: [0u16, 1, 2, 1, 3, 2]
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect(),
                ..MeshParts::default()
            })
            .expect("a strand"),
        )
    };
    let near = strand_at(0.2);
    let far = strand_at(0.0);
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.view_proj = [
        0.25, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0])];
    let fibre = |mesh: u32, color: [f32; 4]| Draw {
        loose: None,
        mesh,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: Material {
            surface: Surface::Strands,
            fibres: true,
            double_sided: true,
            strand_width: [0.04, 0.04],
            roughness: 0.6,
            specular: 0.0,
            color,
            ..Material::default()
        },
    };
    scene.draws = vec![
        fibre(2, [0.0, 0.0, 1.0, 1.0]),
        fibre(1, [1.0, 0.0, 0.0, 1.0]),
    ];
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
        gpu.upload_mesh(1, &near);
        gpu.upload_mesh(2, &far);
        let mut renderer = Renderer::new(1.0, default_families(), 15.0);
        renderer.set_bg(Some([0, 0, 0]));
        gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
            .expect("an offscreen frame is never skipped");
        let pixels = gpu.read_pixels();
        let channel = |x: u32, c: u32| i32::from(pixels[((64 * side + x) * 3 + c) as usize]);
        let red = channel(63, 0) + channel(64, 0);
        let blue = channel(63, 2) + channel(64, 2);
        eprintln!("{name}: over the two fibres red {red} blue {blue}");
        // (each fibre lies over two pixels, ~0.32 of each: front to back gives red ~1/(1 − 0.32) of
        // blue, averaged alike they were even)
        if !(red * 4 > blue * 5 && blue > 0) {
            failures.push(format!(
                "{name}: the nearer fibre does not count more: red {red} blue {blue}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_normal_maps_green_tilts_toward_the_images_up_whatever_the_tangents_w_on_the_gpu() {
    // a quad facing the camera, its image upright (v runs down it, glTF's way), lit from above: a
    // normal map tilted "up" in green (glTF's +Y, the image's up) must light it more than a flat
    // one, with its tangent's w either way (her export's w was reversed on two thirds of her skin
    // and every baked relief was lit upside down, ADR-820). The GPU painter only.
    let side = 64u32;
    let quad = |w: f32| {
        Arc::new(
            Mesh::from_parts(MeshParts {
                positions: bytes_of(&[
                    -0.5f32, -0.5, 0.5, 0.5, -0.5, 0.5, 0.5, 0.5, 0.5, -0.5, 0.5, 0.5,
                ]),
                normals: bytes_of(&[0.0f32, 0.0, -1.0].repeat(4)),
                uvs: bytes_of(&[0.0f32, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]),
                tangents: bytes_of(&[1.0f32, 0.0, 0.0, w].repeat(4)),
                indices: [0u16, 1, 2, 0, 2, 3]
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect(),
                ..MeshParts::default()
            })
            .expect("a quad"),
        )
    };
    let flat = [128u8, 128, 255, 255].repeat(16);
    let up = [128u8, 200, 255, 255].repeat(16);
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.lights = vec![Light::directional([0.0, -0.7, 0.7], [1.0, 1.0, 1.0])];
    scene.draws = vec![Draw {
        loose: None,
        mesh: 1,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: Material {
            double_sided: true,
            specular: 0.0,
            color: [0.8, 0.8, 0.8, 1.0],
            normal: Some(11),
            ..Material::default()
        },
    }];
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let shoot = |w: f32, map: &[u8]| -> Option<i32> {
            let mut gpu = GpuWindow::offscreen(adapter, side, side).ok()?;
            gpu.upload_mesh(1, &quad(w));
            gpu.upload_texture(11, map, 4, 4, crate::host::gui::TextureFilter::Data);
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            let pixels = gpu.read_pixels();
            Some(i32::from(pixels[((32 * side + 32) * 3) as usize]))
        };
        let (Some(level), Some(right), Some(reversed)) =
            (shoot(-1.0, &flat), shoot(-1.0, &up), shoot(1.0, &up))
        else {
            continue;
        };
        eprintln!("{name}: flat {level}, green up with w -1 {right}, with w +1 {reversed}");
        if !(right > level + 5 && reversed > level + 5) {
            failures.push(format!(
                "{name}: green up not lit from above: flat {level}, w -1 {right}, w +1 {reversed}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_see_through_draw_shows_what_is_behind_it_never_another_garment_on_the_gpu() {
    // seen from above, lit straight down: a red floor, over it a green see-through card, over that
    // a smaller blue one (ADR-842: both `:see-through 0.5`). Where the blue card lies over the
    // green, what shows through it is the red floor, never the green card between: the pixel is
    // the same as with no green card at all, and the same again when the green card is an opaque
    // garment (a hem under a panel). Where the green card alone lies, the floor shows through it.
    // With a share of 0 both cards are opaque, as any draw. The GPU painter only.
    let side = 128u32;
    let floor = Arc::new(square(0.0, 2.0));
    let middle = Arc::new(square(0.01, 0.2));
    let top = Arc::new(square(0.02, 0.1));
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    // x → clip x × 4, z → clip −y × 4, height → depth (as the contact shadow's test)
    scene.view_proj = [
        4.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -4.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0])];
    scene.sky = [0.0, 0.0, 0.0];
    scene.ground = [0.0, 0.0, 0.0];
    let draw = |mesh: u32, color: [f32; 4], see_through: f32| Draw {
        loose: None,
        mesh,
        model: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
        palette: None,
        morphs: vec![],
        material: Material {
            color,
            double_sided: true,
            specular: 0.0,
            see_through,
            ..Material::default()
        },
    };
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];
    let mut both = scene.clone();
    both.draws = vec![draw(1, red, 0.0), draw(2, green, 0.5), draw(3, blue, 0.5)];
    let mut top_alone = scene.clone();
    top_alone.draws = vec![draw(1, red, 0.0), draw(3, blue, 0.5)];
    let mut opaque = scene.clone();
    opaque.draws = vec![draw(1, red, 0.0), draw(2, green, 0.0), draw(3, blue, 0.0)];
    // (and an opaque garment between, a hem under a panel: never what shows through it either)
    let mut garment_between = scene.clone();
    let mut hem = draw(2, green, 0.0);
    hem.material.garment = true;
    garment_between.draws = vec![draw(1, red, 0.0), hem, draw(3, blue, 0.5)];
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    // (the middle at column 64; the green card alone at x 0.15 m, column 64 + 0.15 × 4 × 64 = 102)
    let at = |pixels: &[u8], column: u32| {
        let start = (64 * side + column) as usize * 3;
        [0, 1, 2].map(|channel| i32::from(pixels[start + channel]))
    };
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &floor);
        gpu.upload_mesh(2, &middle);
        gpu.upload_mesh(3, &top);
        let mut paint = |scene: &Scene| {
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            gpu.read_pixels()
        };
        let (both, top_alone, opaque) = (paint(&both), paint(&top_alone), paint(&opaque));
        let garment_between = paint(&garment_between);
        let centre = (at(&both, 64), at(&top_alone, 64), at(&opaque, 64));
        let under_garment = at(&garment_between, 64);
        if (0..3).any(|channel| (under_garment[channel] - centre.1[channel]).abs() > 2) {
            failures.push(format!(
                "{name}: an opaque garment shows through the see-through card: {under_garment:?} against {:?} without it",
                centre.1
            ));
        }
        let green_alone = (at(&both, 102), at(&opaque, 102));
        eprintln!(
            "{name}: centre (both, top alone, opaque) {centre:?}; green alone (see-through, opaque) {green_alone:?}"
        );
        if (0..3).any(|channel| (centre.0[channel] - centre.1[channel]).abs() > 2) {
            failures.push(format!(
                "{name}: the green card shows through the blue: {:?} against {:?} without it",
                centre.0, centre.1
            ));
        }
        // (the tonemap greys a saturated colour a little: opaque blue is about [19 19 255])
        if centre.0[0] < centre.2[0] + 100 || centre.0[2] < 150 {
            failures.push(format!(
                "{name}: the blue card does not show the floor half through: {:?}",
                centre.0
            ));
        }
        if green_alone.0[0] < green_alone.1[0] + 100 || green_alone.0[1] < 150 {
            failures.push(format!(
                "{name}: the green card does not show the floor half through: {:?}",
                green_alone.0
            ));
        }
        if centre.2[2] < 240 || centre.2[0] > 40 || green_alone.1[1] < 240 || green_alone.1[0] > 70
        {
            failures.push(format!(
                "{name}: a share of 0 is not opaque: {:?} {:?}",
                centre.2, green_alone.1
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_cloths_stretch_is_shown_live_red_where_drawn_wider_than_made_on_the_gpu() {
    // ADR-842 `:show-stretch`: a square of cloth, UVs in metres as made, seen from above. Drawn as
    // made it is at rest: white. Drawn 30 % wider (its model matrix: the pose's stretch, which the
    // view measures against the mesh as made) it is red; drawn 20 % smaller both ways, slack in
    // every direction, blue (20 % narrower one way only is white: its stronger stretch is rest's).
    // The GPU only.
    let side = 128u32;
    let half = 0.2f32;
    let corners = [[-half, half], [half, half], [half, -half], [-half, -half]];
    let positions: Vec<f32> = corners.iter().flat_map(|[x, z]| [*x, 0.0, *z]).collect();
    let normals: Vec<f32> = (0..4).flat_map(|_| [0.0, 1.0, 0.0]).collect();
    let uvs: Vec<f32> = corners.iter().flat_map(|[x, z]| [*x, -*z]).collect();
    // (its fitted stretch at rest, weft 1 and warp 1, as a glTF UV map holds it: v = 1 − warp)
    let uvs2: Vec<f32> = (0..4).flat_map(|_| [1.0, 0.0]).collect();
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
        .iter()
        .flat_map(|index| index.to_le_bytes())
        .collect();
    let cloth = Arc::new(
        Mesh::from_parts(MeshParts {
            positions: bytes_of(&positions),
            normals: bytes_of(&normals),
            uvs: bytes_of(&uvs),
            uvs2: bytes_of(&uvs2),
            indices,
            ..MeshParts::default()
        })
        .expect("a whole square"),
    );
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.view_proj = [
        4.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -4.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0])];
    scene.sky = [0.0, 0.0, 0.0];
    scene.ground = [0.0, 0.0, 0.0];
    let widened = |across: f32, along: f32, fitted: bool| {
        let mut scene = scene.clone();
        scene.draws = vec![Draw {
            loose: None,
            mesh: 1,
            model: [
                across, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, along, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            palette: None,
            morphs: vec![],
            material: Material {
                double_sided: true,
                specular: 0.0,
                show_stretch: true,
                stretch_from_uv2: fitted,
                ..Material::default()
            },
        }];
        scene
    };
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    // (a pixel off any stroke: the strokes run along x when it is pulled along x, 6 mm apart in v
    // across them; the middle row's average over a few columns)
    let at = |pixels: &[u8]| {
        let mut sum = [0i32; 3];
        for column in 60..68u32 {
            for row in 60..68u32 {
                let start = (row * side + column) as usize * 3;
                for channel in 0..3 {
                    sum[channel] += i32::from(pixels[start + channel]);
                }
            }
        }
        sum.map(|total| total / 64)
    };
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &cloth);
        let mut paint = |scene: &Scene| {
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            gpu.read_pixels()
        };
        let (rest, wide, narrow, smaller) = (
            at(&paint(&widened(1.0, 1.0, false))),
            at(&paint(&widened(1.3, 1.0, false))),
            at(&paint(&widened(0.8, 1.0, false))),
            at(&paint(&widened(0.8, 0.8, false))),
        );
        // (and with its fitted stretch read, at rest: rest's white, its warp read back from
        // glTF's flipped v, never 0, the blue of a cloth slack to a quarter, ADR-842)
        let fitted = at(&paint(&widened(1.0, 1.0, true)));
        if fitted != rest {
            failures.push(format!(
                "{name}: its fitted stretch at rest is not rest's white: {fitted:?}"
            ));
        }
        eprintln!(
            "{name}: as made {rest:?}, 30 % wider {wide:?}, 20 % narrower {narrow:?}, 20 % smaller {smaller:?}"
        );
        if (rest[0] - rest[2]).abs() > 12 || rest[1] < 150 {
            failures.push(format!("{name}: at rest it is not white: {rest:?}"));
        }
        if wide[0] < wide[2] + 80 || wide[0] < wide[1] + 40 {
            failures.push(format!("{name}: 30 % wider is not red: {wide:?}"));
        }
        if narrow != rest {
            failures.push(format!(
                "{name}: 20 % narrower one way is not rest's white: {narrow:?}"
            ));
        }
        if smaller[2] < smaller[0] + 60 {
            failures.push(format!(
                "{name}: 20 % smaller both ways is not blue: {smaller:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_knit_lets_through_only_what_it_lies_on_on_the_gpu() {
    // ADR-842: a white opaque knit card (its stretch read, at rest, a cover of 0.5: half its area
    // open) over a red floor, seen from above with the ambient occlusion's view on. Lying 1 mm
    // over the floor, within `:knit-contact` (3 mm), the floor shows through its open loops: red
    // in the white. Standing 2 cm off, past it, nothing shows: plain white, as a knit stood off
    // skin looks. The GPU painter only.
    use crate::host::gui::mesh::AmbientOcclusion;
    let side = 128u32;
    let floor = Arc::new(square(0.0, 2.0));
    let card = |height: f32| {
        let half = 0.2f32;
        let corners = [[-half, half], [half, half], [half, -half], [-half, -half]];
        let positions: Vec<f32> = corners.iter().flat_map(|[x, z]| [*x, height, *z]).collect();
        let normals: Vec<f32> = (0..4).flat_map(|_| [0.0, 1.0, 0.0]).collect();
        let uvs: Vec<f32> = corners.iter().flat_map(|[x, z]| [*x, -*z]).collect();
        // (weft 1, warp 1 as a glTF UV map holds it: v = 1 − warp)
        let uvs2: Vec<f32> = (0..4).flat_map(|_| [1.0, 0.0]).collect();
        let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        Arc::new(
            Mesh::from_parts(MeshParts {
                positions: bytes_of(&positions),
                normals: bytes_of(&normals),
                uvs: bytes_of(&uvs),
                uvs2: bytes_of(&uvs2),
                indices,
                ..MeshParts::default()
            })
            .expect("a whole card"),
        )
    };
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    // a perspective-free view from above (as the see-through test's): x → clip x × 4, z → clip
    // −y × 4, height → depth over 10 m
    scene.view_proj = [
        4.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.1, 0.0, 0.0, -4.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
    ];
    scene.eye = [0.0, 10.0, 0.0];
    scene.lights = vec![Light::directional([0.0, -1.0, 0.0], [1.0, 1.0, 1.0])];
    scene.sky = [0.0, 0.0, 0.0];
    scene.ground = [0.0, 0.0, 0.0];
    scene.ambient_occlusion = Some(AmbientOcclusion {
        radius: 0.01,
        strength: 0.0,
    });
    let identity = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    scene.draws = vec![
        Draw {
            loose: None,
            mesh: 1,
            model: identity,
            palette: None,
            morphs: vec![],
            material: Material {
                color: [1.0, 0.0, 0.0, 1.0],
                double_sided: true,
                specular: 0.0,
                ..Material::default()
            },
        },
        Draw {
            loose: None,
            mesh: 2,
            model: identity,
            palette: None,
            morphs: vec![],
            material: Material {
                color: [1.0, 1.0, 1.0, 1.0],
                double_sided: true,
                specular: 0.0,
                garment: true,
                stretch_from_uv2: true,
                knit_cover: 0.5,
                knit_layers: 1.0,
                ..Material::default()
            },
        },
    ];
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let at = |pixels: &[u8]| {
        let start = (64 * side + 64) as usize * 3;
        [0, 1, 2].map(|channel| i32::from(pixels[start + channel]))
    };
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &floor);
        let mut paint = |height: f32| {
            gpu.upload_mesh(2, &card(height));
            let mut renderer = Renderer::new(1.0, default_families(), 15.0);
            renderer.set_bg(Some([0, 0, 0]));
            gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                .expect("an offscreen frame is never skipped");
            gpu.read_pixels()
        };
        let (lying, standing) = (at(&paint(0.001)), at(&paint(0.02)));
        eprintln!("{name}: lying 1 mm on the floor {lying:?}, standing 2 cm off {standing:?}");
        if lying[0] < lying[1] + 40 {
            failures.push(format!(
                "{name}: the floor does not show through the knit lying on it: {lying:?}"
            ));
        }
        if (standing[0] - standing[1]).abs() > 4 || standing[1] < 200 {
            failures.push(format!(
                "{name}: the knit standing off shows what is behind it: {standing:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_carried_garment_keeps_its_offset_over_the_pressed_skin_on_the_gpu() {
    // ADR-846: a soft slab pressed by a ball, and a garment slab 5 mm in front of it paired vertex
    // for vertex with it (`:carried-by 0`). Pressed on its own the garment would lie on the ball
    // with the skin; carried, it stays 5 mm off the skin's pressed middle.
    use crate::host::gui::mesh::Contact;
    use crate::host::gui::scene_posed;
    let side = 64u32;
    let cells = 24usize;
    let slab = |z: f32, paired: bool| {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        for row in 0..=cells {
            for column in 0..=cells {
                let x = column as f32 / cells as f32 - 0.5;
                let y = row as f32 / cells as f32 - 0.5;
                positions.extend_from_slice(&[x, y, z]);
                normals.extend_from_slice(&[0.0, 0.0, -1.0]);
            }
        }
        let mut indices = Vec::new();
        for row in 0..cells {
            for column in 0..cells {
                let corner = (row * (cells + 1) + column) as u32;
                let next = corner + (cells + 1) as u32;
                indices.extend_from_slice(&[corner, corner + 1, next, corner + 1, next + 1, next]);
            }
        }
        let count = (cells + 1) * (cells + 1);
        let (skin_vertices, skin_bary) = if paired {
            (
                (0..count as u32)
                    .flat_map(|vertex| [vertex, vertex, vertex])
                    .flat_map(|vertex| vertex.to_le_bytes())
                    .collect(),
                bytes_of(
                    &(0..count)
                        .flat_map(|_| [1.0f32, 0.0, 0.0])
                        .collect::<Vec<_>>(),
                ),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        Arc::new(
            Mesh::from_parts(MeshParts {
                positions: bytes_of(&positions),
                normals: bytes_of(&normals),
                indices: indices
                    .iter()
                    .flat_map(|index| index.to_le_bytes())
                    .collect(),
                indices_wide: true,
                softness: bytes_of(&vec![1.0; count]),
                skin_vertices,
                skin_bary,
                skin_vertex_count: paired.then_some(count as u32),
                ..MeshParts::default()
            })
            .expect("a slab"),
        )
    };
    let skin = slab(0.5, false);
    let cloth = slab(0.495, true);
    let mut scene = self::scene();
    scene.w = side as f32;
    scene.h = side as f32;
    scene.lights = vec![Light::directional([0.6, -0.5, 0.6], [1.0, 1.0, 1.0])];
    let identity = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let draw = |mesh: u32, material: Material| Draw {
        loose: None,
        mesh,
        model: identity,
        palette: None,
        morphs: vec![],
        material,
    };
    scene.draws = vec![
        draw(
            1,
            Material {
                deform: true,
                double_sided: true,
                ..Material::default()
            },
        ),
        draw(
            2,
            Material {
                deform: true,
                garment: true,
                carried_by: Some(0),
                double_sided: true,
                ..Material::default()
            },
        ),
    ];
    // a ball reaching 5 cm past the skin, from in front
    scene.contacts = vec![Contact::Sphere {
        centre: [0.0, 0.0, 0.35],
        radius: 0.2,
    }];
    let adapters = hardware_adapters();
    if adapters.is_empty() {
        eprintln!("skipped the GPU: no hardware GPU adapter here (a software one does not count)");
        return;
    }
    let middle = cells / 2 * (cells + 1) + cells / 2;
    let mut failures = Vec::new();
    for adapter in &adapters {
        let info = adapter.get_info();
        let name = format!("{} ({:?})", info.name, info.backend);
        let Ok(mut gpu) = GpuWindow::offscreen(adapter, side, side) else {
            continue;
        };
        gpu.upload_mesh(1, &skin);
        gpu.upload_mesh(2, &cloth);
        let mut read = |draw: usize| -> Option<f32> {
            scene_posed::request(draw);
            for _ in 0..8 {
                let mut renderer = Renderer::new(1.0, default_families(), 15.0);
                gpu.paint(&[Op::Scene(Arc::new(scene.clone()))], &mut renderer)
                    .expect("an offscreen frame is never skipped");
                std::thread::sleep(std::time::Duration::from_millis(20));
                if let Some(sample) = scene_posed::latest()
                    && sample.draw == draw
                    && scene_posed::requested().is_none()
                {
                    return sample.positions.get(middle * 3 + 2).copied();
                }
            }
            None
        };
        let (Some(skin_z), Some(cloth_z)) = (read(0), read(1)) else {
            failures.push(format!("{name}: the posed slabs were never read back"));
            continue;
        };
        eprintln!(
            "{name}: the skin's middle at z {skin_z:.4}, the carried cloth's at {cloth_z:.4}"
        );
        // (pressed away from the ball, toward +z, by up to `MAX_SQUASH`)
        if skin_z < 0.51 {
            failures.push(format!(
                "{name}: the ball did not press the skin: z {skin_z}"
            ));
        }
        if ((skin_z - cloth_z) - 0.005).abs() > 0.0005 {
            failures.push(format!(
                "{name}: the cloth does not keep its 5 mm over the pressed skin: skin {skin_z}, cloth {cloth_z}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
