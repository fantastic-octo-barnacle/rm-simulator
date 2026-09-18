// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The physics geometry as a wireframe over or instead of the scenery (F3 panel).
use crate::session::Session;
use bevy::{
    asset::RenderAssetUsages,
    light::{NotShadowCaster, NotShadowReceiver},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use rm_simulator_render::{cad::SceneryVisibility, flu_position};
use rm_simulator_server::collision_mesh::CollisionMesh;
use rm_simulator_world::StaticGeometry;
use std::sync::{Mutex, mpsc};

/// How the physics geometry is shown: not at all, as a wireframe over the
/// scenery, or alone with the scenery hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum CollisionView {
    /// Draw no physics geometry; no capture is requested.
    Hidden,
    /// Draw the physics wireframe over the visible scenery.
    Overlay,
    /// Draw the physics wireframe with the scenery hidden once the mesh is ready.
    Alone,
}
/// Fixed geometry is requested once, on first use, and built off the UI thread.
#[derive(Resource)]
pub struct CollisionDebug {
    /// The selected view. The `C` key cycles it, the F3 panel radio group and
    /// the console `inspect` command set it, and `--collision-view` seeds it.
    /// Geometry comes from the client's verified local colliders, never a host
    /// capture.
    pub view: CollisionView,
    build: Build,
}
enum Build {
    Unrequested,
    Building(Mutex<mpsc::Receiver<Result<Mesh, String>>>),
    Ready,
    Unavailable,
    Failed(String),
}
impl CollisionDebug {
    /// Build the resource for `view`; no capture is requested until a frame
    /// needs one.
    pub fn new(view: CollisionView) -> Self {
        Self {
            view,
            build: Build::Unrequested,
        }
    }
    /// Keep the scenery visible until a requested wireframe is usable.
    fn scenery_visible(&self) -> bool {
        self.view != CollisionView::Alone || !matches!(self.build, Build::Ready)
    }
    /// True while a wireframe is wanted but its fixed geometry has not been
    /// requested or built yet.
    pub fn pending(&self) -> bool {
        self.view != CollisionView::Hidden
            && matches!(self.build, Build::Unrequested | Build::Building(_))
    }
    /// Why the fixed-geometry wireframe cannot be drawn, or `None` while the
    /// view is hidden, still building, or already drawn.
    pub fn failure(&self) -> Option<&str> {
        if self.view == CollisionView::Hidden {
            return None;
        }
        match &self.build {
            Build::Unavailable => {
                Some("collision wireframe unavailable: no verified collision geometry")
            }
            Build::Failed(error) => Some(error),
            _ => None,
        }
    }

    fn request(
        &mut self,
        geometry: impl FnOnce() -> Option<Box<dyn FnOnce() -> Result<StaticGeometry, String> + Send>>,
    ) {
        if self.view == CollisionView::Hidden || !matches!(self.build, Build::Unrequested) {
            return;
        }
        let Some(geometry) = geometry() else {
            self.build = Build::Unavailable;
            return;
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        self.build = match std::thread::Builder::new()
            .name("collision-wireframe".into())
            .spawn(move || {
                let geometry = match geometry() {
                    Ok(geometry) => geometry,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        return;
                    }
                };
                let (vertices_m, triangles) = geometry.into_triangles();
                let mesh = wireframe_mesh(&CollisionMesh {
                    vertices_m,
                    triangles,
                });
                let _ = sender.send(Ok(mesh));
            }) {
            Ok(_) => Build::Building(Mutex::new(receiver)),
            Err(error) => Build::Failed(format!("could not build collision wireframe: {error}")),
        };
    }
    fn take_mesh(&mut self) -> Option<Mesh> {
        let Build::Building(receiver) = &mut self.build else {
            return None;
        };
        match receiver
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .try_recv()
        {
            Ok(Ok(mesh)) => {
                self.build = Build::Ready;
                Some(mesh)
            }
            Ok(Err(error)) => {
                self.build = Build::Failed(error);
                None
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.build =
                    Build::Failed("collision wireframe worker stopped unexpectedly".into());
                None
            }
        }
    }
}
/// The entity holding the fixed-geometry line mesh; `collision_view` spawns one
/// and match teardown despawns it.
#[derive(Component)]
pub struct CollisionWireframe;

/// Unique triangle edges as a Bevy line-list mesh in Bevy coordinates.
fn wireframe_mesh(terrain: &CollisionMesh) -> Mesh {
    let positions: Vec<[f32; 3]> = terrain
        .vertices_m
        .iter()
        .map(|&p| flu_position(p).to_array())
        .collect();
    let mut edges = std::collections::HashSet::with_capacity(terrain.triangles.len() * 2);
    for [a, b, c] in &terrain.triangles {
        for (p, q) in [(a, b), (b, c), (c, a)] {
            edges.insert((*p.min(q), *p.max(q)));
        }
    }
    let mut indices = Vec::with_capacity(edges.len() * 2);
    for (p, q) in edges {
        indices.push(p);
        indices.push(q);
    }
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_indices(Indices::U32(indices))
}

/// Keep one background build running, even if the panel hides it again.
#[allow(clippy::too_many_arguments)]
pub fn collision_view(
    session: Res<Session>,
    mut debug: ResMut<CollisionDebug>,
    mut scenery: ResMut<SceneryVisibility>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut wireframes: Query<&mut Visibility, With<CollisionWireframe>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    ui: Res<crate::hud::HudState>,
) {
    if !ui.blocks_input()
        && ui.controls.just_pressed(
            crate::bindings::InputAction::Colliders,
            &keys,
            buttons.as_deref(),
        )
    {
        debug.view = match debug.view {
            CollisionView::Hidden => CollisionView::Overlay,
            CollisionView::Overlay => CollisionView::Alone,
            CollisionView::Alone => CollisionView::Hidden,
        };
    }
    debug.request(|| session.collision_geometry());
    let show_wireframe = debug.view != CollisionView::Hidden;
    if let Some(mesh) = debug.take_mesh() {
        commands.spawn((
            CollisionWireframe,
            bevy::pbr::wireframe::NoWireframe,
            // Debug lines must not enter the shadow passes.
            NotShadowCaster,
            NotShadowReceiver,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 1.0, 0.4),
                unlit: true,
                ..default()
            })),
            Transform::default(),
            if show_wireframe {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        ));
    }
    scenery.set_if_neq(SceneryVisibility(debug.scenery_visible()));
    for mut visibility in &mut wireframes {
        visibility.set_if_neq(if show_wireframe {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// One reusable wireframe entity for a chassis, armor, rune or projectile
/// shape; its index is the slot in the frame's wanted list.
#[derive(Component)]
pub(crate) struct DynamicCollisionWireframe(usize);

struct MovingMesh {
    kind: rm_simulator_world::referee::Mechanism,
    team: rm_simulator_world::Team,
    translations_m: [[f64; 3]; 2],
    mesh: Mesh,
}

/// System-local cache for `dynamic_colliders`: mesh handles, the shared material
/// and at most one in-flight background build of the moving CAD parts.
///
/// The build is polled with a nonblocking receive, so a slow worker never stalls
/// a frame.
#[derive(Default)]
pub(crate) struct DynamicWireframe {
    build: Option<Mutex<mpsc::Receiver<Vec<MovingMesh>>>>,
    moving: Vec<(MovingMeshPose, Handle<Mesh>)>,
    cube: Option<Handle<Mesh>>,
    sphere: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    entities: usize,
}
struct MovingMeshPose {
    kind: rm_simulator_world::referee::Mechanism,
    team: rm_simulator_world::Team,
    translations_m: [[f64; 3]; 2],
}
impl MovingMeshPose {
    fn transform(&self, scene: &rm_simulator_render::sync::SceneState) -> Transform {
        use rm_simulator_world::referee::Mechanism;
        let fraction = match self.kind {
            Mechanism::Base => f64::from(scene.base_open_fraction[self.team.index()]),
            Mechanism::DartDoor => f64::from(scene.dart_door_open[self.team.index()]),
            Mechanism::DartTarget => f64::from(scene.dart_target_fraction),
        };
        Transform::from_translation(flu_position(std::array::from_fn(|i| {
            self.translations_m[0][i]
                + (self.translations_m[1][i] - self.translations_m[0][i]) * fraction
        })))
    }
}

fn primitive_wireframe(mesh: Mesh) -> Mesh {
    let mut edges = std::collections::HashSet::new();
    let indices: Vec<_> = mesh
        .indices()
        .expect("indexed debug primitive")
        .iter()
        .collect();
    for triangle in indices.as_chunks::<3>().0 {
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            edges.insert((a.min(b) as u32, a.max(b) as u32));
        }
    }
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().clone(),
        )
        .with_inserted_indices(Indices::U32(
            edges.into_iter().flat_map(|(a, b)| [a, b]).collect(),
        ))
}

fn box_transform(mut pose: Transform, half_m: [f32; 3], housing: bool) -> Transform {
    if housing {
        pose.translation = pose.transform_point(Vec3::Z * half_m[0]);
    }
    pose.scale = Vec3::new(half_m[1], half_m[2], half_m[0]) * 2.;
    pose
}
fn pose_transform(pose: rm_simulator_render::PoseFlu) -> Transform {
    let mut result = Transform::default();
    rm_simulator_render::apply_pose(&mut result, pose);
    result
}

/// Cheap primitive transforms come from the exact SceneInput consumed by visuals.
/// CAD triangles are expanded once in the background, then only transforms change.
#[allow(clippy::too_many_arguments)]
pub fn dynamic_colliders(
    session: Res<Session>,
    input: Res<rm_simulator_render::sync::SceneInput>,
    debug: Res<CollisionDebug>,
    mut cache: Local<DynamicWireframe>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut instances: Query<(
        &DynamicCollisionWireframe,
        &mut Mesh3d,
        &mut Transform,
        &mut Visibility,
    )>,
) {
    let show = debug.view != CollisionView::Hidden;
    let Some(scene) = &input.0 else { return };
    if !show {
        for (_, _, _, mut visibility) in &mut instances {
            visibility.set_if_neq(Visibility::Hidden);
        }
        return;
    }
    if cache.cube.is_none() {
        cache.cube = Some(meshes.add(primitive_wireframe(Mesh::from(Cuboid::new(1., 1., 1.)))));
        cache.sphere =
            Some(meshes.add(primitive_wireframe(Sphere::new(1.).mesh().ico(2).unwrap())));
        cache.material = Some(materials.add(StandardMaterial {
            base_color: Color::srgb(1., 0.65, 0.15),
            unlit: true,
            ..default()
        }));
        let geometry = session.client_collision_geometry().unwrap_or_default();
        let (sender, receiver) = mpsc::sync_channel(1);
        cache.build = Some(Mutex::new(receiver));
        if let Err(error) = std::thread::Builder::new()
            .name("moving-collider-meshes".into())
            .spawn(move || {
                let parts = geometry
                    .moving_parts()
                    .into_iter()
                    .map(|part| {
                        let (vertices_m, triangles) = part.geometry.into_triangles();
                        MovingMesh {
                            kind: part.kind,
                            team: part.team,
                            translations_m: part.translations_m,
                            mesh: wireframe_mesh(&CollisionMesh {
                                vertices_m,
                                triangles,
                            }),
                        }
                    })
                    .collect();
                let _ = sender.send(parts);
            })
        {
            warn!("cannot build moving collider meshes: {error}");
        }
    }
    let built = cache
        .build
        .as_ref()
        .and_then(|build| build.try_lock().ok()?.try_recv().ok());
    if let Some(parts) = built {
        cache.moving = parts
            .into_iter()
            .map(|part| {
                (
                    MovingMeshPose {
                        kind: part.kind,
                        team: part.team,
                        translations_m: part.translations_m,
                    },
                    meshes.add(part.mesh),
                )
            })
            .collect();
        cache.build = None;
    }
    let cube = cache.cube.as_ref().unwrap();
    let sphere = cache.sphere.as_ref().unwrap();
    let mut wanted: Vec<(Handle<Mesh>, Transform)> = cache
        .moving
        .iter()
        .map(|(part, mesh)| (mesh.clone(), part.transform(scene)))
        .collect();
    for transform in primitive_transforms(scene, &session.snapshot) {
        wanted.push((if transform.0 { sphere } else { cube }.clone(), transform.1));
    }
    for (index, mut mesh, mut transform, mut visibility) in &mut instances {
        if let Some((wanted_mesh, wanted_pose)) = wanted.get(index.0) {
            mesh.set_if_neq(Mesh3d(wanted_mesh.clone()));
            transform.set_if_neq(*wanted_pose);
            visibility.set_if_neq(Visibility::Inherited);
        } else {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
    for (index, (mesh, transform)) in wanted.iter().enumerate().skip(cache.entities) {
        commands.spawn((
            DynamicCollisionWireframe(index),
            bevy::pbr::wireframe::NoWireframe,
            NotShadowCaster,
            NotShadowReceiver,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(cache.material.as_ref().unwrap().clone()),
            *transform,
            Visibility::Inherited,
        ));
    }
    cache.entities = cache.entities.max(wanted.len());
}

fn primitive_transforms(
    scene: &rm_simulator_render::sync::SceneState,
    snapshot: &rm_simulator_world::FieldSnapshot,
) -> Vec<(bool, Transform)> {
    use rm_simulator_world::projectile::{RUNE_HOUSING_HALF_M, SMALL_ARMOR_HOUSING_HALF_M};
    let small = SMALL_ARMOR_HOUSING_HALF_M.map(|x| x as f32);
    let mut result = Vec::new();
    for chassis in &scene.chassis {
        let body = pose_transform(chassis.pose);
        result.push((false, box_transform(body, chassis.body_half_m, false)));
        result.push((
            false,
            box_transform(pose_transform(chassis.turret), chassis.turret_half_m, false),
        ));
        for armor in &chassis.armor {
            result.push((
                false,
                box_transform(body.mul_transform(pose_transform(armor.local)), small, true),
            ));
        }
    }
    for outpost in &scene.outposts {
        for pose in outpost.armors {
            result.push((false, box_transform(pose_transform(pose), small, true)));
        }
    }
    for (rune, state) in scene.runes.iter().zip(&snapshot.runes) {
        for mut face in rm_simulator_world::rune::target_poses(
            crate::frames::to_pose(rune.hub_pose),
            state.target_radius_m,
            rune.angle_rad,
        ) {
            // Match the physics scoring face's outward normal on rune housings.
            let [w, x, y, z] = face.rotation_wxyz;
            face.rotation_wxyz = [-z, y, -x, w];
            result.push((
                false,
                box_transform(
                    crate::frames::transform_of(&face),
                    RUNE_HOUSING_HALF_M.map(|x| x as f32),
                    true,
                ),
            ));
        }
    }
    for ball in &scene.projectiles {
        result.push((
            true,
            Transform::from_translation(flu_position(ball.position_m))
                .with_scale(Vec3::splat(ball.radius_m)),
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collider_toggle_preserves_client_presentation() {
        use crate::controls::{Drive, Player};
        use rm_simulator_render::sync::SceneInput;
        let session = crate::session::test_session(true);
        let id = session.chassis_id.unwrap();
        let mut app = App::new();
        app.insert_resource(session)
            .insert_resource(CollisionDebug::new(CollisionView::Hidden))
            .insert_resource(Player::at(Vec3::ZERO, 1.2, 0.4))
            .insert_resource(Drive::new(id, true))
            .init_resource::<SceneInput>()
            .add_systems(Update, crate::scene::publish_scene);
        app.update();
        let before = app.world().resource::<SceneInput>().0.clone();
        app.world_mut().resource_mut::<CollisionDebug>().view = CollisionView::Overlay;
        app.update();
        assert_eq!(app.world().resource::<SceneInput>().0, before);
    }

    #[test]
    fn moving_colliders_use_this_frames_visual_poses_even_with_an_old_snapshot() {
        use crate::controls::{Drive, Player};
        use rm_simulator_render::sync::SceneInput;
        let session = crate::session::test_session(true);
        let id = session.chassis_id.unwrap();
        let mut app = App::new();
        app.insert_resource(session)
            .insert_resource(CollisionDebug::new(CollisionView::Overlay))
            .insert_resource(Player::at(Vec3::ZERO, 1.2, 0.4))
            .insert_resource(Drive::new(id, true))
            .init_resource::<SceneInput>()
            .add_systems(Update, crate::scene::publish_scene);
        app.update();
        let snapshot = &app.world().resource::<Session>().snapshot;
        let mut scene = app.world().resource::<SceneInput>().0.clone().unwrap();
        for frame in 0..8 {
            let chassis = &mut scene.chassis[0];
            chassis.pose.translation_m[0] += 0.04;
            chassis.turret.translation_m[1] += 0.03;
            chassis.turret.rotation_wxyz = [
                (frame as f64 * 0.1).cos(),
                0.,
                0.,
                (frame as f64 * 0.1).sin(),
            ];
            let shapes = primitive_transforms(&scene, snapshot);
            assert_eq!(
                shapes[0].1.translation,
                flu_position(scene.chassis[0].pose.translation_m)
            );
            assert_eq!(
                shapes[1].1.translation,
                flu_position(scene.chassis[0].turret.translation_m)
            );
            assert_eq!(
                shapes[1].1.rotation,
                pose_transform(scene.chassis[0].turret).rotation
            );
            assert_ne!(
                scene.chassis[0].pose.translation_m,
                snapshot.chassis[0].pose.translation_m
            );
        }
    }

    #[test]
    fn hidden_view_never_requests_geometry_and_unavailable_keeps_scenery() {
        let mut debug = CollisionDebug::new(CollisionView::Hidden);
        debug.request(|| panic!("hidden debug view must not capture physics"));
        assert!(!debug.pending());
        debug.view = CollisionView::Alone;
        debug.request(|| None);
        assert!(!debug.pending());
        assert!(debug.failure().is_some());
        assert!(debug.scenery_visible());
        debug.request(|| panic!("unavailable geometry must not be requested every frame"));
    }

    #[test]
    fn toggling_during_a_build_reuses_one_result_and_keeps_scenery_until_ready() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut debug = CollisionDebug::new(CollisionView::Alone);
        debug.build = Build::Building(Mutex::new(receiver));
        assert!(debug.pending());
        assert!(debug.scenery_visible());
        assert!(debug.take_mesh().is_none());
        debug.view = CollisionView::Hidden;
        debug.request(|| panic!("build already in flight"));
        assert!(!debug.pending());
        sender
            .send(Ok(wireframe_mesh(&CollisionMesh::default())))
            .unwrap();
        assert!(debug.take_mesh().is_some());
        assert!(debug.scenery_visible());
        debug.view = CollisionView::Alone;
        debug.request(|| panic!("reuse the completed wireframe"));
        assert!(!debug.scenery_visible());
        assert!(!debug.pending());
        assert!(debug.take_mesh().is_none());
    }

    #[test]
    fn failed_worker_does_not_leave_a_blank_or_permanently_pending_view() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut debug = CollisionDebug::new(CollisionView::Alone);
        debug.build = Build::Building(Mutex::new(receiver));
        drop(sender);
        assert!(debug.take_mesh().is_none());
        assert!(!debug.pending());
        assert!(debug.failure().is_some());
        assert!(debug.scenery_visible());
    }

    #[test]
    fn capture_failure_reports_the_host_error() {
        let mut debug = CollisionDebug::new(CollisionView::Overlay);
        debug.request(|| Some(Box::new(|| Err("host is stopped".into()))));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while debug.take_mesh().is_none() && debug.pending() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(debug.failure(), Some("host is stopped"));
    }

    #[test]
    fn worker_builds_from_owned_physics_shapes() {
        let mut field = rm_simulator_world::Field::new(&rm_simulator_world::FieldConfig {
            runes: Vec::new(),
            outposts: Vec::new(),
            ..Default::default()
        })
        .unwrap();
        field
            .add_static_mesh(
                vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                vec![[0, 1, 2]],
            )
            .unwrap();
        let geometry = field.static_geometry_snapshot();
        drop(field);
        let mut debug = CollisionDebug::new(CollisionView::Overlay);
        let caller = std::thread::current().id();
        debug.request(|| {
            Some(Box::new(move || {
                assert_ne!(
                    std::thread::current().id(),
                    caller,
                    "capture must run off the frame thread"
                );
                Ok(geometry)
            }))
        });
        let Build::Building(receiver) = &mut debug.build else {
            panic!("worker did not start");
        };
        let mesh = receiver
            .get_mut()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(mesh.indices().unwrap().len(), 6);
    }

    #[test]
    fn wireframe_mesh_shares_edges_between_neighbouring_triangles() {
        let terrain = CollisionMesh {
            vertices_m: vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        };
        let mesh = wireframe_mesh(&terrain);
        assert_eq!(mesh.primitive_topology(), PrimitiveTopology::LineList);
        // Five unique edges: four sides and one shared diagonal.
        assert_eq!(mesh.indices().map(|i| i.len()), Some(10));
        assert_eq!(mesh.count_vertices(), 4);
    }
}
