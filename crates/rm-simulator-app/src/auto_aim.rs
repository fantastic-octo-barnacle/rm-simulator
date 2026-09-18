// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Ground-truth game aim assist. The stages follow the broad organization of
//! Vision2027's aimer: select an object, predict a scoring face at impact, solve
//! gravity/drag, then gate firing on the actual motor pose. No vision code or
//! dependencies are copied. Perfect snapshot velocities replace an estimator.
use crate::{
    bindings::InputAction,
    controls::{Gun, MUZZLE_FORWARD_M, Player},
    frames::{dquat, wxyz},
    hud::HudState,
    session::Session,
};
use bevy::{
    math::{DQuat, DVec2, DVec3},
    prelude::*,
};
use rm_simulator_world::{
    Caliber, ChassisSnapshot, OutpostSnapshot, Pose, RuneSnapshot, Shot, StaticGeometry,
};

// Assist tuning, not competition rules. Bound all prediction and acquisition work.
const MAX_RANGE_M: f64 = 40.;
const MAX_FLIGHT_S: f64 = 2.5;
const AIM_CONE_RAD: f64 = 0.10;
/// The gimbal pitch range the player's manual aim is clamped to. Solving inside
/// the same range keeps an assist solution reachable by hand.
const PITCH_LIMIT: [f64; 2] = [
    crate::controls::DRIVE_PITCH_RAD.0 as f64,
    crate::controls::DRIVE_PITCH_RAD.1 as f64,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetId {
    Base(usize),
    Robot(u32),
    Outpost(usize),
    Rune(usize),
}
/// Auto-aim assist state: the retained target, this frame's firing decision and
/// the HUD line that describes both.
#[derive(Resource, Default)]
pub struct AutoAim {
    target: Option<TargetId>,
    /// Set by [`update`] on frames the gun may release a round; cleared at the
    /// start of every frame and while no target solves. `fire_gun` fires on it,
    /// so it is a permission, not a record that a round left.
    pub fire_ready: bool,
    /// HUD text for the Auto Aim row: the chosen target and whether the shot is
    /// firing, waiting on alignment or aim-only. Empty while the assist is off.
    pub status: String,
    /// Age of the authoritative target state at presentation, in ms.
    /// Execution lead and downstream transit are excluded, so the staleness
    /// gates judge freshness rather than the round trip.
    pub observation_age_ms: f64,
    /// Intended execution time used by the solver, in simulation nanoseconds.
    pub execution_time_ns: u64,
    /// Short gate key for this frame (`stale`, `tracking-only`, `firing`,
    /// `searching`, `no-shot`, `aim only`, or the blocking reason such as
    /// `weapon cadence`). Backs the network-HUD breakdown so
    /// tracking-but-not-firing is visible without reading the full status
    /// line.
    pub last_gate: String,
    /// Cumulative frames the observation was stale (>300 ms, no target).
    pub stale_frames: u64,
    /// Cumulative frames tracking with a stale (>150 ms) observation
    /// (aim, no fire).
    pub tracking_only_frames: u64,
    /// Cumulative frames blocked by cadence, rune, geometry, robots or gate.
    pub blocked_frames: u64,
    /// Cumulative frames with `fire_ready` set.
    pub firing_frames: u64,
    proposed_rune: Option<RuneShot>,
    last_rune: Option<RuneShot>,
}
// Lower rune cadence prevents a trailing burst from hitting an already-cleared
// blade. These delays are assist settings, not rulebook limits.
const RUNE_SHOT_INTERVAL_NS: u64 = 500_000_000;
const RUNE_CONFIRM_MARGIN_NS: u64 = 150_000_000;
const RUNE_MISS_RETRY_NS: u64 = 1_200_000_000;
#[derive(Clone, Copy)]
struct RuneShot {
    rune: usize,
    blade: usize,
    epoch_ns: u64,
    group: u32,
    fired_ns: u64,
    flight_ns: u64,
}
impl AutoAim {
    fn clear_intent(&mut self) {
        self.target = None;
        self.fire_ready = false;
        self.status.clear();
        self.last_gate.clear();
        self.proposed_rune = None;
    }
    /// Record this frame's gate outcome for the HUD breakdown. Cumulative
    /// counters survive `clear_intent`; only the per-frame status is cleared.
    fn record_gate(&mut self, gate: &str) {
        self.last_gate = gate.into();
        match gate {
            "stale" => self.stale_frames += 1,
            "tracking-only" => self.tracking_only_frames += 1,
            "firing" => self.firing_frames += 1,
            "off" | "searching" | "no-shot" | "aim only" => {}
            _ => self.blocked_frames += 1,
        }
    }
    /// Called only when controls actually submit a shot, not while waiting for
    /// the weapon cadence. Releasing/repressing the button cannot bypass it.
    pub fn shot_requested(&mut self, now_ns: u64) {
        if let Some(mut shot) = self.proposed_rune {
            shot.fired_ns = now_ns;
            self.last_rune = Some(shot);
        }
    }
}
fn rune_shot(target: &Target<'_>, solution: Solution) -> Option<RuneShot> {
    let (TargetId::Rune(index), Motion::Rune(rune, _)) = (target.id, target.motion) else {
        return None;
    };
    Some(RuneShot {
        rune: index,
        blade: solution.face,
        epoch_ns: rune.state_since_ns,
        group: rune.completed_groups,
        fired_ns: 0,
        flight_ns: (solution.flight_s * 1e9) as u64,
    })
}
/// Whether one more `caliber` launch keeps the barrel at or below its heat
/// limit (section 5.1.3 overheats above it). Auto-fire never overheats;
/// manual fire is not gated. `heat` is the predicted heat in tenths and the
/// limit, and no referee record allows the shot.
fn heat_allows(heat: Option<(u64, u32)>, caliber: rm_simulator_world::Caliber) -> bool {
    heat.is_none_or(|(tenths, limit)| {
        tenths + rm_simulator_world::referee::game_caliber(caliber).launch_heat_tenths()
            <= u64::from(limit) * 10
    })
}

fn rune_fire_allowed(previous: Option<RuneShot>, target: &Target<'_>, now_ns: u64) -> bool {
    let Motion::Rune(rune, _) = target.motion else {
        return true;
    };
    let Some(previous) = previous else {
        return true;
    };
    let elapsed = now_ns.saturating_sub(previous.fired_ns);
    if elapsed < RUNE_SHOT_INTERVAL_NS.max(previous.flight_ns + RUNE_CONFIRM_MARGIN_NS) {
        return false;
    }
    let still_waiting = target.id == TargetId::Rune(previous.rune)
        && rune.state_since_ns == previous.epoch_ns
        && rune.completed_groups == previous.group
        && rune
            .active_blades
            .get(previous.blade)
            .copied()
            .unwrap_or(false);
    !still_waiting || elapsed >= RUNE_MISS_RETRY_NS.max(previous.flight_ns + 350_000_000)
}

#[derive(Clone, Copy)]
enum Motion<'a> {
    Base(&'a rm_simulator_world::BaseSnapshot, u64),
    Robot(&'a ChassisSnapshot, f64),
    Outpost(&'a OutpostSnapshot, f64),
    Rune(&'a RuneSnapshot, u64),
}
struct Target<'a> {
    id: TargetId,
    motion: Motion<'a>,
    faces: Vec<usize>,
}
impl Target<'_> {
    fn pose(&self, face: usize, future_s: f64) -> Pose {
        match self.motion {
            Motion::Base(base, now) => {
                base.pose(face, now.saturating_add((future_s.max(0.) * 1e9) as u64))
            }
            Motion::Robot(robot, age) => {
                let dt = age + future_s;
                let omega = DVec3::from_array(robot.angular_velocity_rad_s);
                let rotation = (DQuat::from_scaled_axis(omega * dt)
                    * dquat(robot.pose.rotation_wxyz))
                .normalize();
                let local = robot.config.armor_faces()[face];
                let center = DVec3::from_array(robot.pose.translation_m)
                    + DVec3::from_array(robot.velocity_m_s) * dt;
                Pose {
                    translation_m: (center + rotation * DVec3::from_array(local.translation_m))
                        .to_array(),
                    rotation_wxyz: wxyz(rotation * dquat(local.rotation_wxyz)),
                }
            }
            Motion::Outpost(outpost, age) => {
                let mut predicted = outpost.clone();
                predicted.angle_rad += outpost.speed_rad_s * (age + future_s);
                predicted.rebuild_armors();
                predicted.armors[face].pose
            }
            Motion::Rune(rune, now_ns) => {
                let then = now_ns.saturating_add((future_s.max(0.) * 1e9) as u64);
                rm_simulator_world::rune::scoring_pose(
                    rm_simulator_world::rune::target_poses(
                        rune.hub_pose,
                        rune.target_radius_m,
                        rune.presentation_angle_rad(then),
                    )[face],
                )
            }
        }
    }
    fn half_size(&self) -> DVec2 {
        match self.motion {
            Motion::Rune(..) => {
                DVec2::splat(rm_simulator_world::rune::EFFECTIVE_TARGET_RADIUS_M * 0.65)
            }
            _ => {
                DVec2::new(
                    rm_simulator_world::outpost::DETECTION_WIDTH_M,
                    rm_simulator_world::outpost::DETECTION_HEIGHT_M,
                ) * 0.5
            }
        }
    }
    fn description(&self) -> String {
        match self.id {
            TargetId::Base(id) => format!("BASE {}", id + 1),
            TargetId::Robot(id) => format!("ROBOT #{id}"),
            TargetId::Outpost(id) => format!("OUTPOST {}", id + 1),
            TargetId::Rune(id) => format!("RUNE {}", id + 1),
        }
    }
}

fn view_targets<'a>(targets: &[Target<'a>], displayed: &'a [ChassisSnapshot]) -> Vec<Target<'a>> {
    targets
        .iter()
        .map(|target| Target {
            id: target.id,
            motion: match target.motion {
                Motion::Robot(robot, _) => displayed
                    .iter()
                    .find(|view| view.id == robot.id)
                    .map_or(target.motion, |view| Motion::Robot(view, 0.)),
                motion => motion,
            },
            faces: target.faces.clone(),
        })
        .collect()
}

fn targets(session: &Session, caliber: Caliber) -> Vec<Target<'_>> {
    let now = session.fire_time_ns();
    let age = now.saturating_sub(session.snapshot.time_ns) as f64 * 1e-9;
    // Do not coast old network observations indefinitely through a blackout.
    if age > 0.3 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for robot in &session.snapshot.chassis {
        if Some(robot.id) == session.chassis_id || robot.team == session.team || robot.defeated {
            continue;
        }
        if session
            .referee()
            .is_some_and(|r| r.robots.iter().any(|r| r.id == robot.id && !r.alive()))
        {
            continue;
        }
        result.push(Target {
            id: TargetId::Robot(robot.id),
            motion: Motion::Robot(robot, age),
            faces: (0..4).collect(),
        });
    }
    for (index, outpost) in session.snapshot.outposts.iter().enumerate() {
        let owner = session
            .referee()
            .and_then(|r| r.outpost_teams.get(index))
            .copied()
            .unwrap_or_else(|| rm_simulator_server::layout::outpost_team(&outpost.origin));
        if outpost.destroyed || owner == session.team {
            continue;
        }
        result.push(Target {
            id: TargetId::Outpost(index),
            motion: Motion::Outpost(outpost, now.saturating_sub(outpost.time_ns) as f64 * 1e-9),
            faces: (0..outpost.armors.len()).collect(),
        });
    }
    for (index, base) in session.snapshot.bases.iter().enumerate() {
        if base.hp == 0 || base.config.team == session.team {
            continue;
        }
        if session.referee().is_some_and(|r| {
            r.phase != rm_simulator_world::MatchPhase::Idle
                && r.outpost_teams
                    .iter()
                    .zip(&session.snapshot.outposts)
                    .any(|(owner, o)| *owner == base.config.team && !o.destroyed)
        }) {
            continue;
        }
        result.push(Target {
            id: TargetId::Base(index),
            motion: Motion::Base(base, now),
            faces: (0..7).collect(),
        });
    }
    if caliber == Caliber::Mm17 {
        for (index, rune) in session.snapshot.runes.iter().enumerate() {
            let owner = session
                .referee()
                .and_then(|r| r.rune_teams.get(index))
                .copied()
                .unwrap_or_else(|| rm_simulator_server::layout::rune_team(&rune.hub_pose));
            if rune.state != rm_simulator_world::rune::RuneState::Activating
                || owner != session.team
            {
                continue;
            }
            let faces: Vec<_> = rune
                .active_blades
                .iter()
                .enumerate()
                .filter_map(|(i, active)| (*active && !rune.activated[i]).then_some(i))
                .collect();
            if !faces.is_empty() {
                result.push(Target {
                    id: TargetId::Rune(index),
                    motion: Motion::Rune(rune, now),
                    faces,
                });
            }
        }
    }
    result
}

/// Finite slab test; parallel rays and boxes behind the eye are handled explicitly.
fn ray_aabb(origin: DVec3, direction: DVec3, low: DVec3, high: DVec3) -> bool {
    ray_aabb_range(origin, direction, low, high, MAX_RANGE_M)
}
fn ray_aabb_range(origin: DVec3, direction: DVec3, low: DVec3, high: DVec3, range_m: f64) -> bool {
    let mut near: f64 = 0.;
    let mut far: f64 = range_m;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-10 {
            if origin[axis] < low[axis] || origin[axis] > high[axis] {
                return false;
            }
        } else {
            let a = (low[axis] - origin[axis]) / direction[axis];
            let b = (high[axis] - origin[axis]) / direction[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return false;
            }
        }
    }
    true
}
fn visible(geometry: Option<&StaticGeometry>, from: DVec3, to: DVec3) -> bool {
    let delta = to - from;
    // Stop just before the scoring surface rather than treating its housing as a wall.
    geometry.is_none_or(|g| {
        g.segment_clear(
            from.to_array(),
            (to - delta.normalize_or_zero() * 0.025).to_array(),
        )
    })
}
fn acquisition(
    target: &Target<'_>,
    eye: DVec3,
    direction: DVec3,
    retained: bool,
    geometry: Option<&StaticGeometry>,
) -> Option<f64> {
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for &face in &target.faces {
        let point = DVec3::from_array(target.pose(face, 0.).translation_m);
        low = low.min(point);
        high = high.max(point);
    }
    let center = (low + high) * 0.5;
    let delta = center - eye;
    let distance = delta.length();
    if !(0.3..=MAX_RANGE_M).contains(&distance) || delta.dot(direction) <= 0. {
        return None;
    }
    let margin = 0.12 + distance * AIM_CONE_RAD.tan() * if retained { 1.5 } else { 1. };
    if !ray_aabb(
        eye,
        direction,
        low - DVec3::splat(margin),
        high + DVec3::splat(margin),
    ) {
        return None;
    }
    if !target.faces.iter().any(|&face| {
        visible(
            geometry,
            eye,
            DVec3::from_array(target.pose(face, 0.).translation_m),
        )
    }) {
        return None;
    }
    let angular = 1. - delta.normalize().dot(direction);
    Some(angular + distance * 0.00001 - if retained { 0.01 } else { 0. })
}

/// Crosshair acquisition takes priority; otherwise find the nearest visible enemy
/// robot in range, even outside the view cone. `targets` filters team and health.
fn select_target<'a, 'world>(
    targets: &'a [Target<'world>],
    eye: DVec3,
    direction: DVec3,
    retained: Option<TargetId>,
    geometry: Option<&StaticGeometry>,
) -> Option<&'a Target<'world>> {
    targets
        .iter()
        .filter_map(|target| {
            acquisition(
                target,
                eye,
                direction,
                retained == Some(target.id),
                geometry,
            )
            .map(|score| (score, target))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, target)| target)
        .or_else(|| {
            targets
                .iter()
                .filter_map(|target| {
                    let Motion::Robot(robot, age_s) = target.motion else {
                        return None;
                    };
                    let center = DVec3::from_array(robot.pose.translation_m)
                        + DVec3::from_array(robot.velocity_m_s) * age_s;
                    let delta = center - eye;
                    let distance = delta.length();
                    if !(0.3..=MAX_RANGE_M).contains(&distance) {
                        return None;
                    }
                    // Reuse the same face visibility and range checks without the view cone.
                    acquisition(target, eye, delta.normalize(), false, geometry)?;
                    Some((distance, target))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, target)| target)
        })
}

/// Integrate the same quadratic sphere drag and gravity as the game. Midpoint
/// integration at <= 5 ms is cheap enough for the bounded aiming iterations.
fn flight(shot: Shot, pitch: f64, time_s: f64) -> (DVec2, DVec2) {
    let direction = DVec2::new(pitch.cos(), pitch.sin());
    let mut position = direction * f64::from(MUZZLE_FORWARD_M);
    let mut velocity = direction * shot.speed_m_s;
    let k = shot.caliber.drag_factor_per_m();
    let acceleration =
        |v: DVec2| -v * (k * v.length()) - DVec2::Y * rm_simulator_world::projectile::GRAVITY_M_S2;
    let steps = (time_s / 0.005).ceil().max(1.) as usize;
    let dt = time_s / steps as f64;
    for _ in 0..steps {
        let midpoint = velocity + acceleration(velocity) * (dt * 0.5);
        position += midpoint * dt;
        velocity += acceleration(midpoint) * dt;
    }
    (position, velocity)
}
#[derive(Clone, Copy, Debug)]
struct Solution {
    yaw_rad: f64,
    pitch_rad: f64,
    flight_s: f64,
    face: usize,
}
/// Solve radial distance and height simultaneously with a bounded Newton step.
/// The target model is evaluated at each candidate impact time, including spin.
fn solve(
    target: &Target<'_>,
    face: usize,
    pivot: DVec3,
    shot: Shot,
    lead_s: f64,
) -> Option<Solution> {
    let initial = DVec3::from_array(target.pose(face, lead_s).translation_m) - pivot;
    let mut time = ((initial.length() - f64::from(MUZZLE_FORWARD_M)) / shot.speed_m_s)
        .clamp(0.01, MAX_FLIGHT_S);
    let mut pitch = (initial.z.atan2(initial.truncate().length())
        + 0.5 * rm_simulator_world::projectile::GRAVITY_M_S2 * time / shot.speed_m_s)
        .clamp(PITCH_LIMIT[0], PITCH_LIMIT[1]);
    let error = |pitch, time| {
        let point = DVec3::from_array(target.pose(face, lead_s + time).translation_m) - pivot;
        flight(shot, pitch, time).0 - DVec2::new(point.truncate().length(), point.z)
    };
    for _ in 0..12 {
        let residual = error(pitch, time);
        if !residual.is_finite() {
            return None;
        }
        if residual.length() < 0.002 {
            let point = DVec3::from_array(target.pose(face, lead_s + time).translation_m) - pivot;
            return Some(Solution {
                yaw_rad: point.y.atan2(point.x),
                pitch_rad: pitch,
                flight_s: time,
                face,
            });
        }
        let dp = (error(pitch + 0.0001, time) - residual) / 0.0001;
        let dt = (error(pitch, time + 0.0001) - residual) / 0.0001;
        let determinant = dp.x * dt.y - dt.x * dp.y;
        if determinant.abs() < 1e-8 {
            return None;
        }
        let pitch_step = (-residual.x * dt.y + dt.x * residual.y) / determinant;
        let time_step = (-dp.x * residual.y + residual.x * dp.y) / determinant;
        pitch = (pitch + pitch_step.clamp(-0.2, 0.2)).clamp(PITCH_LIMIT[0], PITCH_LIMIT[1]);
        time = (time + time_step.clamp(-0.25, 0.25)).clamp(0.005, MAX_FLIGHT_S);
    }
    None
}
fn facing(pose: Pose, from: DVec3) -> f64 {
    (dquat(pose.rotation_wxyz) * DVec3::X)
        .dot((from - DVec3::from_array(pose.translation_m)).normalize_or_zero())
}
fn choose_solution(
    target: &Target<'_>,
    pivot: DVec3,
    shot: Shot,
    lead_s: f64,
    geometry: Option<&StaticGeometry>,
) -> Option<Solution> {
    target
        .faces
        .iter()
        .filter_map(|&face| {
            let solution = solve(target, face, pivot, shot, lead_s)?;
            let pose = target.pose(face, lead_s + solution.flight_s);
            let cosine = facing(pose, pivot);
            if cosine < 0.25 || !visible(geometry, pivot, DVec3::from_array(pose.translation_m)) {
                return None;
            }
            Some((cosine, solution))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, solution)| solution)
}

/// Gate against the actual barrel, not the commanded angle. This deliberately
/// aims for an inset area and requires normal impact speed above the game's
/// detection threshold. No automatic trigger when simply slewing toward a lock.
#[cfg(test)]
fn fire_gate(
    target: &Target<'_>,
    solution: Solution,
    pivot: DVec3,
    actual: DQuat,
    shot: Shot,
    geometry: Option<&StaticGeometry>,
) -> bool {
    fire_gate_reason(target, solution, pivot, actual, shot, geometry).is_ok()
}

fn fire_gate_reason(
    target: &Target<'_>,
    solution: Solution,
    pivot: DVec3,
    actual: DQuat,
    shot: Shot,
    geometry: Option<&StaticGeometry>,
) -> Result<(), &'static str> {
    let direction = actual * DVec3::X;
    let pitch = direction.z.clamp(-1., 1.).asin();
    let yaw = direction.y.atan2(direction.x);
    let horizontal = DVec3::new(yaw.cos(), yaw.sin(), 0.);
    let mut time = solution.flight_s;
    // Refine the crossing of the moving plate's plane for the actual trajectory.
    for _ in 0..4 {
        let (point, velocity) = flight(shot, pitch, time);
        let bullet = pivot + horizontal * point.x + DVec3::Z * point.y;
        let pose = target.pose(solution.face, time);
        let normal = dquat(pose.rotation_wxyz) * DVec3::X;
        let plate = DVec3::from_array(pose.translation_m);
        let target_velocity =
            (DVec3::from_array(target.pose(solution.face, time + 0.001).translation_m) - plate)
                / 0.001;
        let relative = horizontal * velocity.x + DVec3::Z * velocity.y - target_velocity;
        let speed = relative.dot(normal);
        if -speed <= shot.caliber.armor_detection_speed_m_s() {
            return Err("plate facing or detection speed");
        }
        let error = (bullet - plate).dot(normal);
        if error.abs() < 0.003 {
            let local = dquat(pose.rotation_wxyz).inverse() * (bullet - plate);
            let half = target.half_size() * 0.7;
            if local.y.abs() > half.x || local.z.abs() > half.y {
                return Err("turning barrel");
            }
            // Check the curved shot path as well as the acquisition sight line.
            let mut previous = pivot + direction * f64::from(MUZZLE_FORWARD_M);
            for i in 1..=16 {
                let (p, _) = flight(shot, pitch, time * i as f64 / 16.);
                let point = pivot + horizontal * p.x + DVec3::Z * p.y;
                let end = if i == 16 {
                    point - (point - previous).normalize_or_zero() * 0.025
                } else {
                    point
                };
                if geometry.is_some_and(|g| !g.segment_clear(previous.to_array(), end.to_array())) {
                    return Err("blocked by scenery");
                }
                previous = point;
            }
            return Ok(());
        }
        time = (time - error / speed).clamp(0.005, MAX_FLIGHT_S);
    }
    Err("turning barrel")
}

/// Conservatively reject shots passing through another robot, including allies.
fn clear_of_robots(
    session: &Session,
    target: TargetId,
    pivot: DVec3,
    actual: DQuat,
    shot: Shot,
    flight_s: f64,
) -> bool {
    let direction = actual * DVec3::X;
    let pitch = direction.z.clamp(-1., 1.).asin();
    let horizontal = DVec3::new(direction.x, direction.y, 0.).normalize_or_zero();
    let age = session
        .fire_time_ns()
        .saturating_sub(session.snapshot.time_ns) as f64
        * 1e-9;
    let mut previous = pivot + direction * f64::from(MUZZLE_FORWARD_M);
    for i in 1..=16 {
        let time = flight_s * i as f64 / 16.;
        let (point, _) = flight(shot, pitch, time);
        let point = pivot + horizontal * point.x + DVec3::Z * point.y;
        for robot in &session.snapshot.chassis {
            if Some(robot.id) == session.chassis_id || target == TargetId::Robot(robot.id) {
                continue;
            }
            let center = DVec3::from_array(robot.pose.translation_m)
                + DVec3::from_array(robot.velocity_m_s) * (age + time);
            // A spherical bound covers the body, armor, and turret through any tilt.
            let radius = DVec3::from_array(robot.config.body_half_m).length().max(
                DVec3::from_array(robot.config.turret_center_m).length()
                    + DVec3::from_array(robot.config.turret_half_m).length(),
            ) + 0.08;
            let delta = point - previous;
            if ray_aabb_range(
                previous,
                delta.normalize_or_zero(),
                center - DVec3::splat(radius),
                center + DVec3::splat(radius),
                delta.length(),
            ) {
                return false;
            }
        }
        previous = point;
    }
    true
}

/// Bevy system: select a target from the latest snapshot and, while the pilot
/// holds the Auto Aim binding, steer `Player` at the gimbal angles that solve
/// for it. Runs before `drive_chassis` and `fire_gun` so one frame's aim, command
/// and shot agree. The eye converts the Bevy camera position into world FLU with
/// `flu_vector`; the muzzle pivot and every target pose are already FLU metres.
/// Its only firing decision is [`AutoAim::fire_ready`]; rounds
/// leave through `fire_gun`. The intent is cleared when the binding is released,
/// the pilot is not captured, input is blocked, the match is paused or
/// `Session::aim_observation_fresh` says the observation is stale.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut state: ResMut<AutoAim>,
    session: Res<Session>,
    ui: Res<HudState>,
    mut player: ResMut<Player>,
    gun: Res<Gun>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
) {
    state.execution_time_ns = session.fire_time_ns();
    // Freshness is judged at presentation, not at intended execution: the
    // execution time adds the input lead (rtt/2 + 32 ms, up to 150 ms), so
    // gating on it starves firing on any link with real round-trip delay
    // even while checkpoints arrive fresh. The solver still predicts ahead
    // to `execution_time_ns`; only the staleness gates use this age.
    state.observation_age_ms = session
        .presentation_time_ns()
        .saturating_sub(session.snapshot.time_ns) as f64
        / 1e6;
    state.fire_ready = false;
    state.proposed_rune = None;
    if state
        .last_rune
        .is_some_and(|shot| session.fire_time_ns() < shot.fired_ns)
    {
        state.last_rune = None;
    }
    let aiming = ui
        .controls
        .pressed(InputAction::AutoAim, &keys, Some(&buttons));
    let firing = ui
        .controls
        .pressed(InputAction::AutoFire, &keys, Some(&buttons));
    let Some(own) = session.presented_chassis() else {
        state.clear_intent();
        return;
    };
    if (!aiming && !firing)
        || !player.captured
        || ui.blocks_input()
        || session.paused
        || own.defeated
    {
        state.clear_intent();
        return;
    }
    if !session.aim_observation_fresh() || state.observation_age_ms > 300. {
        state.target = None;
        state.status = "AUTO: stale target observation".into();
        state.record_gate("stale");
        return;
    }
    let geometry = session.aim_geometry();
    // Never auto-fire through unverified geometry after a failed asset load.
    let pivot = DVec3::from_array(own.turret.translation_m);
    let eye = DVec3::from_array(rm_simulator_render::flu_vector(player.position));
    let direction = DQuat::from_rotation_z(f64::from(player.yaw_rad))
        * DQuat::from_rotation_y(-f64::from(player.pitch_rad))
        * DVec3::X;
    let targets = targets(&session, gun.shot.caliber);
    let displayed: Vec<_> = session
        .snapshot
        .chassis
        .iter()
        .filter_map(|robot| {
            session
                .remote_history
                .sample(robot.id, session.remote_view_time_ns)
        })
        .collect();
    let acquisition_targets = view_targets(&targets, &displayed);
    let selected_id = select_target(
        &acquisition_targets,
        eye,
        direction,
        state.target,
        geometry.as_ref(),
    )
    .map(|target| target.id);
    let selected = targets.iter().find(|target| Some(target.id) == selected_id);
    let Some(target) = selected else {
        state.target = None;
        state.status = "AUTO: searching".into();
        state.record_gate("searching");
        return;
    };
    state.target = Some(target.id);
    let lead_s = if aiming {
        own.config.dynamics.gimbal_response_s
    } else {
        0.
    };
    let Some(solution) = choose_solution(target, pivot, gun.shot, lead_s, geometry.as_ref()) else {
        state.status = format!("AUTO: {} / no shot", target.description());
        state.record_gate("no-shot");
        return;
    };
    if aiming {
        player.yaw_rad = solution.yaw_rad as f32;
        player.pitch_rad = solution.pitch_rad as f32;
    }
    state.proposed_rune = rune_shot(target, solution);
    let reason = if !firing {
        "aim only"
    } else if state.observation_age_ms > 150. {
        "stale target, tracking only"
    } else if session.presentation_time_ns() < gun.next_shot_ns {
        "weapon cadence"
    } else if !heat_allows(session.predicted_heat(), gun.shot.caliber) {
        "heat limit"
    } else if !rune_fire_allowed(state.last_rune, target, session.fire_time_ns()) {
        "rune confirmation"
    } else if geometry.is_none() {
        "geometry unavailable"
    } else if !clear_of_robots(
        &session,
        target.id,
        pivot,
        dquat(own.turret.rotation_wxyz),
        gun.shot,
        solution.flight_s,
    ) {
        "blocked by robot"
    } else if let Err(reason) = fire_gate_reason(
        target,
        solution,
        pivot,
        dquat(own.turret.rotation_wxyz),
        gun.shot,
        geometry.as_ref(),
    ) {
        reason
    } else {
        "firing"
    };
    state.fire_ready = reason == "firing";
    state.status = format!("AUTO: {} / {}", target.description(), reason);
    state.record_gate(if reason == "stale target, tracking only" {
        "tracking-only"
    } else {
        reason
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::{ChassisPlacement, Field, FieldConfig, Team};
    fn robot() -> ChassisSnapshot {
        Field::new(&FieldConfig {
            chassis: vec![ChassisPlacement {
                config: default(),
                spawn: Pose::at([8., 0., 1.]),
                team: Team::Blue,
                kind: rm_simulator_world::RobotKind::Infantry,
                performance: None,
            }],
            ..default()
        })
        .unwrap()
        .snapshot()
        .chassis
        .remove(0)
    }
    fn rotation(solution: Solution) -> DQuat {
        DQuat::from_rotation_z(solution.yaw_rad) * DQuat::from_rotation_y(-solution.pitch_rad)
    }
    #[test]
    fn acquisition_uses_displayed_pose_without_changing_impact_observation() {
        let mut latest = robot();
        latest.pose.translation_m = [8., 4., 1.];
        latest.velocity_m_s = [0., 5., 0.];
        let mut displayed = latest.clone();
        displayed.pose.translation_m = [8., 0., 1.];
        let displayed = vec![displayed];
        let targets = vec![Target {
            id: TargetId::Robot(latest.id),
            motion: Motion::Robot(&latest, 0.1),
            faces: (0..4).collect(),
        }];
        let view = view_targets(&targets, &displayed);
        let eye = DVec3::new(0., 0., 1.);
        assert!(acquisition(&targets[0], eye, DVec3::X, false, None).is_none());
        assert!(acquisition(&view[0], eye, DVec3::X, false, None).is_some());
        assert!(targets[0].pose(0, 0.2).translation_m[1] > 5.);
        assert!(view[0].pose(0, 0.).translation_m[1].abs() < 0.5);
    }

    #[test]
    fn fallback_uses_nearest_visible_robot_but_crosshair_wins() {
        let mut near = robot();
        near.pose.translation_m = [0., 4., 1.];
        let mut far = robot();
        far.pose.translation_m = [8., 0., 1.];
        let candidates = vec![
            Target {
                id: TargetId::Robot(1),
                motion: Motion::Robot(&near, 0.),
                faces: (0..4).collect(),
            },
            Target {
                id: TargetId::Robot(2),
                motion: Motion::Robot(&far, 0.),
                faces: (0..4).collect(),
            },
        ];
        let eye = DVec3::new(0., 0., 1.);
        assert_eq!(
            select_target(&candidates, eye, -DVec3::X, None, None)
                .unwrap()
                .id,
            TargetId::Robot(1)
        );
        assert_eq!(
            select_target(&candidates, eye, DVec3::X, None, None)
                .unwrap()
                .id,
            TargetId::Robot(2)
        );
        let mut field = Field::new(&FieldConfig::default()).unwrap();
        field
            .add_static_mesh(
                vec![[-2., 2., -2.], [2., 2., -2.], [2., 2., 4.], [-2., 2., 4.]],
                vec![[0, 1, 2], [0, 2, 3]],
            )
            .unwrap();
        let geometry = field.static_geometry_snapshot();
        assert_eq!(
            select_target(&candidates, eye, -DVec3::X, None, Some(&geometry))
                .unwrap()
                .id,
            TargetId::Robot(2)
        );
        assert!(select_target(&candidates[..1], eye, -DVec3::X, None, Some(&geometry)).is_none());
        assert!(select_target(&candidates, DVec3::splat(100.), -DVec3::X, None, None).is_none());
    }

    #[test]
    fn slabs_reject_parallel_misses_and_boxes_behind_camera() {
        let low = DVec3::new(2., -1., -1.);
        let high = DVec3::new(3., 1., 1.);
        assert!(ray_aabb(DVec3::ZERO, DVec3::X, low, high));
        assert!(!ray_aabb(DVec3::ZERO, -DVec3::X, low, high));
        assert!(!ray_aabb(DVec3::new(0., 2., 0.), DVec3::X, low, high));
    }
    #[test]
    fn leads_linear_and_spinning_armor_and_rejects_unreachable_shots() {
        let mut robot = robot();
        robot.velocity_m_s = [0., 1.5, 0.];
        robot.angular_velocity_rad_s = [0., 0., 5.];
        let target = Target {
            id: TargetId::Robot(robot.id),
            motion: Motion::Robot(&robot, 0.),
            faces: (0..4).collect(),
        };
        let pivot = DVec3::new(0., 0., 1.);
        let shot = Shot::at_limit(Caliber::Mm17);
        let solution = choose_solution(&target, pivot, shot, 0., None).unwrap();
        assert!(solution.yaw_rad > 0.02);
        assert!(solution.pitch_rad > 0.03);
        assert!(fire_gate(
            &target,
            solution,
            pivot,
            rotation(solution),
            shot,
            None
        ));
        assert!(!fire_gate(
            &target,
            solution,
            pivot,
            DQuat::IDENTITY,
            shot,
            None
        ));
        let far = DVec3::new(-100., 0., 1.);
        assert!(choose_solution(&target, far, Shot::at_limit(Caliber::Mm42), 0., None).is_none());
    }
    #[test]
    fn ballistic_solution_matches_actual_rapier_flight() {
        let robot = robot();
        let target = Target {
            id: TargetId::Robot(robot.id),
            motion: Motion::Robot(&robot, 0.),
            faces: (0..4).collect(),
        };
        let pivot = DVec3::new(0., 0., 1.);
        for caliber in [Caliber::Mm17, Caliber::Mm42] {
            let shot = Shot::at_limit(caliber);
            let solution = choose_solution(&target, pivot, shot, 0., None).unwrap();
            let rotation = rotation(solution);
            let mut field = Field::new(&FieldConfig {
                runes: vec![],
                outposts: vec![],
                floor_height_m: -10.,
                ..default()
            })
            .unwrap();
            field
                .fire(
                    Pose {
                        translation_m: (pivot + rotation * DVec3::X * f64::from(MUZZLE_FORWARD_M))
                            .to_array(),
                        rotation_wxyz: wxyz(rotation),
                    },
                    shot,
                    None,
                )
                .unwrap();
            let tick = rm_simulator_world::tick_ns() as f64;
            let ticks = (solution.flight_s * 1e9 / tick).round() as u64;
            field.step(ticks).unwrap();
            let actual = DVec3::from_array(field.projectile_snapshots()[0].position_m);
            let expected = DVec3::from_array(
                target
                    .pose(solution.face, ticks as f64 * tick * 1e-9)
                    .translation_m,
            );
            // The flight is sampled at a whole tick, so the target moves up to
            // half a tick past the solved intercept before the comparison.
            assert!(
                (actual - expected).length() < 0.1,
                "{caliber:?}: {}",
                (actual - expected).length()
            );
        }
    }
    #[test]
    fn outpost_and_active_rune_have_intercept_solutions() {
        let outpost = rm_simulator_world::Outpost::new(
            Pose::at([7., 0., 0.]),
            rm_simulator_world::outpost::DEFAULT_SPEED_RAD_S,
        )
        .unwrap()
        .snapshot(0);
        let target = Target {
            id: TargetId::Outpost(0),
            motion: Motion::Outpost(&outpost, 0.),
            faces: (0..3).collect(),
        };
        let pivot = DVec3::new(0., 0., 1.);
        let shot = Shot::at_limit(Caliber::Mm17);
        let solution = choose_solution(&target, pivot, shot, 0., None).unwrap();
        assert!(fire_gate(
            &target,
            solution,
            pivot,
            rotation(solution),
            shot,
            None
        ));
        let mut rune = rm_simulator_world::rune::SmallRune::new(Pose {
            translation_m: [8., 0., 1.8],
            rotation_wxyz: [1., 0., 0., 0.],
        })
        .unwrap();
        rune.activate(0).unwrap();
        let snapshot = rune.snapshot();
        let target = Target {
            id: TargetId::Rune(0),
            motion: Motion::Rune(&snapshot, 0),
            faces: vec![snapshot.active_blade.unwrap() as usize],
        };
        let solution = choose_solution(&target, pivot, shot, 0., None).unwrap();
        assert!(fire_gate(
            &target,
            solution,
            pivot,
            rotation(solution),
            shot,
            None
        ));
    }
    #[test]
    fn rune_selection_matches_own_color_and_uses_the_visible_scoring_side() {
        let mut session = crate::session::test_session(false);
        session.snapshot.outposts.clear();
        session.snapshot.runes = [0., std::f64::consts::PI]
            .map(|yaw| {
                rm_simulator_world::rune::SmallRune::new(Pose::yawed([0., 0., 1.8], yaw))
                    .unwrap()
                    .snapshot()
            })
            .to_vec();
        for (team, index, eye) in [
            (Team::Blue, 0, DVec3::new(-8., 0., 1.)),
            (Team::Red, 1, DVec3::new(8., 0., 1.)),
        ] {
            session.team = team;
            let candidates = targets(&session, Caliber::Mm17);
            let runes: Vec<_> = candidates
                .iter()
                .filter(|t| matches!(t.id, TargetId::Rune(_)))
                .collect();
            assert_eq!(runes.len(), 1);
            assert_eq!(runes[0].id, TargetId::Rune(index));
            let solution =
                choose_solution(runes[0], eye, Shot::at_limit(Caliber::Mm17), 0., None).unwrap();
            assert!(fire_gate(
                runes[0],
                solution,
                eye,
                rotation(solution),
                Shot::at_limit(Caliber::Mm17),
                None
            ));
            assert!(
                choose_solution(runes[0], -eye, Shot::at_limit(Caliber::Mm17), 0., None).is_none()
            );
            assert!(
                targets(&session, Caliber::Mm42)
                    .iter()
                    .all(|t| !matches!(t.id, TargetId::Rune(_)))
            );
        }
    }
    #[test]
    fn auto_fire_stops_at_the_heat_limit() {
        use rm_simulator_world::Caliber;
        assert!(heat_allows(None, Caliber::Mm17));
        // 90 + 10 reaches the limit of 100 without passing it.
        assert!(heat_allows(Some((900, 100)), Caliber::Mm17));
        assert!(!heat_allows(Some((901, 100)), Caliber::Mm17));
        assert!(!heat_allows(Some((100, 100)), Caliber::Mm42));
        assert!(heat_allows(Some((0, 100)), Caliber::Mm42));
    }
    #[test]
    fn rune_fire_waits_for_confirmation_and_button_release_does_not_bypass_it() {
        let mut rune = rm_simulator_world::rune::SmallRune::new(Pose::at([6., 0., 1.6]))
            .unwrap()
            .snapshot();
        let mut state = AutoAim {
            proposed_rune: Some(RuneShot {
                rune: 0,
                blade: 0,
                epoch_ns: rune.state_since_ns,
                group: rune.completed_groups,
                fired_ns: 0,
                flight_ns: 300_000_000,
            }),
            ..default()
        };
        state.shot_requested(0);
        state.clear_intent();
        let target = Target {
            id: TargetId::Rune(0),
            motion: Motion::Rune(&rune, 0),
            faces: vec![0],
        };
        assert!(!rune_fire_allowed(state.last_rune, &target, 499_000_000));
        assert!(!rune_fire_allowed(state.last_rune, &target, 600_000_000));
        assert!(rune_fire_allowed(
            state.last_rune,
            &target,
            RUNE_MISS_RETRY_NS
        ));
        rune.active_blades[0] = false;
        rune.active_blades[1] = true;
        let target = Target {
            id: TargetId::Rune(0),
            motion: Motion::Rune(&rune, 0),
            faces: vec![1],
        };
        assert!(!rune_fire_allowed(state.last_rune, &target, 499_000_000));
        assert!(rune_fire_allowed(state.last_rune, &target, 500_000_000));
    }
    #[test]
    fn motor_tracking_activates_small_rune_without_a_burst_per_blade() {
        let config = rm_simulator_world::ChassisConfig::default();
        let mut field = Field::new(&FieldConfig {
            runes: vec![default()],
            outposts: vec![],
            chassis: vec![ChassisPlacement {
                spawn: Pose::at([0., 0., config.rest_height_m()]),
                config,
                team: Team::Blue,
                kind: rm_simulator_world::RobotKind::Infantry,
                performance: None,
            }],
            ..default()
        })
        .unwrap();
        let shot = Shot::at_limit(Caliber::Mm17);
        let mut state = AutoAim::default();
        let mut times = Vec::new();
        for _ in 0..1000 {
            let snapshot = field.snapshot();
            if snapshot.runes[0].state == rm_simulator_world::rune::RuneState::Activated {
                break;
            }
            let own = &snapshot.chassis[0];
            let target = Target {
                id: TargetId::Rune(0),
                motion: Motion::Rune(&snapshot.runes[0], snapshot.time_ns),
                faces: vec![snapshot.runes[0].active_blade.unwrap() as usize],
            };
            let pivot = DVec3::from_array(own.turret.translation_m);
            if let Some(solution) = choose_solution(
                &target,
                pivot,
                shot,
                own.config.dynamics.gimbal_response_s,
                None,
            ) {
                field
                    .command_chassis(
                        0,
                        rm_simulator_world::ChassisCommand {
                            aim_yaw_rad: solution.yaw_rad,
                            aim_pitch_rad: solution.pitch_rad,
                            ..default()
                        },
                    )
                    .unwrap();
                if rune_fire_allowed(state.last_rune, &target, snapshot.time_ns)
                    && fire_gate(
                        &target,
                        solution,
                        pivot,
                        dquat(own.turret.rotation_wxyz),
                        shot,
                        None,
                    )
                {
                    state.proposed_rune = rune_shot(&target, solution);
                    state.shot_requested(snapshot.time_ns);
                    field
                        .fire(field.chassis_muzzle_pose(0).unwrap(), shot, Some(0))
                        .unwrap();
                    times.push(snapshot.time_ns);
                }
            }
            field.step(8).unwrap();
        }
        let snapshot = field.snapshot();
        eprintln!(
            "rune: {} shots, {} hits, {:?}",
            snapshot.shots_fired, snapshot.hits_detected, snapshot.runes[0].state
        );
        assert_eq!(
            snapshot.runes[0].state,
            rm_simulator_world::rune::RuneState::Activated
        );
        assert!(
            times
                .windows(2)
                .all(|pair| pair[1] - pair[0] >= RUNE_SHOT_INTERVAL_NS)
        );
        assert!(snapshot.shots_fired <= 7, "{} shots", snapshot.shots_fired);
    }
    #[test]
    fn motor_tracking_and_fire_control_hit_a_rotating_outpost() {
        let config = rm_simulator_world::ChassisConfig::default();
        let mut field = Field::new(&FieldConfig {
            runes: vec![],
            outposts: vec![rm_simulator_world::OutpostConfig {
                pivot_cad_m: None,
                origin: Pose::at([7., 0., 0.]),
                speed_rad_s: rm_simulator_world::outpost::DEFAULT_SPEED_RAD_S,
            }],
            chassis: vec![ChassisPlacement {
                spawn: Pose::at([0., 0., config.rest_height_m()]),
                config,
                team: Team::Red,
                kind: rm_simulator_world::RobotKind::Infantry,
                performance: None,
            }],
            ..default()
        })
        .unwrap();
        let shot = Shot::at_limit(Caliber::Mm17);
        let mut next_shot = 0;
        let started = std::time::Instant::now();
        for _ in 0..500 {
            let snapshot = field.snapshot();
            let own = &snapshot.chassis[0];
            let target = Target {
                id: TargetId::Outpost(0),
                motion: Motion::Outpost(&snapshot.outposts[0], 0.),
                faces: (0..3).collect(),
            };
            let pivot = DVec3::from_array(own.turret.translation_m);
            if let Some(solution) = choose_solution(
                &target,
                pivot,
                shot,
                own.config.dynamics.gimbal_response_s,
                None,
            ) {
                field
                    .command_chassis(
                        0,
                        rm_simulator_world::ChassisCommand {
                            aim_yaw_rad: solution.yaw_rad,
                            aim_pitch_rad: solution.pitch_rad,
                            ..default()
                        },
                    )
                    .unwrap();
                if snapshot.tick >= next_shot
                    && fire_gate(
                        &target,
                        solution,
                        pivot,
                        dquat(own.turret.rotation_wxyz),
                        shot,
                        None,
                    )
                {
                    field
                        .fire(field.chassis_muzzle_pose(0).unwrap(), shot, Some(0))
                        .unwrap();
                    next_shot = snapshot.tick + 100;
                }
            }
            field.step(8).unwrap();
        }
        let snapshot = field.snapshot();
        eprintln!(
            "outpost: {} shots, {} hits; 500 updates+physics in {:?}",
            snapshot.shots_fired,
            snapshot.hits_detected,
            started.elapsed()
        );
        assert!(snapshot.shots_fired > 3);
        assert!(snapshot.hits_detected > 3);
    }
    #[test]
    fn another_robot_between_barrel_and_target_blocks_auto_fire() {
        let mut session = crate::session::test_session(false);
        let mut blocker = robot();
        blocker.id = 99;
        blocker.pose.translation_m = [3., 0., 1.];
        session.snapshot.chassis.push(blocker);
        assert!(!clear_of_robots(
            &session,
            TargetId::Outpost(0),
            DVec3::new(0., 0., 1.),
            DQuat::IDENTITY,
            Shot::at_limit(Caliber::Mm17),
            0.25
        ));
        assert!(clear_of_robots(
            &session,
            TargetId::Outpost(0),
            DVec3::new(0., 3., 1.),
            DQuat::IDENTITY,
            Shot::at_limit(Caliber::Mm17),
            0.25
        ));
    }
    #[test]
    fn shared_bindings_release_and_menus_clear_the_trigger() {
        let mut session = crate::session::test_session(false);
        session.snapshot.runes.clear();
        session.snapshot.outposts.clear();
        let mut enemy = robot();
        enemy.id = 42;
        enemy.pose.translation_m = [5., 3., session.own_chassis().unwrap().pose.translation_m[2]];
        session.snapshot.chassis.push(enemy);
        let own = session.own_chassis().unwrap();
        let mut player = Player::at(
            rm_simulator_render::flu_position(own.turret.translation_m),
            0.,
            0.,
        );
        player.captured = true;
        let mut app = App::new();
        app.insert_resource(session)
            .insert_resource(player)
            .insert_resource(Gun::new(Shot::at_limit(Caliber::Mm17), 100_000_000))
            .init_resource::<HudState>()
            .init_resource::<AutoAim>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, update);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        assert!(
            app.world().resource::<AutoAim>().target.is_some(),
            "{} paused={} eye={:?} candidates={}",
            app.world().resource::<AutoAim>().status,
            app.world().resource::<Session>().paused,
            app.world().resource::<Player>().position,
            targets(app.world().resource::<Session>(), Caliber::Mm17).len()
        );
        assert!(
            !app.world().resource::<AutoAim>().fire_ready,
            "unaligned barrel cannot fire"
        );
        assert!(app.world().resource::<Player>().pitch_rad.abs() > 0.001);
        // Auto Fire alone never steers, but it can fire an already aligned gun.
        app.world_mut()
            .resource_mut::<HudState>()
            .controls
            .set(InputAction::AutoAim, 0, None);
        let solution = {
            let session = app.world().resource::<Session>();
            let candidates = targets(session, Caliber::Mm17);
            choose_solution(
                &candidates[0],
                DVec3::from_array(session.own_chassis().unwrap().turret.translation_m),
                Shot::at_limit(Caliber::Mm17),
                0.,
                None,
            )
            .unwrap()
        };
        {
            let mut session = app.world_mut().resource_mut::<Session>();
            session.snapshot.chassis[0].turret.rotation_wxyz = wxyz(rotation(solution));
        }
        {
            let mut player = app.world_mut().resource_mut::<Player>();
            player.yaw_rad = solution.yaw_rad as f32;
            player.pitch_rad = solution.pitch_rad as f32;
        }
        app.update();
        assert!(app.world().resource::<AutoAim>().fire_ready);
        assert_eq!(
            app.world().resource::<Player>().yaw_rad,
            solution.yaw_rad as f32
        );
        assert_eq!(
            app.world().resource::<Player>().pitch_rad,
            solution.pitch_rad as f32
        );
        app.world_mut().resource_mut::<HudState>().settings = true;
        app.update();
        assert!(app.world().resource::<AutoAim>().target.is_none());
        assert!(!app.world().resource::<AutoAim>().fire_ready);
        app.world_mut().resource_mut::<HudState>().settings = false;
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Right);
        app.update();
        assert!(app.world().resource::<AutoAim>().status.is_empty());
    }
}
