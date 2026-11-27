//! A `:fibres` hair's dual scattering tables (ADR-834; Zinke, Yuksel, Weber and Keyser, "Dual
//! Scattering Approximation for Fast Multiple Scattering in Hair", SIGGRAPH 2008, and Disney's
//! implementation notes): what one fibre sends forward and back, averaged over the directions
//! light leaves it, for every inclination of the light, roughness and colour the shader meets.
//!
//! The lobes integrated are the GPU shader's own (`karis_hair`, ADR-813: Karis 2016's R, TT and
//! TRT, π times UE's), term for term, so the light a strand passes on is what that strand would
//! show. Built once in Rust and uploaded below the skin table's rows (`scenes::table_bytes`).
//!
//! Three kinds of rows, each [`FIBRE_TABLE_ANGLES`] columns wide:
//!
//! - **lobes** ([`FIBRE_TABLE_ROUGHNESSES`] × [`FIBRE_TABLE_ALBEDOS`] rows, a roughness's albedos
//!   together): TT's energy sent forward and back, TRT's forward and back, for one channel of
//!   that albedo, the column the light's inclination (`|sin θi|`, Eq. 6's θd);
//! - **spread** (as many): TT's and TRT's azimuthal parts averaged over the forward half
//!   (`N^G`, Disney's notes after Eq. 16: light come through other fibres arrives from every
//!   side), the column the half difference of the inclinations (`|sin θd|`, Karis's);
//! - **shine** ([`FIBRE_TABLE_ROUGHNESSES`] rows): R's energy forward and back for a `:specular`
//!   of 0.5 (it is linear in it), R's mean longitudinal shift sent back in cuticle tilts, and
//!   R's `N^G`; colourless, R never enters the fibre.

use std::f64::consts::PI;
use std::sync::OnceLock;

/// Columns: the inclination's sine, `(column + ½) / count` from 0 to 1.
pub const FIBRE_TABLE_ANGLES: usize = 32;
/// Roughness rows: `0.2 + 0.1 · index`, read at the nearest.
pub const FIBRE_TABLE_ROUGHNESSES: usize = 8;
/// Albedo rows within a roughness: `(index / (count − 1))²`, dark hair the finest.
pub const FIBRE_TABLE_ALBEDOS: usize = 16;
/// The cuticle's tilt the table is integrated at (`:cuticle-tilt`'s default, radians): the
/// shifts it moves the lobes by are small beside their widths, and the shader scales the shifts
/// by the material's own.
pub const FIBRE_TABLE_TILT: f64 = 0.035;
/// All the table's rows: lobes, spread, then shine.
pub const FIBRE_TABLE_ROWS: usize =
    2 * FIBRE_TABLE_ROUGHNESSES * FIBRE_TABLE_ALBEDOS + FIBRE_TABLE_ROUGHNESSES;

/// The roughness row `index` holds.
pub fn table_roughness(index: usize) -> f64 {
    0.2 + 0.1 * index as f64
}

/// The albedo row `index` holds.
pub fn table_albedo(index: usize) -> f64 {
    let along = index as f64 / (FIBRE_TABLE_ALBEDOS - 1) as f64;
    along * along
}

/// The sine column `index` holds.
pub fn table_sine(index: usize) -> f64 {
    (index as f64 + 0.5) / FIBRE_TABLE_ANGLES as f64
}

/// The whole table, row by row, four floats a texel ([`FIBRE_TABLE_ROWS`] ×
/// [`FIBRE_TABLE_ANGLES`]).
pub fn fibre_table() -> &'static [[f32; 4]] {
    static TABLE: OnceLock<Vec<[f32; 4]>> = OnceLock::new();
    TABLE.get_or_init(build)
}

fn gaussian(width: f64, angle: f64) -> f64 {
    (-0.5 * angle * angle / (width * width)).exp() / (2.506_628_27 * width)
}

/// Schlick at hair's index, 1.55 (`hair_fresnel`)
fn fresnel(cosine: f64) -> f64 {
    0.0465 + 0.9535 * (1.0 - cosine.clamp(0.0, 1.0)).powi(5)
}

/// One fibre's three lobes as `karis_hair` has them (π times each, R for `:specular` 0.5, the
/// light's shadow map known): the light's and the view's inclinations' sines, the cosine of the
/// azimuth between them, the colour of one channel, the longitudinal width and the cuticle's
/// tilt. (R, TT, TRT)
pub fn lobes(
    sin_light: f64,
    sin_view: f64,
    cos_phi: f64,
    colour: f64,
    width: f64,
    tilt: f64,
) -> (f64, f64, f64) {
    let cos_difference = (0.5 * (sin_view.asin() - sin_light.asin()).abs()).cos();
    let cos_half_phi = (0.5 + 0.5 * cos_phi).clamp(0.0, 1.0).sqrt();
    let cos_light = (1.0 - sin_light * sin_light).max(0.0).sqrt();
    let cos_view = (1.0 - sin_view * sin_view).max(0.0).sqrt();
    let view_dot_light = cos_light * cos_view * cos_phi + sin_light * sin_view;
    let index = 1.19 / cos_difference + 0.36 * cos_difference;
    let colour = colour.max(1e-4);
    let along = sin_light + sin_view;
    let shift = 2.0
        * (-2.0 * tilt).sin()
        * ((-2.0 * tilt).cos() * cos_half_phi * cos_view + (-2.0 * tilt).sin() * sin_view);
    let reflected = gaussian(width * 2f64.sqrt() * cos_half_phi.max(1e-3), along - shift)
        * 0.25
        * cos_half_phi
        * fresnel((0.5 + 0.5 * view_dot_light).clamp(0.0, 1.0).sqrt());
    let inverse = 1.0 / index;
    let entry = cos_half_phi * (1.0 + inverse * (0.6 - 0.8 * cos_phi));
    let entering = fresnel(cos_difference * (1.0 - entry * entry).clamp(0.0, 1.0).sqrt());
    let through = colour
        .powf(0.5 * (1.0 - entry * entry * inverse * inverse).max(0.0).sqrt() / cos_difference);
    let transmitted = gaussian(width * 0.5, along - tilt)
        * (-3.65 * cos_phi - 3.98).exp()
        * (1.0 - entering)
        * (1.0 - entering)
        * through;
    let inside = fresnel(cos_difference * 0.5);
    let twice = colour.powf(0.8 / cos_difference);
    let second = gaussian(width * 2.0, along - 4.0 * tilt)
        * (17.0 * cos_phi - 16.78).exp()
        * (1.0 - inside)
        * (1.0 - inside)
        * inside
        * twice;
    (PI * reflected, PI * transmitted, PI * second)
}

/// The lobes' azimuthal parts alone (their Gaussians left out) at the half difference of the
/// inclinations whose sine is `sin_difference`, the light and the view either side of the
/// fibre's normal plane by it, averaged over the forward half of the azimuth: (R, TT, TRT),
/// π times each, R for `:specular` 0.5
fn forward_azimuths(sin_difference: f64, colour: f64) -> (f64, f64, f64) {
    const STEPS: usize = 256;
    let cos_difference = (1.0 - sin_difference * sin_difference).max(0.0).sqrt();
    let index = 1.19 / cos_difference + 0.36 * cos_difference;
    let inverse = 1.0 / index;
    let colour = colour.max(1e-4);
    let (mut shine, mut transmitted, mut second) = (0.0, 0.0, 0.0);
    for step in 0..STEPS {
        // φ from π/2 to π: the forward half
        let phi = 0.5 * PI + 0.5 * PI * (step as f64 + 0.5) / STEPS as f64;
        let cos_phi = phi.cos();
        let cos_half_phi = (0.5 + 0.5 * cos_phi).clamp(0.0, 1.0).sqrt();
        // the light below the plane by θd and the view above it by as much
        let view_dot_light =
            cos_difference * cos_difference * cos_phi - sin_difference * sin_difference;
        shine += 0.25 * cos_half_phi * fresnel((0.5 + 0.5 * view_dot_light).clamp(0.0, 1.0).sqrt());
        let entry = cos_half_phi * (1.0 + inverse * (0.6 - 0.8 * cos_phi));
        let entering = fresnel(cos_difference * (1.0 - entry * entry).clamp(0.0, 1.0).sqrt());
        let through = colour
            .powf(0.5 * (1.0 - entry * entry * inverse * inverse).max(0.0).sqrt() / cos_difference);
        transmitted +=
            (-3.65 * cos_phi - 3.98).exp() * (1.0 - entering) * (1.0 - entering) * through;
        let inside = fresnel(cos_difference * 0.5);
        second += (17.0 * cos_phi - 16.78).exp()
            * (1.0 - inside)
            * (1.0 - inside)
            * inside
            * colour.powf(0.8 / cos_difference);
    }
    let mean = PI / STEPS as f64;
    (shine * mean, transmitted * mean, second * mean)
}

/// Each lobe's energy sent forward (`cos φ < 0`) and back over every direction the light can
/// leave by (Eq. 6: `∫ shine dω`, a share of the light falling on it), for light at inclination sine `sin_light` and each of
/// `colours`; and R's mean shift in the light sent back. ([R forward, R back, TT forward, TT
/// back, TRT forward, TRT back] a colour, R's mean back shift)
pub fn energies(sin_light: f64, colours: &[f64], width: f64, tilt: f64) -> (Vec<[f64; 6]>, f64) {
    const ANGLES: usize = 96;
    const AZIMUTHS: usize = 96;
    let logs: Vec<f64> = colours.iter().map(|colour| colour.max(1e-4).ln()).collect();
    let mut sums = vec![[0.0f64; 6]; colours.len()];
    let mut shift_sum = 0.0;
    let mut shine_back = 0.0;
    let angle_step = PI / ANGLES as f64;
    let azimuth_step = 2.0 * PI / AZIMUTHS as f64;
    for angle in 0..ANGLES {
        let theta = -0.5 * PI + angle_step * (angle as f64 + 0.5);
        let sin_view = theta.sin();
        let solid = theta.cos() * angle_step * azimuth_step;
        for azimuth in 0..AZIMUTHS {
            let phi = -PI + azimuth_step * (azimuth as f64 + 0.5);
            let cos_phi = phi.cos();
            // (each lobe at white, and the powers its colour is raised to: `lobes` term for term)
            let (shine, transmitted, second) =
                lobes(sin_light, sin_view, cos_phi, 1.0, width, tilt);
            let (through_power, twice_power) = powers(sin_light, sin_view, cos_phi);
            let back = usize::from(cos_phi >= 0.0);
            for (index, log) in logs.iter().enumerate() {
                sums[index][back] += shine * solid;
                sums[index][2 + back] += transmitted * (through_power * log).exp() * solid;
                sums[index][4 + back] += second * (twice_power * log).exp() * solid;
            }
            if back == 1 {
                let cos_half_phi = (0.5 + 0.5 * cos_phi).sqrt();
                let shift = 2.0
                    * (-2.0 * tilt).sin()
                    * ((-2.0 * tilt).cos() * cos_half_phi * theta.cos()
                        + (-2.0 * tilt).sin() * sin_view);
                shift_sum += shine * solid * shift;
                shine_back += shine * solid;
            }
        }
    }
    let mean_shift = if shine_back > 0.0 {
        shift_sum / shine_back
    } else {
        0.0
    };
    // (the lobes are π times the radiometric ones, as every surface of the renderer is: a matte
    // one lights as base · light · cos, no 1/π. Divided out, each is the share of the light
    // falling on the fibre that it sends that way, Eq. 6's āf and āb)
    for lobe_sums in sums.iter_mut() {
        for sum in lobe_sums.iter_mut() {
            *sum /= PI;
        }
    }
    (sums, mean_shift)
}

/// The powers `lobes` raises a colour to for TT and TRT at these angles
fn powers(sin_light: f64, sin_view: f64, cos_phi: f64) -> (f64, f64) {
    let cos_difference = (0.5 * (sin_view.asin() - sin_light.asin()).abs()).cos();
    let cos_half_phi = (0.5 + 0.5 * cos_phi).clamp(0.0, 1.0).sqrt();
    let inverse = 1.0 / (1.19 / cos_difference + 0.36 * cos_difference);
    let entry = cos_half_phi * (1.0 + inverse * (0.6 - 0.8 * cos_phi));
    (
        0.5 * (1.0 - entry * entry * inverse * inverse).max(0.0).sqrt() / cos_difference,
        0.8 / cos_difference,
    )
}

fn build() -> Vec<[f32; 4]> {
    let mut table = vec![[0.0f32; 4]; FIBRE_TABLE_ROWS * FIBRE_TABLE_ANGLES];
    let spread_rows = FIBRE_TABLE_ROUGHNESSES * FIBRE_TABLE_ALBEDOS;
    let shine_rows = 2 * spread_rows;
    let colours: Vec<f64> = (0..FIBRE_TABLE_ALBEDOS).map(table_albedo).collect();
    for roughness_index in 0..FIBRE_TABLE_ROUGHNESSES {
        let width = table_roughness(roughness_index).powi(2);
        for column in 0..FIBRE_TABLE_ANGLES {
            let sine = table_sine(column);
            let (all, mean_shift) = energies(sine, &colours, width, FIBRE_TABLE_TILT);
            for (albedo_index, sums) in all.iter().enumerate() {
                let row = roughness_index * FIBRE_TABLE_ALBEDOS + albedo_index;
                table[row * FIBRE_TABLE_ANGLES + column] = [
                    sums[2] as f32,
                    sums[3] as f32,
                    sums[4] as f32,
                    sums[5] as f32,
                ];
                let (shine, transmitted, second) = forward_azimuths(sine, colours[albedo_index]);
                table[(spread_rows + row) * FIBRE_TABLE_ANGLES + column] =
                    [transmitted as f32, second as f32, 0.0, 0.0];
                if albedo_index == 0 {
                    // (R never enters the fibre: its numbers are any albedo's)
                    table[(shine_rows + roughness_index) * FIBRE_TABLE_ANGLES + column] = [
                        sums[0] as f32,
                        sums[1] as f32,
                        (mean_shift / FIBRE_TABLE_TILT) as f32,
                        shine as f32,
                    ];
                }
            }
        }
    }
    table
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fibre_sends_on_no_more_light_than_falls_on_it() {
        // every lobe's energy, forward and back, at any inclination, roughness and colour: a
        // white fibre passes most of it on, a black one only what its cuticle reflects
        for roughness_index in [0, 4, 7] {
            let width = table_roughness(roughness_index).powi(2);
            for sine in [0.05, 0.5, 0.9] {
                let (both, _) = energies(sine, &[1.0, 0.0], width, FIBRE_TABLE_TILT);
                let total: f64 = both[0].iter().sum();
                // (Karis's lobes are fits, not quite energy-conserving: at grazing light on the
                // smoothest fibres they give back up to 6 % more than falls on them; the shader
                // keeps āf under 1 before Ā_b divides by 1 − āf²)
                assert!(
                    total < 1.08,
                    "a white fibre gives back {total} at {sine}, {roughness_index}"
                );
                let black = both[1];
                let white_through = both[0][2] + both[0][3] + both[0][4] + both[0][5];
                let black_through = black[2] + black[3] + black[4] + black[5];
                // (Karis's TT raises the colour to as little as a quarter: a black fibre still
                // passes a little, as the shader draws it)
                assert!(
                    black_through < 0.25 * white_through,
                    "a black fibre passes {black_through} through, a white one {white_through}"
                );
                assert!(black[0] + black[1] > 0.0, "its cuticle still reflects");
            }
        }
    }

    #[test]
    fn a_light_fibre_sends_more_forward_than_back() {
        // TT, the strong forward lobe: why light hair glows from behind (Zinke 2008 §3.1); a
        // darker fibre passes less of TT and TRT both
        let (both, _) = energies(0.1, &[0.8, 0.2], 0.3 * 0.3, FIBRE_TABLE_TILT);
        let light = both[0];
        let forward = light[0] + light[2] + light[4];
        let back = light[1] + light[3] + light[5];
        assert!(forward > 1.5 * back, "forward {forward}, back {back}");
        assert!(both[1][2] < light[2] && both[1][5] < light[5]);
    }

    #[test]
    fn energies_for_many_colours_are_each_colours_own() {
        // the all-albedos integration against the lobes taken one colour at a time
        let width = 0.5 * 0.5;
        let (both, _) = energies(0.3, &[0.25, 0.7], width, FIBRE_TABLE_TILT);
        for (index, colour) in [0.25, 0.7].iter().enumerate() {
            let (one, _) = energies(0.3, &[*colour], width, FIBRE_TABLE_TILT);
            for lobe in 0..6 {
                assert!((one[0][lobe] - both[index][lobe]).abs() < 1e-12);
            }
        }
        // and against `lobes` summed directly over a coarse grid
        let (direct, _) = energies(0.3, &[0.7], width, FIBRE_TABLE_TILT);
        let mut through_forward = 0.0;
        let steps = 96;
        for angle in 0..steps {
            let theta = -0.5 * PI + PI * (angle as f64 + 0.5) / steps as f64;
            for azimuth in 0..steps {
                let phi = -PI + 2.0 * PI * (azimuth as f64 + 0.5) / steps as f64;
                if phi.cos() < 0.0 {
                    let (_, transmitted, _) =
                        lobes(0.3, theta.sin(), phi.cos(), 0.7, width, FIBRE_TABLE_TILT);
                    through_forward +=
                        transmitted * theta.cos() * (PI / steps as f64) * (2.0 * PI / steps as f64);
                }
            }
        }
        assert!((through_forward / PI - direct[0][2]).abs() < 1e-9);
    }

    #[test]
    fn the_table_holds_what_its_rows_say() {
        let table = fibre_table();
        assert_eq!(table.len(), FIBRE_TABLE_ROWS * FIBRE_TABLE_ANGLES);
        let (sums, shift) = energies(
            table_sine(3),
            &[table_albedo(9)],
            table_roughness(5).powi(2),
            FIBRE_TABLE_TILT,
        );
        let row = 5 * FIBRE_TABLE_ALBEDOS + 9;
        let texel = table[row * FIBRE_TABLE_ANGLES + 3];
        assert!((f64::from(texel[0]) - sums[0][2]).abs() < 1e-6);
        assert!((f64::from(texel[3]) - sums[0][5]).abs() < 1e-6);
        let shine =
            table[(2 * FIBRE_TABLE_ROUGHNESSES * FIBRE_TABLE_ALBEDOS + 5) * FIBRE_TABLE_ANGLES + 3];
        assert!((f64::from(shine[1]) - sums[0][1]).abs() < 1e-6);
        assert!((f64::from(shine[2]) - shift / FIBRE_TABLE_TILT).abs() < 1e-4);
        // R's light sent back is shifted toward the root (Marschner's −2 tilts or so)
        assert!(shine[2] < 0.0, "R's mean shift {}", shine[2]);
    }

    #[test]
    fn the_table_is_built_quickly() {
        // it is built when the GPU painter starts: under a second in a release build
        let started = std::time::Instant::now();
        let _ = build();
        let took = started.elapsed().as_secs_f64();
        eprintln!("fibre table built in {took:.3} s");
        assert!(
            took < 30.0,
            "{took} s (debug builds are slow; release is the budget)"
        );
    }
}
