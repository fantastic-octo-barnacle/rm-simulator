// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! From a verified CAD package to a `FieldConfig`: rune hubs and outpost
//! origins from the placements, the visual meshes as static trimeshes
//! (ground, rune frame and outpost towers, base and tech core), and the
//! chassis set down on the terrain.
use crate::cad_assets::CadAssets;
use crate::collision_mesh::{self, CollisionMesh, CollisionPart, GroundMesh};
use crate::math::{gltf_child, quat_axis_angle, rotate};
use anyhow::ensure;
use rm_simulator_world::{
    ChassisConfig, ChassisPlacement, Field, FieldConfig, OutpostConfig, Pose, RefereeConfig,
    RobotKind, RuneConfig, RuneKind, Team, set_tick_ns, tick_ns_for_hz,
};

/// Rune face pivots inside the extracted rune asset (glTF axes, metres).
pub const RUNE_FACE_PIVOTS_M: [[f64; 3]; 2] = [
    [-0.035_830_23, 0.093, -0.270_318_74],
    [-0.035_832_06, 0.093, 0.367_281_26],
];
/// Blade fronts sit 12.2 mm ahead of the face pivot; targets live on that plane.
pub const RUNE_FACE_FRONT_M: f64 = 0.0122;
/// Start position and heading (degrees, counter-clockwise from +x) for a
/// team's pilot: on its own half just before the centre-line plateau,
/// looking down the field towards the opponent.
pub fn default_spawn(team: Team) -> ([f64; 3], f64) {
    match team {
        Team::Red => ([9.0, 0.0, 1.0], 180.0),
        Team::Blue => ([-9.0, 0.0, 1.0], 0.0),
    }
}

/// Team whose half of the field a point lies in. The V1.2.0 CAD paints its
/// red marking sheets on the +x half and the blue ones on the -x half
/// (the rulebook's field figures place one team per end and name no
/// axis); the centre line goes to red.
pub fn side_team(x_m: f64) -> Team {
    if x_m >= 0.0 { Team::Red } else { Team::Blue }
}

/// Owner of a rune face: the team whose half its front normal faces
/// (section 4.3.2.2: one side of the rune is red's, the other blue's). The
/// hub pose's forward points into the wheel, away from that team.
pub fn rune_team(hub_pose: &Pose) -> Team {
    side_team(-rotate(hub_pose.rotation_wxyz, [1.0, 0.0, 0.0])[0])
}

/// Owner of an outpost: the team on whose half it stands (section 5.5.1).
pub fn outpost_team(origin: &Pose) -> Team {
    side_team(origin.translation_m[0])
}
/// Catch plane under the CAD ground (the crowned floor slab bottoms out about
/// 0.41 m below the playing floor at the side walls), so a body that slips
/// through a mesh seam does not fall forever.
pub const CATCH_FLOOR_M: f64 = -0.45;
/// Node containing the stationary equipment geometry.
const FIXED_NODE: &str = "static";

/// Rune hub pose for one face of the placed CAD rune. The rear face (index 0)
/// is turned about the asset's up axis so its front normal faces its own
/// viewer.
///
/// Panics if `face` is neither 0 nor 1.
pub fn cad_rune_hub(rune_root: &Pose, face: usize) -> Pose {
    rune_hub_at(rune_root, face, RUNE_FACE_PIVOTS_M[face])
}

/// Rune hub pose at an explicit face pivot in the placed rune's glTF frame.
/// Face 0 is turned a half turn about the asset's up axis; both hubs then sit
/// `RUNE_FACE_FRONT_M` forward of their pivot.
fn rune_hub_at(rune_root: &Pose, face: usize, origin_m: [f64; 3]) -> Pose {
    let flip = if face == 0 {
        quat_axis_angle([0.0, 1.0, 0.0], std::f64::consts::PI)
    } else {
        [1.0, 0.0, 0.0, 0.0]
    };
    let pivot = gltf_child(rune_root, origin_m, flip);
    gltf_child(&pivot, [0.0, 0.0, RUNE_FACE_FRONT_M], [1.0, 0.0, 0.0, 0.0])
}

/// Tower base pose of a placed CAD outpost: the asset's +x forward and +y up
/// become the tower's FLU forward and up.
pub fn cad_outpost_origin(outpost_root: &Pose) -> Pose {
    gltf_child(
        outpost_root,
        [0.0; 3],
        quat_axis_angle([0.0, 1.0, 0.0], -std::f64::consts::FRAC_PI_2),
    )
}

/// Everything rigid the chassis and projectiles collide with, in FLU metres.
pub struct Terrain {
    /// The perimeter walls that keep chassis inside the arena, in FLU metres.
    /// Projectile collision ignores them.
    pub boundary: CollisionMesh,
    /// Projectile bounds in FLU metres as `[low, high]`; a shot touching or
    /// crossing this XY perimeter is absorbed. `None` leaves the field without
    /// projectile bounds.
    pub projectile_bounds_m: Option<[[f64; 3]; 2]>,
    /// Moving mechanisms, one entry per placed prismatic joint.
    pub mechanisms: Vec<MechanismMesh>,
    /// The floor and arena visual meshes and the
    /// placed static scenery, as one triangle soup; also the ground the
    /// chassis is set down on.
    pub ground: GroundMesh,
    /// The rune frame and the outpost towers at their placements: the
    /// `static` node of each visual mesh. The faces, hubs and rotors are
    /// rule-driven bodies the field owns, so their nodes are left out.
    pub fixtures: CollisionMesh,
    /// The bases and tech cores at their placements, from their
    /// visual meshes, kept as trimeshes.
    pub equipment: Vec<CollisionMesh>,
}
/// One placed mechanism: a prismatic joint's moving subtree at its exported
/// rest pose, plus where the two travel limits put it.
#[derive(Clone, Debug)]
pub struct MechanismMesh {
    /// Moving subtree geometry in world FLU metres, at the exported rest pose.
    pub mesh: CollisionMesh,
    /// Referee mechanism that drives the travel.
    pub mechanism: rm_simulator_world::referee::Mechanism,
    /// Team the placement belongs to, from its x side.
    pub team: Team,
    /// Displacement of the subtree in FLU metres at fraction `0` and `1` of the
    /// mechanism's travel: the two joint limits scaled onto the world slide
    /// axis.
    pub offsets_m: [[f64; 3]; 2],
}
/// Prismatic mechanisms driven by referee controls or simulation time.
pub fn mechanism_joint(
    joint: &crate::semantics::Joint,
) -> Option<rm_simulator_world::referee::Mechanism> {
    use rm_simulator_world::referee::Mechanism;
    if joint.kind != "prismatic" || joint.limits.is_none() {
        return None;
    }
    match joint.id.as_str() {
        "base.shield.0.slide" | "base.shield.1.slide" | "base.shield.2.slide" => {
            Some(Mechanism::Base)
        }
        "dart-station.gate.slide" => Some(Mechanism::DartDoor),
        "base.dart_target.slide" => Some(Mechanism::DartTarget),
        _ => None,
    }
}
impl Terrain {
    /// One line for the start-up log.
    pub fn describe(&self) -> String {
        format!(
            "field collision: {} ground triangles, {} fixture triangles, {} equipment triangles (source tessellations or CAD visual meshes)",
            self.ground.triangles.len(),
            self.fixtures.triangles.len() + self.boundary.triangles.len(),
            self.equipment
                .iter()
                .map(|m| m.triangles.len())
                .sum::<usize>()
        )
    }
}

/// Build the collision terrain for a verified package.
///
/// The floor and arena scenery become the ground soup, with the placed static
/// scenery appended. The rune and outpost contribute only their `static` node;
/// the bases, tech cores and static scenery contribute their collision
/// primitives, and the prismatic joints the referee drives are pulled out into
/// `mechanisms` instead of staying in those static meshes. Everything is
/// placed in world FLU metres with the playing floor top at height zero.
pub fn load_terrain(cad: &CadAssets) -> anyhow::Result<Terrain> {
    let mut mechanisms = Vec::new();
    let mut ground =
        collision_mesh::load_glb(&cad.floor.physics_file(&cad.root))?.into_flu(cad.floor_top_cad_m);
    let scenery = collision_mesh::load_glb(&cad.arena_static.physics_file(&cad.root))?
        .into_flu(cad.floor_top_cad_m);
    let projectile_bounds_m = arena_boundary_bounds(&ground, &scenery)?;
    let boundary = arena_boundary(projectile_bounds_m);
    ground.append(scenery);
    for asset in &cad.static_assets {
        let mesh = load_asset_collision(cad, asset)?;
        load_mechanisms(cad, asset, &mut mechanisms)?;
        for placement in &asset.placements {
            ground.append(mesh.placed(placement));
        }
    }
    let mut fixtures = CollisionMesh::default();
    for (name, asset) in [("rune", &cad.rune), ("outpost", &cad.outpost)] {
        let fixed = if let Some(semantics) = asset.physics_semantics() {
            collision_mesh::load_glb_selected(
                &asset.physics_file(&cad.root),
                &semantics.fixed_primitives,
            )?
        } else {
            fixed_parts(collision_mesh::load_glb_parts(
                &asset.physics_file(&cad.root),
            )?)
        };
        ensure!(
            !fixed.triangles.is_empty(),
            "{name} visual mesh has no `{FIXED_NODE}` node"
        );
        for placement in &asset.placements {
            fixtures.append(fixed.placed(placement));
        }
    }
    let mut equipment = Vec::new();
    for asset in [&cad.base, &cad.tech_core] {
        let mesh = load_asset_collision(cad, asset)?;
        load_mechanisms(cad, asset, &mut mechanisms)?;
        for placement in &asset.placements {
            equipment.push(mesh.placed(placement));
        }
    }
    Ok(Terrain {
        boundary,
        projectile_bounds_m: Some(projectile_bounds_m),
        mechanisms,
        ground: ground.into(),
        fixtures,
        equipment,
    })
}

/// Playing boundary in FLU metres, shared by the fence and its colliders.
/// The slab includes an exterior apron. Fit XY to scenery vertices at slab
/// elevation, excluding elevated equipment overhangs. The 10 mm tolerance is
/// an asset-fitting setting, not a rulebook dimension. Empty scenery falls
/// back to the slab bounds for synthetic fields.
pub fn arena_boundary_bounds(
    floor: &CollisionMesh,
    scenery: &CollisionMesh,
) -> anyhow::Result<[[f64; 3]; 2]> {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for point in &floor.vertices_m {
        for i in 0..3 {
            low[i] = low[i].min(point[i]);
            high[i] = high[i].max(point[i]);
        }
    }
    ensure!(
        low.iter().chain(&high).all(|x| x.is_finite()) && high[0] > low[0] && high[1] > low[1],
        "floor has no finite arena boundary"
    );
    let mut edge_low = [f64::INFINITY; 2];
    let mut edge_high = [f64::NEG_INFINITY; 2];
    for point in &scenery.vertices_m {
        if point[2] >= low[2] - 0.01 && point[2] <= high[2] + 0.01 {
            for axis in 0..2 {
                edge_low[axis] = edge_low[axis].min(point[axis]);
                edge_high[axis] = edge_high[axis].max(point[axis]);
            }
        }
    }
    // Only use a footprint spanning the arena, not an isolated fixture.
    if (0..2).all(|axis| edge_high[axis] - edge_low[axis] > (high[axis] - low[axis]) * 0.5) {
        for axis in 0..2 {
            low[axis] = low[axis].max(edge_low[axis]);
            high[axis] = high[axis].min(edge_high[axis]);
        }
    }
    Ok([low, high])
}

/// Two metres high and 100 mm thick are simulator settings, not CAD dimensions.
fn arena_boundary([low, high]: [[f64; 3]; 2]) -> CollisionMesh {
    let mut mesh = CollisionMesh::default();
    for (a, b) in [
        (
            [low[0] - 0.1, low[1] - 0.1, low[2]],
            [low[0], high[1] + 0.1, high[2] + 2.0],
        ),
        (
            [high[0], low[1] - 0.1, low[2]],
            [high[0] + 0.1, high[1] + 0.1, high[2] + 2.0],
        ),
        (
            [low[0], low[1] - 0.1, low[2]],
            [high[0], low[1], high[2] + 2.0],
        ),
        (
            [low[0], high[1], low[2]],
            [high[0], high[1] + 0.1, high[2] + 2.0],
        ),
    ] {
        let base = mesh.vertices_m.len() as u32;
        for bits in 0..8 {
            mesh.vertices_m.push(std::array::from_fn(|axis| {
                if bits & (1 << axis) == 0 {
                    a[axis]
                } else {
                    b[axis]
                }
            }));
        }
        mesh.triangles.extend(
            [
                [0, 2, 1],
                [1, 2, 3],
                [4, 5, 6],
                [5, 7, 6],
                [0, 1, 4],
                [1, 5, 4],
                [2, 6, 3],
                [3, 6, 7],
                [0, 4, 2],
                [2, 4, 6],
                [1, 3, 5],
                [3, 7, 5],
            ]
            .map(|t| t.map(|i| i + base)),
        );
    }
    mesh
}

/// Collision triangles for one asset: the sidecar's collision primitives when
/// the asset has semantics, otherwise the whole GLB as one soup. Mechanism
/// subtrees are left out, because `load_mechanisms` adds them as moving meshes.
fn load_asset_collision(
    cad: &CadAssets,
    asset: &crate::cad_assets::CadAsset,
) -> anyhow::Result<CollisionMesh> {
    if let Some(semantics) = asset.physics_semantics() {
        let excluded: Vec<_> = semantics
            .joints
            .iter()
            .filter(|j| mechanism_joint(j).is_some())
            .map(|j| j.motion_node)
            .collect();
        collision_mesh::load_glb_filtered(
            &asset.physics_file(&cad.root),
            &semantics.collision_primitives,
            &excluded,
            None,
        )
    } else {
        collision_mesh::load_glb(&asset.physics_file(&cad.root))
    }
}

/// Append one `MechanismMesh` per placement for every joint the referee
/// drives. Geometry comes from the joint's motion subtree, and the offsets
/// from its limits and world slide axis. Joints that are not mechanisms, or
/// whose subtree has no collision triangles, are skipped.
fn load_mechanisms(
    cad: &CadAssets,
    asset: &crate::cad_assets::CadAsset,
    output: &mut Vec<MechanismMesh>,
) -> anyhow::Result<()> {
    let Some(semantics) = asset.physics_semantics() else {
        return Ok(());
    };
    for joint in &semantics.joints {
        let Some(mechanism) = mechanism_joint(joint) else {
            continue;
        };
        let mesh = collision_mesh::load_glb_filtered(
            &asset.physics_file(&cad.root),
            &semantics.collision_primitives,
            &[],
            Some(joint.motion_node),
        )?;
        if mesh.triangles.is_empty() {
            continue;
        }
        let axis = crate::math::rotate(
            [
                joint.rotation_xyzw[3],
                joint.rotation_xyzw[0],
                joint.rotation_xyzw[1],
                joint.rotation_xyzw[2],
            ],
            joint.axis,
        );
        for placement in &asset.placements {
            let axis = crate::math::rotate(crate::math::gltf_rotation(placement), axis);
            output.push(MechanismMesh {
                mesh: mesh.placed(placement),
                mechanism,
                team: side_team(placement.translation_m[0]),
                offsets_m: joint
                    .limits
                    .expect("bounded mechanism")
                    .map(|v| axis.map(|a| a * v)),
            });
        }
    }
    Ok(())
}

/// The parts of a rune or outpost visual that never move, as one soup.
fn fixed_parts(parts: Vec<CollisionPart>) -> CollisionMesh {
    let mut fixed = CollisionMesh::default();
    for part in parts {
        if part.name == FIXED_NODE {
            fixed.append(part.mesh);
        }
    }
    fixed
}

/// Hand the terrain to the field: the ground, fixtures and equipment as
/// the same triangles used to draw the CAD.
pub fn add_terrain(field: &mut Field, terrain: &Terrain) -> anyhow::Result<()> {
    field.remove_catch_floor();
    if let Some(bounds) = terrain.projectile_bounds_m {
        field.set_projectile_bounds(bounds)?;
    }
    if !terrain.boundary.triangles.is_empty() {
        field.add_boundary_mesh(
            terrain.boundary.vertices_m.clone(),
            terrain.boundary.triangles.clone(),
        )?;
    }
    for mesh in std::iter::once(&*terrain.ground)
        .chain(std::iter::once(&terrain.fixtures))
        .chain(&terrain.equipment)
        .filter(|mesh| !mesh.triangles.is_empty())
    {
        field.add_static_mesh(mesh.vertices_m.clone(), mesh.triangles.clone())?;
    }
    for part in &terrain.mechanisms {
        field.add_mechanism_mesh(
            part.mesh.vertices_m.clone(),
            part.mesh.triangles.clone(),
            part.mechanism,
            part.team,
            part.offsets_m,
        )?;
    }
    Ok(())
}

/// Set a chassis down on the highest ground below the requested point (the
/// flat floor without terrain), heading `yaw_deg` from FLU forward.
pub fn chassis_placement(
    config: ChassisConfig,
    kind: RobotKind,
    terrain: Option<&Terrain>,
    team: Team,
    spawn: [f64; 3],
    yaw_deg: f64,
) -> ChassisPlacement {
    let ground = terrain
        .and_then(|terrain| {
            terrain
                .ground
                .ground_height_below(spawn[0], spawn[1], spawn[2])
        })
        .unwrap_or(0.0);
    let spawn = Pose::yawed(
        [spawn[0], spawn[1], ground + config.rest_height_m() + 0.01],
        yaw_deg.to_radians(),
    );
    ChassisPlacement {
        config,
        spawn,
        team,
        kind,
    }
}

/// Sideways spacing between a team's spawn slots.
pub const SPAWN_SLOT_SPACING_M: f64 = 0.9;
/// Spawn point and heading for the `slot`th chassis of a team: the team's
/// default spawn, with later slots stepped left and right of it in turn so
/// bodies do not overlap.
pub fn spawn_slot(team: Team, slot: usize) -> ([f64; 3], f64) {
    let ([x, y, z], yaw_deg) = default_spawn(team);
    let step = slot.div_ceil(2) as f64 * SPAWN_SLOT_SPACING_M;
    let offset = if slot.is_multiple_of(2) { -step } else { step };
    ([x, y + offset, z], yaw_deg)
}

/// Places chassis for players as they arrive: one configuration, the
/// ground to set them down on, and the team spawn slots.
pub struct ChassisSpawner {
    /// Chassis configuration a chassis gets when no robot is named for it:
    /// training bots and the plain `slot` and `at` placements.
    pub config: ChassisConfig,
    /// Ground to set chassis down on; `None` uses the flat floor at height
    /// zero.
    pub terrain: Option<Terrain>,
}
impl ChassisSpawner {
    /// The placement for a team's `slot`th chassis, in the default
    /// configuration as an infantry.
    pub fn slot(&self, team: Team, slot: usize) -> ChassisPlacement {
        self.slot_with(self.config.clone(), RobotKind::Infantry, team, slot)
    }
    /// A placement at an explicit point, set down on the ground below it, in
    /// the default configuration as an infantry.
    pub fn at(&self, team: Team, spawn: [f64; 3], yaw_deg: f64) -> ChassisPlacement {
        self.at_with(
            self.config.clone(),
            RobotKind::Infantry,
            team,
            spawn,
            yaw_deg,
        )
    }
    /// The placement for a team's `slot`th chassis with an explicit
    /// configuration and robot class, for a pilot who chose a robot.
    pub fn slot_with(
        &self,
        config: ChassisConfig,
        kind: RobotKind,
        team: Team,
        slot: usize,
    ) -> ChassisPlacement {
        let (spawn, yaw_deg) = spawn_slot(team, slot);
        self.at_with(config, kind, team, spawn, yaw_deg)
    }
    /// A placement at an explicit point with an explicit configuration and
    /// robot class, set down on the ground below it.
    pub fn at_with(
        &self,
        config: ChassisConfig,
        kind: RobotKind,
        team: Team,
        spawn: [f64; 3],
        yaw_deg: f64,
    ) -> ChassisPlacement {
        chassis_placement(config, kind, self.terrain.as_ref(), team, spawn, yaw_deg)
    }
}

/// What to put on the CAD field besides its fixed geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutOptions {
    /// Rune rules on both CAD faces, or none (the CAD rune stays static).
    pub rune: Option<RuneKind>,
    /// Outpost tower spin rate in radians per second.
    pub outpost_speed_rad_s: f64,
    /// The CAD visual triangles are in the world (lowers the catch floor).
    pub terrain: bool,
    /// A referee owning the runes and outposts by field side (`rune_team`,
    /// `outpost_team`).
    pub referee: bool,
    /// Shared physics rate in Hz, one of [`rm_simulator_world::OFFERED_RATES_HZ`]. It names the
    /// tick length the whole process runs at; [`field_config`] freezes it, and
    /// an unoffered rate leaves the frozen default in place. Every peer in one
    /// match must agree on it, which the handshake enforces.
    pub physics_rate_hz: u32,
    /// Ball restitution, hard flight limit and low-speed retirement rule the
    /// hosted field runs. Defaults to the simulator's control behaviour.
    pub projectile_policy: rm_simulator_world::projectile::ProjectilePolicy,
}
impl Default for LayoutOptions {
    /// The shipped configuration: no rune, a still outpost, CAD terrain, a
    /// referee, the default 1 kHz physics rate and spent-ball retirement.
    fn default() -> Self {
        Self {
            rune: None,
            outpost_speed_rad_s: 0.0,
            terrain: true,
            referee: true,
            physics_rate_hz: DEFAULT_RATE_HZ,
            projectile_policy: Default::default(),
        }
    }
}

/// The shipped physics rate in Hz, matching `rm_simulator_world::tick_ns`'s
/// default of 1 ms per tick. It is the control path and the rollback value.
pub const DEFAULT_RATE_HZ: u32 = 1000;

/// The field configuration for a CAD package.
///
/// This is also where `options.physics_rate_hz` freezes the process-wide tick
/// length, before any field exists to read it. A rate outside
/// [`rm_simulator_world::OFFERED_RATES_HZ`], or a second call naming a different one, leaves the
/// already frozen value alone; callers that must refuse a mismatch compare
/// [`rm_simulator_world::tick_ns`] afterwards.
///
/// Panics if the rune asset has no placement.
pub fn field_config(cad: &CadAssets, options: &LayoutOptions) -> FieldConfig {
    if let Some(ns) = tick_ns_for_hz(options.physics_rate_hz) {
        set_tick_ns(ns);
    }
    let rune_root = &cad.rune.placements[0];
    let runes: Vec<RuneConfig> = options
        .rune
        .map(|kind| {
            (0..2)
                .map(|face| RuneConfig {
                    kind,
                    hub_pose: rune_hub_at(
                        rune_root,
                        face,
                        cad.rune
                            .visual_semantics()
                            .and_then(|s| {
                                s.joints
                                    .iter()
                                    .find(|j| j.id == format!("rune.face.{face}.spin"))
                            })
                            .map_or(RUNE_FACE_PIVOTS_M[face], |j| j.origin_m),
                    ),
                    cad_orbit: true,
                })
                .collect()
        })
        .unwrap_or_default();
    let outposts: Vec<OutpostConfig> = cad
        .outpost
        .placements
        .iter()
        .map(|root| OutpostConfig {
            pivot_cad_m: cad
                .outpost
                .visual_semantics()
                .and_then(|s| s.joints.iter().find(|j| j.id == "outpost.spin"))
                .map(|j| j.origin_m),
            origin: cad_outpost_origin(root),
            speed_rad_s: options.outpost_speed_rad_s,
        })
        .collect();
    FieldConfig {
        bases: vec![],
        floor_height_m: if options.terrain { CATCH_FLOOR_M } else { 0.0 },
        chassis: Vec::new(),
        referee: options.referee.then(|| {
            RefereeConfig::owned(
                runes.iter().map(|rune| rune_team(&rune.hub_pose)).collect(),
                outposts
                    .iter()
                    .map(|outpost| outpost_team(&outpost.origin))
                    .collect(),
            )
        }),
        runes,
        outposts,
        projectile_policy: options.projectile_policy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arena_boundary_follows_grounded_scenery_instead_of_the_slab_apron() {
        let floor = CollisionMesh {
            vertices_m: vec![[-14.5, -8.0, -0.2], [14.5, 8.0, 0.0]],
            ..Default::default()
        };
        let scenery = CollisionMesh {
            vertices_m: vec![
                [-14.0, -7.5, 0.0],
                [14.0, 7.5, 0.0],
                [-15.0, -8.5, 1.5],
                [15.0, 8.5, 1.5],
            ],
            ..Default::default()
        };
        let bounds = arena_boundary_bounds(&floor, &scenery).unwrap();
        assert_eq!(bounds, [[-14.0, -7.5, -0.2], [14.0, 7.5, 0.0]]);
        let walls = arena_boundary(bounds);
        assert!(
            walls
                .vertices_m
                .iter()
                .all(|p| p[0].abs() <= 14.1 && p[1].abs() <= 7.6)
        );
        assert_eq!(
            arena_boundary_bounds(&floor, &CollisionMesh::default()).unwrap(),
            [[-14.5, -8.0, -0.2], [14.5, 8.0, 0.0]]
        );
    }

    #[test]
    fn arena_walls_contain_driving_robots_and_do_not_add_an_infinite_floor() {
        use rm_simulator_world::ChassisCommand;
        let floor = CollisionMesh {
            vertices_m: vec![
                [-2.0, -1.0, 0.0],
                [2.0, -1.0, 0.0],
                [2.0, 1.0, 0.0],
                [-2.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        };
        let boundary =
            arena_boundary(arena_boundary_bounds(&floor, &CollisionMesh::default()).unwrap());
        assert_eq!(boundary.triangles.len(), 48);
        for (start, command) in [
            (
                [1.4, 0.0],
                ChassisCommand {
                    forward_m_s: 5.0,
                    ..Default::default()
                },
            ),
            (
                [-1.4, 0.0],
                ChassisCommand {
                    forward_m_s: -5.0,
                    ..Default::default()
                },
            ),
            (
                [0.0, 0.4],
                ChassisCommand {
                    left_m_s: 5.0,
                    ..Default::default()
                },
            ),
            (
                [0.0, -0.4],
                ChassisCommand {
                    left_m_s: -5.0,
                    ..Default::default()
                },
            ),
        ] {
            let mut field = Field::new(&FieldConfig {
                runes: vec![],
                outposts: vec![],
                ..Default::default()
            })
            .unwrap();
            field.remove_catch_floor();
            field
                .add_static_mesh(floor.vertices_m.clone(), floor.triangles.clone())
                .unwrap();
            field
                .add_static_mesh(boundary.vertices_m.clone(), boundary.triangles.clone())
                .unwrap();
            let config = ChassisConfig::default();
            let id = field
                .add_chassis(&ChassisPlacement {
                    team: Team::Red,
                    kind: rm_simulator_world::RobotKind::Infantry,
                    spawn: Pose::at([start[0], start[1], config.rest_height_m()]),
                    config,
                })
                .unwrap();
            field.command_chassis(id, command).unwrap();
            field.step(1500).unwrap();
            let p = field.snapshot().chassis[0].pose.translation_m;
            assert!(
                p[0].abs() < 2.0 && p[1].abs() < 1.0 && p[2] > 0.0,
                "escaped: {p:?}"
            );
            field.place_chassis(id, Pose::at([4.0, 0.0, 1.0])).unwrap();
            field.step(1000).unwrap();
            assert!(field.snapshot().chassis[0].pose.translation_m[2] < -2.0);
        }
    }

    use crate::cad_assets::CadAsset;
    use crate::math::{RENDER_BASIS_WXYZ, gltf_root_pose, quat_conjugate, quat_multiply};

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-4)
    }
    #[test]
    fn cad_rune_faces_get_opposing_front_normals_ahead_of_their_pivots() {
        // Rune asset placed with its front (+z) toward FLU forward: under
        // the renderer's basis glTF +z is FLU backward, so the placement is
        // a half turn about the renderer's up axis, read back through that
        // basis.
        let root = gltf_root_pose(
            [5.0, 0.0, 2.0],
            quat_multiply(
                quat_conjugate(RENDER_BASIS_WXYZ),
                quat_axis_angle([0.0, 1.0, 0.0], std::f64::consts::PI),
            ),
        );
        let front = cad_rune_hub(&root, 1);
        let rear = cad_rune_hub(&root, 0);
        // The hub pose's FLU forward is the direction into the wheel (opposite the front normal).
        assert!(close(
            rotate(front.rotation_wxyz, [1.0, 0.0, 0.0]),
            [-1.0, 0.0, 0.0]
        ));
        assert!(close(
            rotate(rear.rotation_wxyz, [1.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0]
        ));
        // Face 1 sits ahead of the pivot along its own normal; face 0 the other way.
        let z1 = RUNE_FACE_PIVOTS_M[1][2] + RUNE_FACE_FRONT_M;
        let z0 = RUNE_FACE_PIVOTS_M[0][2] - RUNE_FACE_FRONT_M;
        assert!(close(
            front.translation_m,
            [5.0 + z1, RUNE_FACE_PIVOTS_M[1][0], 2.093]
        ));
        assert!(close(
            rear.translation_m,
            [5.0 + z0, RUNE_FACE_PIVOTS_M[0][0], 2.093]
        ));
        // Both hubs keep up as up.
        assert!(close(
            rotate(front.rotation_wxyz, [0.0, 0.0, 1.0]),
            [0.0, 0.0, 1.0]
        ));
        assert!(close(
            rotate(rear.rotation_wxyz, [0.0, 0.0, 1.0]),
            [0.0, 0.0, 1.0]
        ));
    }
    #[test]
    fn cad_outpost_forward_follows_the_asset_x_axis() {
        // The blue outpost placement (see cad_assets tests).
        let n = (0.716_301_943_f64.powi(2) + 0.697_790_46_f64.powi(2)).sqrt();
        let root = gltf_root_pose(
            [3.0917, 3.8674, 0.1907],
            [0.0, 0.0, 0.716_301_943 / n, 0.697_790_46 / n],
        );
        let origin = cad_outpost_origin(&root);
        assert!(close(origin.translation_m, [3.0917, 3.8674, 0.1907]));
        assert!(close(
            rotate(origin.rotation_wxyz, [1.0, 0.0, 0.0]),
            [-1.0, 0.0, 0.0]
        ));
        let up = rotate(origin.rotation_wxyz, [0.0, 0.0, 1.0]);
        assert!(up[2] > 0.999, "{up:?}");
    }
    #[test]
    fn only_the_static_node_of_a_rune_or_outpost_visual_is_fixed() {
        let tri = |z: f64| CollisionMesh {
            vertices_m: vec![[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]],
            triangles: vec![[0, 1, 2]],
        };
        let parts = vec![
            CollisionPart {
                name: "face_0/blade_0".into(),
                mesh: tri(1.0),
            },
            CollisionPart {
                name: "static".into(),
                mesh: tri(2.0),
            },
            CollisionPart {
                name: "rotor/carrier".into(),
                mesh: tri(3.0),
            },
            CollisionPart {
                name: "static".into(),
                mesh: tri(4.0),
            },
        ];
        let fixed = fixed_parts(parts);
        assert_eq!(fixed.triangles, vec![[0, 1, 2], [3, 4, 5]]);
        assert!(fixed.vertices_m.iter().all(|v| v[2] == 2.0 || v[2] == 4.0));
        assert!(fixed_parts(Vec::new()).triangles.is_empty());
    }
    #[test]
    fn chassis_is_set_down_on_the_ground_below_the_spawn_point() {
        let config = ChassisConfig::default();
        let terrain = Terrain {
            boundary: CollisionMesh::default(),
            projectile_bounds_m: None,
            mechanisms: Vec::new(),
            ground: CollisionMesh {
                vertices_m: vec![
                    [-5.0, -5.0, 0.3],
                    [5.0, -5.0, 0.3],
                    [5.0, 5.0, 0.3],
                    [-5.0, 5.0, 0.3],
                ],
                triangles: vec![[0, 1, 2], [0, 2, 3]],
            }
            .into(),
            fixtures: CollisionMesh::default(),
            equipment: Vec::new(),
        };
        let placement = chassis_placement(
            config.clone(),
            RobotKind::Hero,
            Some(&terrain),
            Team::Red,
            [1.0, 2.0, 1.0],
            90.0,
        );
        assert_eq!(placement.kind, RobotKind::Hero);
        assert!(close(
            placement.spawn.translation_m,
            [1.0, 2.0, 0.3 + config.rest_height_m() + 0.01]
        ));
        assert!(close(
            rotate(placement.spawn.rotation_wxyz, [1.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0]
        ));
        // Without terrain the flat floor at zero is the ground.
        let flat = chassis_placement(
            config.clone(),
            RobotKind::Infantry,
            None,
            Team::Red,
            [1.0, 2.0, 1.0],
            0.0,
        );
        assert!(close(
            flat.spawn.translation_m,
            [1.0, 2.0, config.rest_height_m() + 0.01]
        ));
        // Above the mesh footprint there is no ground: the flat floor applies.
        let outside = chassis_placement(
            config.clone(),
            RobotKind::Infantry,
            Some(&terrain),
            Team::Blue,
            [9.0, 9.0, 1.0],
            0.0,
        );
        assert!(close(
            outside.spawn.translation_m,
            [9.0, 9.0, config.rest_height_m() + 0.01]
        ));
        assert_eq!(outside.team, Team::Blue);
        // Spawn slots step sideways from the team's default point.
        assert_eq!(spawn_slot(Team::Red, 0), default_spawn(Team::Red));
        assert_eq!(spawn_slot(Team::Blue, 1).0[1], SPAWN_SLOT_SPACING_M);
        assert_eq!(spawn_slot(Team::Blue, 2).0[1], -SPAWN_SLOT_SPACING_M);
        assert_eq!(spawn_slot(Team::Blue, 3).0[1], 2.0 * SPAWN_SLOT_SPACING_M);
        let spawner = ChassisSpawner {
            config: config.clone(),
            terrain: Some(terrain),
        };
        let second = spawner.slot(Team::Red, 1);
        assert!(close(
            second.spawn.translation_m,
            [9.0, SPAWN_SLOT_SPACING_M, config.rest_height_m() + 0.01]
        ));
    }
    #[test]
    fn layout_options_place_runes_outposts_and_the_referee() {
        let cad = CadAssets {
            root: std::path::PathBuf::new(),
            floor_top_cad_m: crate::cad_assets::FLOOR_TOP_CAD_M,
            collision_solids: true,
            floor: asset(0),
            arena_static: asset(0),
            rune: asset(1),
            outpost: asset(2),
            base: asset(2),
            tech_core: asset(2),
            static_assets: Vec::new(),
        };
        let mut cad = cad;
        cad.rune.placements[0] = gltf_root_pose(
            [5.0, 0.0, 2.0],
            quat_multiply(
                quat_conjugate(RENDER_BASIS_WXYZ),
                quat_axis_angle([0.0, 1.0, 0.0], std::f64::consts::PI),
            ),
        );
        cad.outpost.placements = vec![
            gltf_root_pose([3.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]),
            gltf_root_pose([-3.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]),
        ];
        let options = LayoutOptions {
            rune: Some(RuneKind::Big),
            outpost_speed_rad_s: 1.0,
            terrain: true,
            referee: true,
            physics_rate_hz: DEFAULT_RATE_HZ,
            projectile_policy: Default::default(),
        };
        let config = field_config(&cad, &options);
        assert_eq!(config.runes.len(), 2);
        assert!(
            config
                .runes
                .iter()
                .all(|r| r.kind == RuneKind::Big && r.cad_orbit)
        );
        assert_eq!(config.outposts.len(), 2);
        assert_eq!(config.floor_height_m, CATCH_FLOOR_M);
        let referee = config.referee.unwrap();
        // The rune stands across the field as in `cad_rune_faces`: face 0
        // looks towards -x (blue's half), face 1 towards +x. The outposts
        // stand at x = 3 and x = -3.
        assert_eq!(referee.rune_teams, [Team::Blue, Team::Red]);
        assert_eq!(referee.outpost_teams, [Team::Red, Team::Blue]);
        for (rune, team) in config.runes.iter().zip(&referee.rune_teams) {
            assert_eq!(rune_team(&rune.hub_pose), *team);
        }
        assert_eq!(side_team(0.0), Team::Red);
        assert_eq!(side_team(-0.1), Team::Blue);
        assert_eq!(default_spawn(Team::Red).0[0], 9.0);
        assert_eq!(default_spawn(Team::Blue), ([-9.0, 0.0, 1.0], 0.0));
        assert!(Field::new(&field_config(&cad, &options)).is_ok());
        let plain = field_config(
            &cad,
            &LayoutOptions {
                rune: None,
                terrain: false,
                referee: false,
                physics_rate_hz: DEFAULT_RATE_HZ,
                projectile_policy: Default::default(),
                ..options
            },
        );
        assert!(plain.runes.is_empty());
        assert!(plain.referee.is_none());
        assert_eq!(plain.floor_height_m, 0.0);
    }
    /// With the shipped package each team owns one rune face and one outpost,
    /// red on the +x half where the CAD paints its red markings.
    #[test]
    fn the_field_package_gives_each_team_one_rune_face_and_one_outpost() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let root = std::path::PathBuf::from(home).join("dev/RM/assets/rm2026-field");
        if !root.join("manifest.json").is_file() {
            return;
        }
        let cad = crate::cad_assets::load(&root).unwrap();
        let config = field_config(
            &cad,
            &LayoutOptions {
                rune: Some(RuneKind::Small),
                outpost_speed_rad_s: 1.0,
                terrain: false,
                referee: true,
                physics_rate_hz: DEFAULT_RATE_HZ,
                projectile_policy: Default::default(),
            },
        );
        let referee = config.referee.unwrap();
        let mut rune_teams = referee.rune_teams.clone();
        rune_teams.sort_by_key(|team| team.index());
        assert_eq!(rune_teams, [Team::Red, Team::Blue]);
        let mut outpost_teams = referee.outpost_teams.clone();
        outpost_teams.sort_by_key(|team| team.index());
        assert_eq!(outpost_teams, [Team::Red, Team::Blue]);
        for (outpost, team) in config.outposts.iter().zip(&referee.outpost_teams) {
            let x = outpost.origin.translation_m[0];
            assert_eq!(*team == Team::Red, x > 0.0, "outpost at x = {x}");
        }
        eprintln!(
            "rune faces {:?}, outposts {:?} at x = {:?}",
            referee.rune_teams,
            referee.outpost_teams,
            config
                .outposts
                .iter()
                .map(|o| o.origin.translation_m[0])
                .collect::<Vec<_>>()
        );
    }

    fn asset(placements: usize) -> CadAsset {
        CadAsset {
            semantics: None,
            file: "x.glb".into(),
            placements: (0..placements)
                .map(|i| gltf_root_pose([i as f64, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]))
                .collect(),
            collision: None,
            source_tessellated_collision: false,
        }
    }
}
