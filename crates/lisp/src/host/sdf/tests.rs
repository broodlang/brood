//! Each shape against its analytic distance, each join, the region and material rules, the
//! validation's refusals, and a bake that does not depend on its thread count.

use super::*;

/// A record from its kind, op, size and shape parameters, everything else absent.
fn record(kind: f64, op: f64, size: f64, params: &[f64]) -> Vec<Option<f64>> {
    let mut words = vec![None; RECORD_WORDS];
    words[KIND] = Some(kind);
    words[OP] = Some(op);
    words[SIZE] = Some(size);
    for (offset, value) in params.iter().enumerate() {
        words[PARAMS + offset] = Some(*value);
    }
    words
}

fn with(mut words: Vec<Option<f64>>, slot: usize, value: f64) -> Vec<Option<f64>> {
    words[slot] = Some(value);
    words
}

fn with_bound(mut words: Vec<Option<f64>>, bound: [f64; 4]) -> Vec<Option<f64>> {
    for (offset, value) in bound.iter().enumerate() {
        words[BOUND + offset] = Some(*value);
    }
    words
}

/// A material record: index, name, albedo, no base, alpha 1, no scatter.
fn material(index: f64, name: f64, albedo: [f64; 3]) -> Vec<Option<f64>> {
    vec![
        Some(index),
        Some(name),
        Some(albedo[0]),
        Some(albedo[1]),
        Some(albedo[2]),
        None,
        None,
        None,
        Some(1.0),
        None,
    ]
}

fn scene(records: Vec<Vec<Option<f64>>>) -> Scene {
    scene_with(
        records,
        vec![material(0.0, 0.0, [200.0, 150.0, 120.0])],
        Vec::new(),
    )
}

fn scene_with(
    records: Vec<Vec<Option<f64>>>,
    materials: Vec<Vec<Option<f64>>>,
    points: Vec<f64>,
) -> Scene {
    Scene::from_source(&source(records, materials, points)).expect("a well-formed scene")
}

fn source(
    records: Vec<Vec<Option<f64>>>,
    materials: Vec<Vec<Option<f64>>>,
    points: Vec<f64>,
) -> SceneSource {
    SceneSource {
        records: records.into_iter().flatten().collect(),
        points,
        fields: Vec::new(),
        materials: materials.into_iter().flatten().collect(),
        fallback: 0,
        bounds: vec![-10.0, -10.0, -10.0, 10.0, 10.0, 10.0],
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

const SPHERE: f64 = 1.0;
const ELLIPSOID: f64 = 2.0;
const LIMB: f64 = 3.0;
const BOX: f64 = 4.0;
const OUTLINE: f64 = 5.0;
const PLANE: f64 = 6.0;
const GRID: f64 = 7.0;
const GROUP: f64 = 0.0;
const UNION: f64 = 0.0;
const CARVE: f64 = 1.0;
const INTERSECT: f64 = 2.0;
const SWELL: f64 = 3.0;
const COAT: f64 = 4.0;
const TINT: f64 = 5.0;

#[test]
fn sdf_sphere_is_its_distance_from_the_centre_less_the_radius() {
    let ball = scene(vec![record(SPHERE, UNION, 1.0, &[1.0, 2.0, 3.0, 2.0])]);
    assert_eq!(ball.distance(1.0, 2.0, 3.0), -2.0);
    assert_eq!(ball.distance(1.0, 2.0, 8.0), 3.0);
}

#[test]
fn sdf_ellipsoid_is_exact_on_its_axes_and_a_bound_off_them() {
    let egg = scene(vec![record(
        ELLIPSOID,
        UNION,
        1.0,
        &[0.0, 0.0, 0.0, 2.0, 1.0, 1.0],
    )]);
    assert!(close(egg.distance(4.0, 0.0, 0.0), 2.0));
    assert!(close(egg.distance(0.0, 3.0, 0.0), 2.0));
    assert_eq!(egg.distance(0.0, 0.0, 0.0), -1.0);
    // off the axes it never overestimates the distance to the true surface
    let (x, y) = (3.0_f64, 3.0_f64);
    let nearest = (0..10_000)
        .map(|step| {
            let angle = step as f64 / 10_000.0 * std::f64::consts::TAU;
            let (surface_x, surface_y) = (2.0 * angle.cos(), angle.sin());
            ((x - surface_x).powi(2) + (y - surface_y).powi(2)).sqrt()
        })
        .fold(f64::MAX, f64::min);
    assert!(egg.distance(x, y, 0.0) <= nearest + 1e-6);
}

#[test]
fn sdf_limb_is_a_capsule_when_its_radii_agree_and_tapers_when_not() {
    let capsule = scene(vec![record(
        LIMB,
        UNION,
        1.0,
        &[0.0, 0.0, 0.0, 0.0, 10.0, 0.0, 2.0, 2.0],
    )]);
    assert!(close(capsule.distance(4.0, 5.0, 0.0), 2.0));
    assert!(close(capsule.distance(0.0, 5.0, 0.0), -2.0));
    assert!(close(capsule.distance(0.0, -3.0, 0.0), 1.0));
    assert!(close(capsule.distance(0.0, 14.0, 0.0), 2.0));
    let cone = scene(vec![record(
        LIMB,
        UNION,
        1.0,
        &[0.0, 0.0, 0.0, 0.0, 10.0, 0.0, 2.0, 1.0],
    )]);
    assert!(close(cone.distance(0.0, -3.0, 0.0), 1.0));
    assert!(close(cone.distance(0.0, 13.0, 0.0), 2.0));
    assert!(cone.distance(0.0, 10.0, 0.0) < -0.9);
}

#[test]
fn sdf_box_is_rounded_at_its_edges() {
    let cube = scene(vec![record(
        BOX,
        UNION,
        1.0,
        &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0],
    )]);
    assert_eq!(cube.distance(3.0, 0.0, 0.0), 2.0);
    assert_eq!(cube.distance(0.0, 0.0, 0.0), -1.0);
    assert!(close(cube.distance(2.0, 2.0, 0.0), 2.0_f64.sqrt()));
    let rounded = scene(vec![record(
        BOX,
        UNION,
        1.0,
        &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.5],
    )]);
    assert!(close(
        rounded.distance(2.0, 2.0, 0.0),
        2.0 * 0.5_f64.sqrt() + 0.5_f64.sqrt() - 0.5
    ));
}

#[test]
fn sdf_outline_is_the_polygon_pushed_through_depth() {
    let square = vec![-1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0];
    let pattern = scene_with(
        vec![record(OUTLINE, UNION, 1.0, &[0.0, 4.0])],
        vec![material(0.0, 0.0, [1.0, 1.0, 1.0])],
        square,
    );
    assert_eq!(pattern.distance(0.0, 0.0, 0.0), -1.0);
    assert_eq!(pattern.distance(3.0, 0.0, 50.0), 2.0);
    assert!(close(pattern.distance(2.0, 2.0, -7.0), 2.0_f64.sqrt()));
}

#[test]
fn sdf_plane_is_signed_along_its_normal() {
    let floor = scene(vec![record(
        PLANE,
        UNION,
        1.0,
        &[0.0, 1.0, 0.0, 0.0, 1.0, 0.0],
    )]);
    assert_eq!(floor.distance(5.0, 4.0, -3.0), 3.0);
    assert_eq!(floor.distance(5.0, -1.0, -3.0), -2.0);
}

#[test]
fn sdf_shell_hollows_a_shape_to_a_skin() {
    let hollow = scene(vec![with(
        record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 2.0]),
        SHELL,
        0.25,
    )]);
    assert_eq!(hollow.distance(0.0, 0.0, 0.0), 1.75);
    assert_eq!(hollow.distance(0.0, 0.0, 2.0), -0.25);
}

#[test]
fn sdf_joins_union_carve_intersect_smoothly_or_sharply() {
    let left = record(SPHERE, UNION, 1.0, &[-1.0, 0.0, 0.0, 1.5]);
    let right = |op: f64| record(SPHERE, op, 1.0, &[1.0, 0.0, 0.0, 1.5]);
    let union = scene(vec![left.clone(), right(UNION)]);
    assert_eq!(union.distance(-3.0, 0.0, 0.0), 0.5);
    assert_eq!(union.distance(3.0, 0.0, 0.0), 0.5);
    let smooth = scene(vec![left.clone(), with(right(UNION), BLEND, 1.0)]);
    assert!(smooth.distance(0.0, 1.2, 0.0) < union.distance(0.0, 1.2, 0.0));
    let carve = scene(vec![left.clone(), right(CARVE)]);
    assert!(carve.distance(0.0, 0.0, 0.0) > 0.0);
    assert_eq!(carve.distance(-2.0, 0.0, 0.0), -0.5);
    let intersect = scene(vec![left.clone(), right(INTERSECT)]);
    assert_eq!(intersect.distance(0.0, 0.0, 0.0), -0.5);
    assert!(intersect.distance(-2.0, 0.0, 0.0) > 0.0);
    let smooth_intersect = scene(vec![left, with(right(INTERSECT), BLEND, 0.5)]);
    assert!(smooth_intersect.distance(0.0, 0.0, 0.0) > -0.5);
}

#[test]
fn sdf_swell_raises_and_coat_lifts_with_a_hem_and_tint_leaves_the_surface() {
    let body = record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 2.0]);
    let bump = with(
        with(
            record(SPHERE, SWELL, 1.0, &[0.0, 0.0, 2.0, 0.5]),
            AMOUNT,
            0.3,
        ),
        BLEND,
        0.5,
    );
    let swollen = scene(vec![body.clone(), bump]);
    assert!(close(swollen.distance(0.0, 0.0, 2.0), -0.3));
    assert_eq!(swollen.distance(0.0, 0.0, -2.0), 0.0);
    let cloth = with(
        record(SPHERE, COAT, 1.0, &[0.0, 0.0, 2.0, 1.0]),
        AMOUNT,
        0.2,
    );
    let coated = scene(vec![body.clone(), cloth]);
    assert!(close(coated.distance(0.0, 0.0, 2.0), -0.2));
    assert_eq!(coated.distance(0.0, 0.0, -2.0), 0.0);
    let mut zone = with(
        record(SPHERE, TINT, 1.0, &[0.0, 0.0, 2.0, 1.0]),
        AMOUNT,
        1.0,
    );
    zone[TINT_COLOUR..TINT_COLOUR + 3].copy_from_slice(&[Some(40.0), Some(150.0), Some(120.0)]);
    let tinted = scene(vec![body, zone]);
    assert_eq!(tinted.distance(0.0, 0.0, 2.0), 0.0);
}

#[test]
fn sdf_a_group_joins_its_parts_first_and_skips_what_its_bounds_rule_out() {
    // a group of two spheres carved from a slab, and many far parts the tree culls
    let mut records = vec![record(
        BOX,
        UNION,
        1.0,
        &[0.0, 0.0, 0.0, 4.0, 1.0, 4.0, 0.0],
    )];
    records.push(record(GROUP, CARVE, 3.0, &[]));
    records.push(with_bound(
        record(SPHERE, UNION, 1.0, &[-1.0, 0.0, 0.0, 0.5]),
        [-1.0, 0.0, 0.0, 0.5],
    ));
    records.push(with_bound(
        record(SPHERE, UNION, 1.0, &[1.0, 0.0, 0.0, 0.5]),
        [1.0, 0.0, 0.0, 0.5],
    ));
    for index in 0..8 {
        let x = 100.0 + index as f64 * 3.0;
        records.push(with_bound(
            record(SPHERE, UNION, 1.0, &[x, 0.0, 0.0, 1.0]),
            [x, 0.0, 0.0, 1.0],
        ));
    }
    let carved = scene(records);
    assert_eq!(carved.distance(-1.0, 0.0, 0.0), 0.5);
    assert_eq!(carved.distance(1.0, 0.0, 0.0), 0.5);
    assert_eq!(carved.distance(0.0, 0.0, 0.0), -0.5);
    assert_eq!(carved.distance(103.0, 0.0, 0.0), -1.0);
    // the plain walk (regions, materials) agrees where nothing is culled
    assert_eq!(carved.part_distance(&carved.parts[1], -1.0, 0.0, 0.0), -0.5);
}

#[test]
fn sdf_the_region_is_the_deepest_top_level_union_part() {
    let trunk = with(
        record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 3.0]),
        REGION,
        0.0,
    );
    let arm = with(
        with(
            record(SPHERE, UNION, 1.0, &[-5.0, 0.0, 0.0, 1.5]),
            REGION,
            1.0,
        ),
        BLEND,
        1.0,
    );
    let figure = scene(vec![trunk, arm]);
    assert_eq!(figure.region(0.0, 2.9, 0.0), Some(0));
    assert_eq!(figure.region(-5.0, 1.4, 0.0), Some(1));
    let nameless = scene(vec![record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 3.0])]);
    assert_eq!(nameless.region(0.0, 0.0, 0.0), None);
}

#[test]
fn sdf_material_is_the_nearest_part_unless_claimed_and_tints_shift_the_albedo() {
    let materials = vec![
        material(0.0, 0.0, [220.0, 180.0, 150.0]),
        material(1.0, 1.0, [150.0, 30.0, 40.0]),
    ];
    let body = record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 3.0]);
    let cloth = with(
        with(
            record(SPHERE, COAT, 1.0, &[0.0, 0.0, 3.0, 2.5]),
            AMOUNT,
            0.2,
        ),
        MATERIAL,
        1.0,
    );
    let mut zone = with(
        with(
            record(SPHERE, TINT, 1.0, &[0.0, 0.0, -3.0, 1.0]),
            AMOUNT,
            1.0,
        ),
        BLEND,
        0.2,
    );
    zone[TINT_COLOUR] = Some(40.0);
    zone[TINT_COLOUR + 1] = Some(150.0);
    zone[TINT_COLOUR + 2] = Some(120.0);
    zone[TINT_ON] = Some(0.0);
    let model = scene_with(vec![body, cloth, zone], materials, Vec::new());
    assert_eq!(model.material_at(1.0, 0.0, 2.9), 1);
    assert_eq!(model.material_at(0.0, 0.0, -3.0), 0);
    let tinted = model.tinted(0, 0.0, 0.0, -3.0);
    assert_eq!(tinted, [40.0, 150.0, 120.0]);
    assert_eq!(model.tinted(0, 0.0, 3.0, 0.0), [220.0, 180.0, 150.0]);
    // a tint on another material's name leaves this one alone
    assert_eq!(model.tinted(1, 0.0, 0.0, -3.0), [150.0, 30.0, 40.0]);
}

fn cube_field() -> FieldSource {
    // a 2-unit cube on an 9×9×9 grid of spacing 0.5 from -2: distances in steps of 0.01,
    // labels 3 left of x = 0, 4 right
    let (n, spacing, origin, quantum) = (9i64, 0.5, -2.0, 0.01);
    let mut distances = Vec::new();
    let mut labels = Vec::new();
    for j in 0..n {
        for k in 0..n {
            for i in 0..n {
                let at = |index: i64| origin + spacing * index as f64;
                let (x, y, z) = (at(i), at(j), at(k));
                let outside = [x.abs() - 1.0, y.abs() - 1.0, z.abs() - 1.0];
                let out = outside
                    .iter()
                    .map(|value| value.max(0.0).powi(2))
                    .sum::<f64>()
                    .sqrt();
                let inside = outside[0].max(outside[1]).max(outside[2]).min(0.0);
                let steps = ((out + inside) / quantum).round() as i16;
                distances.extend_from_slice(&steps.to_le_bytes());
                labels.push(if x < 0.0 { 3 } else { 4 });
            }
        }
    }
    let far = origin + spacing * (n - 1) as f64;
    let blob = |bytes: Vec<u8>| crate::core::blob::SharedBlob::new(&bytes);
    FieldSource {
        body: GridSource {
            distances: blob(distances),
            labels: blob(labels),
            numbers: vec![
                origin,
                origin,
                origin,
                spacing,
                1.0 / spacing,
                9.0,
                9.0,
                9.0,
                quantum,
                far,
                far,
                far,
                1000.0,
                43.0,
            ],
        },
        head: None,
        regions: vec![None, None, None, Some(0), Some(1)],
        keep: None,
    }
}

#[test]
fn sdf_grid_reads_its_voxels_through_the_spline_and_bounds_beyond_its_box() {
    let mut source = source(
        vec![record(GRID, UNION, 1.0, &[0.0])],
        vec![material(0.0, 0.0, [1.0, 1.0, 1.0])],
        Vec::new(),
    );
    source.fields.push(cube_field());
    let cube = Scene::from_source(&source).expect("a grid scene");
    assert!(cube.distance(0.0, 0.0, 0.0) < -0.5);
    assert!((cube.distance(0.0, 0.0, 1.5) - 0.5).abs() < 0.1);
    let far = cube.distance(0.0, 0.0, 5.0);
    assert!(far > 2.0 && far <= 4.0 + 1e-9);
    assert_eq!(cube.region(-0.5, 0.0, 1.0), Some(0));
    assert_eq!(cube.region(0.5, 0.0, 1.0), Some(1));
    // keeping only the right half pushes the left one out
    source.fields[0].keep = Some(vec![false, false, false, false, true]);
    let half = Scene::from_source(&source).expect("a kept grid scene");
    assert!(half.distance(0.5, 0.0, 0.0) < 0.0);
    assert!(half.distance(-0.5, 0.0, 0.0) > 0.0);
}

#[test]
fn sdf_malformed_scenes_are_refused_naming_the_record() {
    let materials = || vec![material(0.0, 0.0, [1.0, 1.0, 1.0])];
    let refused = |records: Vec<Vec<Option<f64>>>| {
        Scene::from_source(&source(records, materials(), Vec::new())).unwrap_err()
    };
    assert!(refused(vec![record(9.0, UNION, 1.0, &[])]).contains("unknown kind 9"));
    assert!(
        refused(vec![record(SPHERE, 8.0, 1.0, &[0.0, 0.0, 0.0, 1.0])]).contains("unknown op 8")
    );
    assert!(
        refused(vec![
            record(GROUP, UNION, 3.0, &[]),
            record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 1.0])
        ])
        .contains("runs past")
    );
    assert!(
        refused(vec![record(SPHERE, UNION, 1.0, &[0.0, f64::NAN, 0.0, 1.0])])
            .contains("not a finite number")
    );
    assert!(refused(vec![record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0])]).contains("missing"));
    assert!(
        refused(vec![record(SPHERE, COAT, 1.0, &[0.0, 0.0, 0.0, 1.0])]).contains("needs an amount")
    );
    assert!(refused(vec![record(OUTLINE, UNION, 1.0, &[0.0, 3.0])]).contains("outline points"));
    assert!(refused(vec![record(GRID, UNION, 1.0, &[0.0])]).contains("field"));
    assert!(
        refused(vec![with(
            record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 1.0]),
            MATERIAL,
            4.0
        )])
        .contains("material")
    );
    // nesting past the cap
    let deep: Vec<_> = (0..=MAX_DEPTH + 1)
        .map(|level| record(GROUP, UNION, (MAX_DEPTH + 2 - level) as f64 + 1.0, &[]))
        .chain(std::iter::once(record(
            SPHERE,
            UNION,
            1.0,
            &[0.0, 0.0, 0.0, 1.0],
        )))
        .collect();
    assert!(refused(deep).contains("nest deeper"));
    let mut short = source(
        vec![record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 1.0])],
        materials(),
        Vec::new(),
    );
    short.records.pop();
    assert!(Scene::from_source(&short).unwrap_err().contains("multiple"));
}

fn small_figure() -> Scene {
    // two materials, a carve, a coat, a mirrored pair as separate parts, regions: enough to
    // flag subsamples
    let materials = vec![
        material(0.0, 0.0, [220.0, 150.0, 120.0]),
        material(1.0, 1.0, [40.0, 70.0, 150.0]),
    ];
    let trunk = with_bound(
        with(
            record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 2.0]),
            REGION,
            0.0,
        ),
        [0.0, 0.0, 0.0, 2.0],
    );
    let arm = with_bound(
        with(
            with(
                record(
                    LIMB,
                    UNION,
                    1.0,
                    &[-1.5, -0.5, 0.0, -3.0, 2.0, 0.5, 0.6, 0.4],
                ),
                REGION,
                1.0,
            ),
            BLEND,
            0.5,
        ),
        [-2.25, 0.75, 0.25, 2.2],
    );
    let navel = with_bound(
        record(SPHERE, CARVE, 1.0, &[0.0, 0.5, 2.0, 0.3]),
        [0.0, 0.5, 2.0, 0.3],
    );
    let shirt = with_bound(
        with(
            with(
                record(BOX, COAT, 1.0, &[0.0, 1.5, 0.0, 3.0, 1.0, 3.0, 0.0]),
                AMOUNT,
                0.15,
            ),
            MATERIAL,
            1.0,
        ),
        [0.0, 1.5, 0.0, 4.4],
    );
    scene_with(vec![trunk, arm, navel, shirt], materials, Vec::new())
}

fn request<'a>(scene: &'a Scene, workers: usize) -> BakeRequest<'a> {
    BakeRequest {
        scene,
        castings: Vec::new(),
        centre: [0.0, 0.0, 0.0],
        axes: [[1.0, 0.0, -0.0], [0.0, 1.0, 0.0], [-0.0, 0.0, -1.0]],
        at: [-4.0, -4.0],
        pixel: 0.25,
        columns: 32,
        rows: 32,
        lights: vec![
            Light {
                towards: [-0.5773502691896258, -0.5773502691896258, 0.5773502691896258],
                softness: Some(6.0),
            },
            Light {
                towards: [0.0, 0.0, 1.0],
                softness: None,
            },
        ],
        subsampling: Some(Subsampling {
            depth_step: 0.75,
            least_facing: 0.8,
            always: Vec::new(),
        }),
        workers,
    }
}

#[test]
fn sdf_bake_bytes_do_not_depend_on_the_thread_count() {
    let figure = small_figure();
    let alone = bake(&request(&figure, 1));
    let shared = bake(&request(&figure, 4));
    assert!(alone.subsamples > 0, "the figure's edges are subsampled");
    assert_eq!(alone.rgba.len(), 20 * 32 * alone.height);
    assert_eq!(alone.rgba, shared.rgba);
    // the middle pixel is covered, faces the viewer and is skin; a corner is empty
    let middle = 20 * (16 + 16 * 32);
    assert_eq!(alone.rgba[middle + 3], 255);
    assert!(alone.rgba[middle + 2] > 200);
    assert_eq!(alone.rgba[middle + 12], 0);
    assert_eq!(&alone.rgba[0..20], &[0; 20]);
}

#[test]
fn sdf_bake_casting_scenes_shade_their_regions() {
    // the arm casts nothing on a casting scene without it: the trunk under it is lit more
    let figure = small_figure();
    let without_arm = scene_with(
        vec![with_bound(
            record(SPHERE, UNION, 1.0, &[0.0, 0.0, 0.0, 2.0]),
            [0.0, 0.0, 0.0, 2.0],
        )],
        vec![material(0.0, 0.0, [1.0, 1.0, 1.0])],
        Vec::new(),
    );
    let plain = bake(&request(&figure, 2));
    let mut cast = request(&figure, 2);
    cast.castings = vec![Some(&without_arm)];
    let cast = bake(&cast);
    let occlusion = |bytes: &[u8]| {
        (0..32 * 32)
            .map(|index| bytes[20 * index + 7] as u32)
            .sum::<u32>()
    };
    assert!(occlusion(&cast.rgba) > occlusion(&plain.rgba));
}
