//! Ground decals — draped, alpha-feathered surface patches.
//!
//! A decal is the ground-cover primitive the world XML uses for a plaza
//! floor, a market apron or a fusion disc at a road junction. Every one of
//! them shares the same three contracts, which is why they live in one
//! generator instead of one hand-rolled mesh per caller:
//!
//! * **Draped, not planar.** A `<Plane>` decal is a single quad at a fixed
//!   `y`; on anything but a perfectly flat pad it clips into the ground on
//!   one side and floats on the other. A decal samples the [`BrushGrid`] at
//!   every vertex and carries **concentric rings** so the interior follows
//!   the terrain too — a centre fan alone is planar between the hub and the
//!   rim, which is what made the 16 m junction discs cut through the slope.
//! * **Organic rim.** The rim radius is modulated by three periodic
//!   harmonics (integer frequencies ⇒ the seam closes exactly), so a patch
//!   reads as a worn area of ground and not as a stamped circle.
//! * **Feathered, never hard-edged.** Alpha ramps 1 → 0 across the feather
//!   band with a smootherstep, so the patch dissolves into the terrain
//!   instead of ending on a cut line.
//! * **Stacked above the ones they sit on.** Two decals draped from the same
//!   grid at the same [`DECAL_LIFT`] are coplanar, and their meshes do not
//!   share a triangulation: the depth test picks a different winner per
//!   triangle and the overlap shimmers as the camera moves. [`stack_decal_lifts`]
//!   raises each decal above the larger ones it overlaps, so a dirt pit, a
//!   wear trail or a market apron always wins over the plaza floor under it
//!   instead of z-fighting with it.
//!
//! Decals are transparent and **never cast shadows** — a shadow caster
//! hovering `LIFT` above the ground draws its own silhouette back onto the
//! ground, which is exactly the dark ring that made the north-gate junction
//! disc read as a separate object sitting on the grass.

use bevy::math::Vec2;

use super::brush::BrushGrid;
use super::mesh::ChunkMeshData;

/// Lift above the sampled ground (meters). Decals sit **below** the road
/// ribbon so that where the two overlap the ribbon wins deterministically
/// instead of z-fighting; see [`super::roads::RIBBON_LIFT`]. Overlapping
/// decals start here and climb by [`stack_decal_lifts`].
pub const DECAL_LIFT: f32 = 0.04;

/// Headroom kept between the top of a decal stack and the road ribbon
/// (meters). The ribbon's skirt dips below its surface, so the stack must not
/// reach it; 4 mm also leaves the stack's own steps room to be visible to the
/// depth test at the far plane.
const DECAL_STACK_HEADROOM: f32 = 0.004;
/// Floor for the vertical step between two layers of the stack (meters). Only
/// reached by pathological worlds with dozens of nested decals; the step is
/// otherwise spread so the whole stack ends just under the ribbon.
const DECAL_STACK_MIN_STEP: f32 = 0.0005;
/// How close two decal cores may come before they count as overlapping
/// (meters). Catches the plaza/street decals that meet exactly at the rim: the
/// two meshes share that seam and would otherwise flicker along it.
const DECAL_STACK_SLACK: f32 = 0.25;

/// Target arc length between rim samples (meters).
const RIM_ARC: f32 = 1.5;
/// Rim sample count bounds.
const MIN_SEGMENTS: usize = 24;
const MAX_SEGMENTS: usize = 96;
/// Concentric rings inside the opaque core bounds. The count scales with the
/// patch size so the drape resolution stays near the terrain texel.
const MIN_CORE_RINGS: usize = 2;
const MAX_CORE_RINGS: usize = 12;
/// Rings across the feather band — 3 keeps the alpha ramp curved rather than
/// showing the straight-line banding a single quad ring produces.
const FEATHER_RINGS: usize = 3;

/// A draped ground patch (`<GroundDecal>`, and the fusion disc at a road
/// junction).
#[derive(Debug, Clone, PartialEq)]
pub struct GroundDecalSpec {
    pub name: Option<String>,
    /// World XZ centre.
    pub at: Vec2,
    /// Ellipse radii in X and Z (meters) — equal radii give a circle.
    pub half_extent: Vec2,
    /// Alpha fade band outside the rim (meters).
    pub feather: f32,
    /// Rim wobble as a fraction of the radius (0 = a true ellipse).
    pub noise: f32,
    /// Determinism seed for the rim harmonics.
    pub seed: u32,
    /// Base color map (world asset path).
    pub texture: Option<String>,
    /// Meters per texture repeat — world-space UVs, so a decal and the road
    /// ribbons that cross it keep the same stone grid.
    pub texture_scale: f32,
    /// Material tint under the texture.
    pub base_color: [f32; 3],
    pub roughness: f32,
    /// Lift above the sampled ground (meters).
    pub lift: f32,
}

impl Default for GroundDecalSpec {
    fn default() -> Self {
        Self {
            name: None,
            at: Vec2::ZERO,
            half_extent: Vec2::splat(6.0),
            feather: 2.5,
            noise: 0.1,
            seed: 0,
            texture: None,
            texture_scale: 9.0,
            base_color: [0.62, 0.60, 0.56],
            roughness: 0.9,
            lift: DECAL_LIFT,
        }
    }
}

impl GroundDecalSpec {
    /// Outermost reach including the feather band (meters) — the radius a
    /// caller must clear to know the decal covers a point.
    pub fn outer_reach(&self) -> f32 {
        self.half_extent.max_element() * (1.0 + self.noise) + self.feather.max(0.0)
    }

    /// True when `p` lies inside the opaque core (rim wobble ignored, so the
    /// test is conservative: it only reports points the decal certainly
    /// covers). Used to suppress a junction disc a plaza floor already hides.
    pub fn covers(&self, p: Vec2) -> bool {
        let h = self.half_extent * (1.0 - self.noise).max(0.0);
        if h.x <= 0.0 || h.y <= 0.0 {
            return false;
        }
        let d = (p - self.at) / h;
        d.length_squared() <= 1.0
    }

    /// Footprint of the opaque core (m², π omitted — only the order matters).
    fn area(&self) -> f32 {
        self.half_extent.x.max(0.0) * self.half_extent.y.max(0.0)
    }

    /// Radius of the wobbled core in the direction of the unit vector `dir`
    /// (meters) — the support function of the ellipse that bounds the rim.
    fn support(&self, dir: Vec2) -> f32 {
        let r = self.half_extent * (1.0 + self.noise.clamp(0.0, 0.9));
        ((r.x * dir.x).powi(2) + (r.y * dir.y).powi(2)).sqrt()
    }

    /// True when the two opaque cores can share ground: the centre line is
    /// shorter than the two cores together (plus [`DECAL_STACK_SLACK`]).
    /// Deliberately forgiving — a false positive costs a few millimetres of
    /// lift, a false negative brings the z-fight back.
    pub fn overlaps(&self, other: &Self) -> bool {
        let delta = other.at - self.at;
        let dist = delta.length();
        if dist <= f32::EPSILON {
            return true;
        }
        let dir = delta / dist;
        dist <= self.support(dir) + other.support(dir) + DECAL_STACK_SLACK
    }
}

/// Raises each decal above the larger decals it overlaps, so overlapping
/// patches stack in a deterministic order instead of sharing a plane.
///
/// A decal's *depth* is one more than the deepest decal below it — the
/// longest chain of larger cores it overlaps (equal areas break the tie by
/// declaration order, earlier below). Sibling patches that all sit on the same
/// floor share one level instead of each adding a step. The step is spread
/// over the headroom between [`DECAL_LIFT`] and [`super::roads::RIBBON_LIFT`],
/// so the whole stack ends just below the road ribbon whatever the depth — a
/// plaza floor keeps 4 cm, a dirt pit on top of it sits a few millimetres
/// higher, and the ribbon still wins over both.
///
/// Bigger means lower on purpose: a plaza floor is the base, the apron, wear
/// trail or tree pit painted on it goes above. Authored `lift` values are
/// kept as the starting point and only ever raised, clamped below the ribbon.
pub fn stack_decal_lifts(decals: &mut [GroundDecalSpec]) {
    let n = decals.len();
    if n < 2 {
        return;
    }
    // Largest core first: every "below" decal (larger area, or equal area with
    // a smaller index) is already resolved when its turn comes, so one pass
    // gives the longest chain.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        decals[b]
            .area()
            .total_cmp(&decals[a].area())
            .then_with(|| a.cmp(&b))
    });
    let mut depth = vec![0usize; n];
    for &i in &order {
        let mut below = 0;
        for &j in &order {
            if j == i {
                continue;
            }
            // `j` is below `i` only when it comes earlier in `order` (larger
            // core, or the same core with a smaller index).
            let (mine, theirs) = (decals[i].area(), decals[j].area());
            let is_below = theirs > mine || (theirs == mine && j < i);
            if is_below && decals[j].overlaps(&decals[i]) {
                below = below.max(depth[j] + 1);
            }
        }
        depth[i] = below;
    }
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    if max_depth == 0 {
        return;
    }
    let ceiling = super::roads::RIBBON_LIFT - DECAL_STACK_HEADROOM;
    let headroom = (ceiling - DECAL_LIFT).max(0.0);
    let step = (headroom / max_depth as f32).max(DECAL_STACK_MIN_STEP);
    for (i, decal) in decals.iter_mut().enumerate() {
        if depth[i] == 0 {
            continue;
        }
        decal.lift = (decal.lift + depth[i] as f32 * step).min(ceiling);
    }
}

/// Deterministic `[0, 1)` hash of an integer.
fn hash01(i: u32) -> f32 {
    let mut h = i.wrapping_mul(0x27d4_eb2d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x8549_5cb5);
    h ^= h >> 16;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

/// Rim wobble in `[-1, 1]`, **exactly periodic** in `angle` (integer
/// harmonics), so the last rim sample meets the first without a crack.
fn rim_wobble(seed: u32, angle: f32) -> f32 {
    use std::f32::consts::TAU;
    let p1 = hash01(seed) * TAU;
    let p2 = hash01(seed ^ 0x9e37_79b9) * TAU;
    let p3 = hash01(seed ^ 0x85eb_ca6b) * TAU;
    0.55 * (angle * 3.0 + p1).sin()
        + 0.30 * (angle * 5.0 + p2).sin()
        + 0.15 * (angle * 8.0 + p3).sin()
}

/// `smootherstep` on `[0, 1]` — the alpha ramp across the feather band.
fn smootherstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Builds the decal mesh draped on `grid`.
///
/// Layout: vertex 0 is the centre hub, then `rings × segments` ring vertices.
/// Rings `1..=core_rings` climb from the hub to the wobbled rim at alpha 1;
/// rings beyond that walk outward across the feather band to alpha 0. The rim
/// is closed by modular indexing (no duplicated seam column) — world-space
/// UVs mean there is no UV seam to split for.
pub fn ground_decal_mesh(grid: &BrushGrid, spec: &GroundDecalSpec) -> ChunkMeshData {
    let mut mesh = ChunkMeshData::default();
    let hx = spec.half_extent.x;
    let hz = spec.half_extent.y;
    if hx <= 0.0 || hz <= 0.0 {
        return mesh;
    }
    let scale = if spec.texture_scale > 0.0 {
        spec.texture_scale
    } else {
        1.0
    };
    let feather = spec.feather.max(0.0);
    let noise = spec.noise.clamp(0.0, 0.9);
    let lift = spec.lift;

    let max_r = hx.max(hz);
    let segments = ((std::f32::consts::TAU * max_r / RIM_ARC).ceil() as usize)
        .clamp(MIN_SEGMENTS, MAX_SEGMENTS);
    let core_rings =
        ((max_r / grid.texel().max(0.5)).ceil() as usize).clamp(MIN_CORE_RINGS, MAX_CORE_RINGS);
    let feather_rings = if feather > 0.0 { FEATHER_RINGS } else { 0 };
    let rings = core_rings + feather_rings;

    let mut push = |x: f32, z: f32, alpha: f32| {
        let y = grid.sample(x, z) + lift;
        mesh.positions.push([x, y, z]);
        mesh.normals
            .push(grid.sample_normal(x, z, grid.texel()).to_array());
        mesh.uvs.push([x / scale, z / scale]);
        mesh.colors.push([1.0, 1.0, 1.0, alpha]);
    };

    // Hub.
    push(spec.at.x, spec.at.y, 1.0);

    // Ring vertices. `rim` is the wobbled ellipse point for this angle; core
    // rings scale it toward the hub, feather rings push it outward along the
    // same radial by `feather` meters in total.
    for j in 1..=rings {
        for k in 0..segments {
            let angle = (k as f32) / (segments as f32) * std::f32::consts::TAU;
            let (dx, dz) = (angle.cos(), angle.sin());
            let wobble = 1.0 + noise * rim_wobble(spec.seed, angle);
            let rim = Vec2::new(dx * hx, dz * hz) * wobble;
            let rim_len = rim.length().max(1e-4);
            let (offset, alpha) = if j <= core_rings {
                let t = (j as f32) / (core_rings as f32);
                (rim * t, 1.0)
            } else {
                let f = ((j - core_rings) as f32) / (feather_rings as f32);
                (rim * (1.0 + f * feather / rim_len), 1.0 - smootherstep01(f))
            };
            push(spec.at.x + offset.x, spec.at.y + offset.y, alpha);
        }
    }

    // Indices. Ring `j` starts at `1 + (j - 1) * segments`; `%segments`
    // closes the loop. Winding matches the road ribbon: +Y normal seen from
    // above.
    let idx = |j: usize, k: usize| -> u32 { (1 + (j - 1) * segments + (k % segments)) as u32 };
    for k in 0..segments {
        // Hub fan.
        mesh.indices
            .extend_from_slice(&[0, idx(1, k + 1), idx(1, k)]);
    }
    for j in 1..rings {
        for k in 0..segments {
            let (a0, a1) = (idx(j, k), idx(j, k + 1));
            let (b0, b1) = (idx(j + 1, k), idx(j + 1, k + 1));
            mesh.indices.extend_from_slice(&[a0, a1, b1, a0, b1, b0]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 128×128 grid over 128 m with a rolling hill, so a decal that fails to
    /// drape shows up as a vertical error against the ground.
    fn test_grid() -> BrushGrid {
        let mut grid =
            BrushGrid::new(vec![0; 128 * 128], 128, 128, 128.0, 50.0, 0.0).expect("grid");
        grid.begin_stroke("base");
        for z in 0..128 {
            for x in 0..128 {
                let p = grid.cell_center(x, z);
                let h = 8.0 + 6.0 * (p.x * 0.05).sin() + 4.0 * (p.y * 0.04).cos();
                grid.set_cell_height(x, z, h);
            }
        }
        grid.commit_stroke();
        grid
    }

    fn spec() -> GroundDecalSpec {
        GroundDecalSpec {
            half_extent: Vec2::new(8.0, 6.0),
            feather: 2.5,
            noise: 0.12,
            seed: 7,
            texture_scale: 9.0,
            ..GroundDecalSpec::default()
        }
    }

    #[test]
    fn test_decal_mesh_is_non_empty_and_indexed() {
        let mesh = ground_decal_mesh(&test_grid(), &spec());
        assert!(!mesh.positions.is_empty());
        assert_eq!(mesh.positions.len(), mesh.normals.len());
        assert_eq!(mesh.positions.len(), mesh.uvs.len());
        assert_eq!(mesh.positions.len(), mesh.colors.len());
        assert_eq!(mesh.indices.len() % 3, 0);
        let max = *mesh.indices.iter().max().expect("indices");
        assert!(
            (max as usize) < mesh.positions.len(),
            "index {max} out of range for {} vertices",
            mesh.positions.len()
        );
    }

    /// The whole point of the concentric rings: every vertex hugs the ground,
    /// including the interior. A centre fan would only be exact at the hub
    /// and the rim.
    #[test]
    fn test_decal_drapes_on_the_terrain() {
        let grid = test_grid();
        let mesh = ground_decal_mesh(&grid, &spec());
        for p in &mesh.positions {
            let expected = grid.sample(p[0], p[2]) + DECAL_LIFT;
            assert!(
                (p[1] - expected).abs() < 1e-3,
                "vertex {p:?} off the ground (expected y {expected})"
            );
        }
    }

    /// Alpha must reach 0 on the outermost ring — that is what dissolves the
    /// patch into the terrain instead of ending it on a cut line.
    #[test]
    fn test_decal_feathers_to_zero_alpha() {
        let mesh = ground_decal_mesh(&test_grid(), &spec());
        let min = mesh
            .colors
            .iter()
            .map(|c| c[3])
            .fold(f32::INFINITY, f32::min);
        let max = mesh
            .colors
            .iter()
            .map(|c| c[3])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(min.abs() < 1e-6, "outer ring alpha should be 0, got {min}");
        assert!(
            (max - 1.0).abs() < 1e-6,
            "core alpha should be 1, got {max}"
        );
    }

    /// No feather ⇒ no feather rings, and the rim stays opaque.
    #[test]
    fn test_decal_without_feather_stays_opaque() {
        let mesh = ground_decal_mesh(
            &test_grid(),
            &GroundDecalSpec {
                feather: 0.0,
                ..spec()
            },
        );
        assert!(mesh.colors.iter().all(|c| (c[3] - 1.0).abs() < 1e-6));
    }

    /// The rim must not be a circle — that is the "stamped disc" look.
    #[test]
    fn test_rim_wobble_breaks_the_circle() {
        let s = GroundDecalSpec {
            half_extent: Vec2::splat(8.0),
            noise: 0.15,
            ..spec()
        };
        let mesh = ground_decal_mesh(&test_grid(), &s);
        // Core rim ring: the last opaque ring. Collect its radii.
        let radii: Vec<f32> = mesh
            .positions
            .iter()
            .map(|p| Vec2::new(p[0], p[2]).distance(s.at))
            .collect();
        let spread = radii.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b))
            - radii.iter().fold(f32::INFINITY, |a, b| a.min(*b));
        assert!(spread > 1.0, "rim radii too uniform (spread {spread})");
    }

    /// Periodic harmonics: the wobble at angle 0 and at TAU must agree, or
    /// the rim shows a crack at the seam.
    #[test]
    fn test_rim_wobble_is_periodic() {
        for seed in [0u32, 7, 1234, 0xdead_beef] {
            let a = rim_wobble(seed, 0.0);
            let b = rim_wobble(seed, std::f32::consts::TAU);
            assert!((a - b).abs() < 1e-4, "seed {seed}: {a} != {b}");
        }
    }

    #[test]
    fn test_covers_is_conservative() {
        let s = spec();
        assert!(s.covers(s.at), "the centre is always covered");
        assert!(
            !s.covers(s.at + Vec2::new(s.outer_reach() + 1.0, 0.0)),
            "a point past the feather is not covered"
        );
    }

    #[test]
    fn test_degenerate_extent_yields_no_mesh() {
        let mesh = ground_decal_mesh(
            &test_grid(),
            &GroundDecalSpec {
                half_extent: Vec2::ZERO,
                ..spec()
            },
        );
        assert!(mesh.positions.is_empty());
        assert!(mesh.indices.is_empty());
    }

    /// The plaza floor with a dirt pit on its rim — the Vael case: the two
    /// cores overlap, they sit on the same `at` grid, so without stacking both
    /// meshes are coplanar and the overlap shimmers.
    fn plaza_and_pit() -> (GroundDecalSpec, GroundDecalSpec) {
        let floor = GroundDecalSpec {
            at: Vec2::ZERO,
            half_extent: Vec2::splat(17.0),
            feather: 3.5,
            noise: 0.11,
            ..GroundDecalSpec::default()
        };
        let pit = GroundDecalSpec {
            at: Vec2::new(-13.0, 11.0),
            half_extent: Vec2::splat(2.8),
            feather: 1.2,
            noise: 0.2,
            ..GroundDecalSpec::default()
        };
        (floor, pit)
    }

    #[test]
    fn test_stacking_raises_the_smaller_patch_above_the_floor() {
        let (floor, pit) = plaza_and_pit();
        assert!(floor.overlaps(&pit), "the pit sits on the plaza rim");
        let mut decals = vec![floor, pit];
        stack_decal_lifts(&mut decals);
        assert_eq!(decals[0].lift, DECAL_LIFT, "the floor keeps the base lift");
        assert!(
            decals[1].lift > decals[0].lift,
            "the pit must sit above the floor: {:?}",
            decals.iter().map(|d| d.lift).collect::<Vec<_>>()
        );
        for d in &decals {
            assert!(
                d.lift < super::super::roads::RIBBON_LIFT,
                "decal lift {} must stay under the ribbon",
                d.lift
            );
        }
    }

    /// The mesh is what the depth test sees: after stacking, a shared ground
    /// point has the two surfaces at different heights by the whole step.
    #[test]
    fn test_stacked_meshes_are_not_coplanar() {
        let (floor, pit) = plaza_and_pit();
        let mut decals = vec![floor, pit];
        stack_decal_lifts(&mut decals);
        let grid = test_grid();
        let high = ground_decal_mesh(&grid, &decals[1]);
        // The pit's hub (vertex 0) is the shared point; the floor's surface
        // there is the same grid sample at the floor's lift.
        let p = decals[1].at;
        let low_surface = grid.sample(p.x, p.y) + decals[0].lift;
        let high_surface = high.positions[0][1];
        assert!(
            high_surface - low_surface > 1e-3,
            "stacked decals must not share a plane (low {low_surface}, high {high_surface})"
        );
    }

    #[test]
    fn test_disjoint_decals_keep_the_default_lift() {
        let a = spec();
        let b = GroundDecalSpec {
            at: Vec2::new(60.0, 60.0),
            ..spec()
        };
        assert!(!a.overlaps(&b));
        let mut decals = vec![a, b];
        stack_decal_lifts(&mut decals);
        assert!(decals.iter().all(|d| d.lift == DECAL_LIFT));
    }

    /// A deep nest must still end under the ribbon, and every layer must be
    /// strictly above the one below it.
    #[test]
    fn test_deep_stack_stays_below_the_ribbon() {
        let mut decals: Vec<GroundDecalSpec> = (0..6)
            .map(|i| GroundDecalSpec {
                at: Vec2::ZERO,
                half_extent: Vec2::splat(20.0 - i as f32 * 2.5),
                feather: 0.5,
                noise: 0.0,
                ..GroundDecalSpec::default()
            })
            .collect();
        stack_decal_lifts(&mut decals);
        for (i, d) in decals.iter().enumerate() {
            assert!(
                d.lift < super::super::roads::RIBBON_LIFT,
                "layer {i} reaches the ribbon: {}",
                d.lift
            );
            if i > 0 {
                assert!(
                    d.lift > decals[i - 1].lift,
                    "layer {i} is not above layer {}: {} <= {}",
                    i - 1,
                    d.lift,
                    decals[i - 1].lift
                );
            }
        }
    }

    /// Two patches of the same size must still stack deterministically:
    /// declaration order breaks the tie, earlier below.
    #[test]
    fn test_equal_areas_break_the_tie_by_declaration_order() {
        let a = spec();
        let b = GroundDecalSpec {
            at: Vec2::new(4.0, 0.0),
            ..spec()
        };
        let mut decals = vec![a.clone(), b.clone()];
        stack_decal_lifts(&mut decals);
        assert_eq!(decals[0].lift, DECAL_LIFT);
        assert!(decals[1].lift > decals[0].lift);

        let mut swapped = vec![b, a];
        stack_decal_lifts(&mut swapped);
        assert_eq!(swapped[0].lift, DECAL_LIFT);
        assert!(swapped[1].lift > swapped[0].lift);
    }

    /// Stacking is a pure function of the set of decals: the same world
    /// always gets the same lifts, whatever order the parser produced.
    #[test]
    fn test_stacking_is_deterministic_and_order_independent() {
        let (floor, pit) = plaza_and_pit();
        let wear = GroundDecalSpec {
            at: Vec2::new(9.0, -9.0),
            half_extent: Vec2::new(5.0, 4.0),
            feather: 3.0,
            noise: 0.16,
            ..GroundDecalSpec::default()
        };
        let mut a = vec![floor.clone(), pit.clone(), wear.clone()];
        stack_decal_lifts(&mut a);
        let mut b = vec![wear, pit, floor];
        stack_decal_lifts(&mut b);
        // Same area ⇒ same depth ⇒ same lift, regardless of slot.
        let by_area = |v: &Vec<GroundDecalSpec>| {
            let mut lifts: Vec<(u32, f32)> = v
                .iter()
                .map(|d| (d.area().to_bits(), d.lift))
                .collect();
            lifts.sort_by_key(|(a, _)| *a);
            lifts.into_iter().map(|(_, l)| l).collect::<Vec<_>>()
        };
        assert_eq!(by_area(&a), by_area(&b));
    }
}
